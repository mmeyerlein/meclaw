"""Tests for `scripts/pipeline/receipt.py` -- receipt v2 as a projection of events.

The receipt is never written beside the events: it is BUILT from them, so the
receipt the stream produced and the receipt rebuilt from the journal later
have to be the same document. It keeps every v1 key (the export reads `rev
dirty mode verdict plan stations`), and its summary lines are the lines
`scripts/gate.sh` prints today, byte for byte.
"""

import contextlib
import io
import json
import os
import pathlib
import sys
import tempfile
import unittest
from unittest import mock
from scripts.tests import store_isolation

REPO = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO))

from scripts.pipeline import cli, events, receipt, store  # noqa: E402

RUN = "run-0123456789ab"
REV = "abcdef0123456789abcdef0123456789abcdef01"
BASE = "0" * 40
FINGERPRINT = dict.fromkeys(
    ("rustc", "cargo", "nextest", "node", "playwright", "python", "kernel", "vcpu", "ram_g",
     "target_fs", "target_free_g", "ulimit_n", "incremental_policy", "linker", "sccache",
     "linger", "pdftotext", "portal_venv"), None)
FINGERPRINT.update(rustc="1.90", vcpu=24)
V1_KEYS = ("mode", "rev", "base", "dirty", "lock_wait_secs", "archive", "plan",
           "started", "finished", "stations", "verdict", "owner_decisions")


def at(sec):
    return f"2026-10-08T12:{sec // 60:02d}:{sec % 60:02d}Z"


def line(sec, kind, event, data, item=None):
    row = {"at": at(sec), "run": RUN, "kind": kind, "event": event,
           "data": dict(data, run=RUN) if event != "lane_fingerprint" else data}
    if item:
        row["item"] = item
    return row


def stream():
    """A two-round strand pass: `tests` red in round 1, flaky in round 2.

    A FLAKY station is a finding (VG TV-F1): the pass ends RED.
    """
    return [
        line(0, "status", "pass_planned",
             {"mode": "strand", "repo": "meclaw", "rev": REV, "base": BASE,
              "stations": ["fmt", "tests", "browser:display"],
              "reason": {"fmt": ["rust_src"], "tests": ["rust_src"]}, "cache_known": 1,
              "dirty": False, "lock_wait_secs": 2, "archive": "plans/w/receipts/s/x"}),
        line(1, "note", "run_dispatched",
             {"runner": "lane-a", "tree": "/srv/target/wt/x", "attempt": 1, "round": 1,
              "deadline_at": at(59), "ws": "/srv/target/ws/x"}),
        line(2, "note", "lane_fingerprint", FINGERPRINT),
        line(3, "status", "station_started",
             {"station": "fmt", "scope": "check", "needs": {"cpu": 1}, "cmds": 1}),
        line(4, "note", "station_log", {"station": "fmt", "path": "logs/strand-fmt.log"}),
        line(4, "note", "cache_hit",
             {"station": "fmt", "part": "", "key": "k" * 64, "from_run": "run-aaaaaaaaaaaa"}),
        line(5, "status", "station_ended",
             {"station": "fmt", "scope": "check", "verdict": "CACHED", "secs": 0, "exit": 0,
              "cache_from": "run-aaaaaaaaaaaa"}),
        line(6, "status", "station_started",
             {"station": "tests", "scope": "2 binaries", "needs": {"cpu": 4, "cargo": 1},
              "cmds": 1}),
        line(7, "note", "station_log", {"station": "tests", "path": "logs/strand-tests.log"}),
        line(20, "note", "test_result",
             {"station": "tests", "part": "bin-a", "verdict": "GREEN", "secs": 4.5,
              "tests": {"total": 10, "passed": 10, "failed": 0, "flaky": 0, "skipped": 0}}),
        line(30, "note", "test_result",
             {"station": "tests", "part": "bin-b", "verdict": "RED", "secs": 9.0,
              "tests": {"total": 5, "passed": 4, "failed": 1, "flaky": 0, "skipped": 0},
              "failed_names": ["bin-b::slow"]}),
        line(31, "status", "station_ended",
             {"station": "tests", "scope": "2 binaries", "verdict": "RED", "secs": 24,
              "build_s": 10.0, "run_s": 14.0, "exit": 1}),
        line(32, "status", "station_ended",
             {"station": "browser:display", "scope": "display", "verdict": "SKIP", "secs": 0,
              "exit": 0, "reason": "no playwright in this tree", "in_total": True}),
        line(33, "note", "run_dispatched",
             {"runner": "lane-a", "tree": "/srv/target/wt/x", "attempt": 2, "round": 2,
              "deadline_at": at(59), "ws": "/srv/target/ws/x"}),
        line(34, "status", "station_started",
             {"station": "tests", "scope": "2 binaries", "needs": {"cpu": 4, "cargo": 1},
              "cmds": 1}),
        line(40, "note", "test_result",
             {"station": "tests", "part": "bin-b", "verdict": "GREEN", "secs": 5.0,
              "tests": {"total": 5, "passed": 5, "failed": 0, "flaky": 0, "skipped": 0}}),
        line(41, "finding", "flake_isolated",
             {"station": "tests", "test": "bin-b::slow", "count": 10, "green": 10,
              "verdict": "FLAKY", "issue": "#1"}),
        line(42, "status", "station_ended",
             {"station": "tests", "scope": "2 binaries", "verdict": "FLAKY", "secs": 8,
              "build_s": 1.0, "run_s": 7.0, "exit": 0, "isolated": 1}),
        line(43, "status", "owner_decision",
             {"station": "persona-receipt", "decision": "without", "reason": "PoC"}),
        line(44, "status", "pass_ended",
             {"verdict": "RED", "secs": 44, "lane_seconds": 32, "critical_path_s": 30,
              "rounds": 2, "cache_hits": 1,
              "stations": {"green": 0, "red": 0, "cached": 1, "flaky": 1, "skip": 1,
                           "note": 0, "ask": 0, "aborted": 0}}),
    ]


def legacy_stream():
    """`stream()` with the round-2 FLAKY station legacy (VG TV-F4): the pass ends GREEN."""
    rows = stream()
    for row in rows:
        if row["event"] == "station_ended" and row["data"]["verdict"] == "FLAKY":
            row["data"]["legacy"] = True
        if row["event"] == "pass_ended":
            row["data"]["verdict"] = "GREEN"
    return rows


# gate-selftest loads every module into one process: no fixture event of this
# module may reach the metrics target of the caller's environment.
_METRICS_URL = mock.patch.dict(os.environ, {"MECLAW_PIPELINE_METRICS_URL": ""})


def setUpModule():  # noqa: N802 -- unittest's name
    store_isolation.isolate()
    _METRICS_URL.start()


def tearDownModule():  # noqa: N802 -- unittest's name
    _METRICS_URL.stop()
    store_isolation.restore()


class ReceiptCase(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.db = store.open(pathlib.Path(self.tmp.name) / "pipeline.db")

    def tearDown(self):
        store.close(self.db)
        self.tmp.cleanup()

    def test_receipt_v2_keeps_all_v1_keys(self):
        doc = receipt.project(stream())
        for key in V1_KEYS:
            self.assertIn(key, doc, key)
        self.assertEqual(doc["schema"], "meclaw.gate.receipt/2")
        self.assertEqual((doc["mode"], doc["rev"], doc["base"], doc["dirty"],
                          doc["lock_wait_secs"], doc["verdict"]),
                         ("strand", REV, BASE, False, 2, "RED"))
        self.assertEqual(doc["plan"], ["fmt", "tests", "browser:display"])
        self.assertEqual((doc["started"], doc["finished"]), (at(0), at(44)))
        self.assertEqual(doc["owner_decisions"], [
            {"station": "persona-receipt", "decision": "without", "reason": "PoC",
             "at": at(43)}])
        for key in ("repo", "lane", "run", "rounds", "critical_path_s", "lane_seconds",
                    "cache_hits", "fingerprint", "minimum", "plan_reason"):
            self.assertIn(key, doc, key)
        self.assertEqual((doc["repo"], doc["lane"], doc["run"]), ("meclaw", "lane-a", RUN))
        # The schema file names every v1 key as required, and the document passes it.
        schema = json.loads((REPO / "scripts/pipeline/receipt-v2.schema.json").read_text())
        self.assertTrue(set(V1_KEYS) <= set(schema["required"]))
        self.assertEqual(receipt.validate(doc), [])
        bad = dict(doc)
        del bad["plan"]
        self.assertTrue(receipt.validate(bad))
        # A pass still running is RUNNING with no `finished`, as in v1.
        running = receipt.project(stream()[:8])
        self.assertEqual((running["verdict"], running["finished"]), ("RUNNING", None))

    def test_graph_build_secs_travels_from_the_plan_into_the_receipt(self):
        """3a (#1199, re-check v2-kit c23 M-2): the seconds of the graph the
        narrowing built stand in `pass_planned`, in the receipt projected from
        it and in the receipt the gate finishes; a plan that built no graph
        carries no key."""
        import types  # noqa: PLC0415
        from scripts.pipeline import gate  # noqa: PLC0415
        o = types.SimpleNamespace(mode="strand", archive=None, cache="on")
        doc = {"stations": [], "rev": REV, "base": BASE,
               "graph_build": {"rev": REV, "secs": 27.2, "stored": True, "reason": "auto"}}
        p = gate._Pass(o, "/nonexistent-repo", doc, io.StringIO(), io.StringIO())
        planned = p.head(RUN, "lane-a", 1)[0]
        self.assertEqual(("pass_planned", 27.2),
                         (planned["event"], planned["data"]["graph_build_secs"]))
        rows = stream()
        rows[0] = dict(rows[0], data=dict(rows[0]["data"], graph_build_secs=27.2))
        projected = receipt.project(rows)
        self.assertEqual(27.2, projected["graph_build_secs"])
        self.assertEqual([], receipt.validate(projected))
        self.assertNotIn("graph_build_secs", receipt.project(stream()))
        p.pass_ended = {"data": {"verdict": "GREEN"}}
        with mock.patch.object(gate._Pass, "write_legacy") as legacy:
            p.finish({"mode": "strand", "rev": REV, "secs": 1, "verdict": "GREEN",
                      "stations": []}, "/nonexistent-dest", "local", 1)
        self.assertEqual(27.2, legacy.call_args[0][0]["graph_build_secs"])
        plain = gate._Pass(o, "/nonexistent-repo", {"stations": [], "rev": REV},
                           io.StringIO(), io.StringIO())
        self.assertNotIn("graph_build_secs", plain.head(RUN, "lane-a", 1)[0]["data"])

    def test_last_round_is_station_state(self):
        doc = receipt.project(stream())
        st = {s["name"]: s for s in doc["stations"]}
        self.assertEqual([s["name"] for s in doc["stations"]],
                         ["fmt", "tests", "browser:display"])
        self.assertEqual((st["tests"]["verdict"], st["tests"]["round"], st["tests"]["secs"]),
                         ("FLAKY", 2, 8))
        self.assertEqual(st["tests"]["tests"],
                         {"total": 5, "passed": 5, "failed": 0, "flaky": 0, "skipped": 0})
        self.assertEqual(st["tests"]["parts"], 1)
        self.assertEqual(st["tests"]["log"], "logs/strand-tests.log")
        self.assertEqual((st["fmt"]["verdict"], st["fmt"]["round"], st["fmt"]["cache_from"]),
                         ("CACHED", 1, "run-aaaaaaaaaaaa"))
        self.assertEqual([r["n"] for r in doc["rounds"]], [1, 2])
        self.assertEqual([s["verdict"] for s in doc["rounds"][0]["stations"]],
                         ["CACHED", "RED", "SKIP"])
        self.assertEqual(doc["rounds"][0]["verdict"], "RED")
        self.assertEqual(doc["rounds"][1]["verdict"], "RED")  # FLAKY is a finding
        self.assertEqual((doc["lane_seconds"], doc["critical_path_s"], doc["cache_hits"]),
                         (32, 30, 1))
        self.assertEqual(doc["fingerprint"], FINGERPRINT)

    def test_a_cached_station_row_carries_its_cached_parts(self):
        # Re-check H2 m2: the receipt names the cut parts with key and run.
        parts = [{"part": "a::x", "key": "1" * 64, "from_run": "run-111111111111"}]
        rows = stream()
        rows[11] = line(31, "status", "station_ended",
                        {"station": "tests", "scope": "2 binaries", "verdict": "CACHED",
                         "secs": 1, "exit": 6, "reason": "selection emptied by the cache",
                         "cached_parts": parts})
        doc = receipt.project(rows)
        row = [s for s in doc["rounds"][0]["stations"] if s["name"] == "tests"][0]
        self.assertEqual(parts, row["cached_parts"])

    def test_receipt_rebuilt_from_events_is_identical(self):
        streamed = receipt.project(stream())
        result = events.ingest_stream(self.db, stream(), RUN)
        self.assertFalse(result["lost"])
        rebuilt = receipt.build(self.db, RUN)
        self.assertEqual(rebuilt, streamed)
        self.assertEqual(json.dumps(rebuilt, sort_keys=True),
                         json.dumps(streamed, sort_keys=True))

    def test_summary_lines_are_byte_compatible(self):
        doc = receipt.project(stream())
        lines = receipt.summary_lines(doc)
        # The GATE and GATE-SUMMARY forms of scripts/gate.sh (`report` and the
        # summary printf): `GATE %s [%s] %ss %s%s` and
        # `GATE-SUMMARY %s %s %s/%s %ss %s` with the rev cut to seven.
        self.assertEqual(lines, [
            "GATE-PLAN strand abcdef0 stations=3 cached=1 reason=-",
            "GATE fmt [check] 0s CACHED",
            "GATE tests [2 binaries] 8s FLAKY",
            "GATE browser:display [display] 0s SKIP no playwright in this tree",
            "GATE-SUMMARY strand abcdef0 1/3 44s RED",
            "GATE-LANE lane-a crit=30 lane_s=32 rounds=2",
        ])
        # A FLAKY station whose tests are all legacy (VG TV-F4) counts green.
        doc = receipt.project(legacy_stream())
        self.assertEqual(receipt.summary_lines(doc)[-2],
                         "GATE-SUMMARY strand abcdef0 2/3 44s GREEN")
        self.assertEqual(doc["rounds"][1]["verdict"], "GREEN")
        # A plain v1-shaped run prints exactly what gate.sh printed for it.
        v1 = {"mode": "integration", "rev": REV, "verdict": "RED",
              "secs": 61,
              "stations": [
                  {"name": "precheck", "scope": "form", "secs": 3, "verdict": "GREEN"},
                  {"name": "claims", "scope": "docs", "secs": 1, "verdict": "NOTE",
                   "reason": "advisory"},
                  {"name": "tests", "scope": "t0", "secs": 57, "verdict": "RED"}]}
        self.assertEqual(receipt.gate_lines(v1), [
            "GATE precheck [form] 3s GREEN",
            "GATE claims [docs] 1s NOTE advisory",
            "GATE tests [t0] 57s RED",
            "GATE-SUMMARY integration abcdef0 1/2 61s RED",
        ])

    def test_secs_round_half_up(self):
        # The rule is int(x + 0.5): halves go up, never to the even neighbour
        # (Python's round(2.5) == 2 would print a different GATE line).
        self.assertEqual([receipt._secs(x) for x in (None, 0, 0.5, 1.49, 2.5, 3.5, 7)],
                         [0, 0, 1, 1, 3, 4, 7])
        line = receipt.gate_lines({"mode": "strand", "rev": REV, "verdict": "GREEN",
                                   "secs": 2.5, "stations": [
                                       {"name": "fmt", "scope": "check", "secs": 0.5,
                                        "verdict": "GREEN"}]})
        self.assertEqual(line, ["GATE fmt [check] 1s GREEN",
                                "GATE-SUMMARY strand abcdef0 1/1 3s GREEN"])

    def test_legacy_form_reads_cached_green_and_flaky_red(self):
        doc = receipt.project(stream())
        old = receipt.legacy(doc)
        st = {s["name"]: s for s in old["stations"]}
        self.assertEqual(st["fmt"]["verdict"], "GREEN")
        self.assertEqual(st["fmt"]["cache_from"], "run-aaaaaaaaaaaa")
        self.assertEqual((st["tests"]["verdict"], st["tests"]["flaky"]), ("RED", True))
        old = receipt.legacy(receipt.project(legacy_stream()))
        st = {s["name"]: s for s in old["stations"]}
        self.assertEqual((st["tests"]["verdict"], st["tests"]["flaky"]), ("GREEN", True))
        # The v2 document itself is not changed.
        self.assertEqual({s["name"]: s["verdict"] for s in doc["stations"]}["fmt"], "CACHED")

    def test_cli_prints_receipt_lines_from_the_store(self):
        events.ingest_stream(self.db, stream(), RUN)
        out = io.StringIO()
        path = str(pathlib.Path(self.tmp.name) / "pipeline.db")
        with contextlib.redirect_stdout(out):
            code = cli.main(["receipt", RUN, "--db", path, "--lines"])
        self.assertEqual(code, 0)
        self.assertIn("GATE-SUMMARY strand abcdef0 1/3 44s RED", out.getvalue())
        with contextlib.redirect_stderr(io.StringIO()):
            self.assertEqual(cli.main(["receipt", "run-ffffffffffff", "--db", path]), 1)


if __name__ == "__main__":
    unittest.main()
