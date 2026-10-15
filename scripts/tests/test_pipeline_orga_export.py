"""Tests for `scripts/pipeline/orga_export.py` -- `pipeline export-orga`.

The export writes a wave (or one pass) of the pipeline store in the form
Orga's linter reads (`<x>.graph.json`) plus the run rows in Orga's column
names (`<x>.runs.json`). No Orga code is loaded here: the grammar the export
must keep is copied below as tables from Orga's README (sections "Executable
tasks" and "Conditions on dependencies"); the real lint run is the acceptance
step, not a test. Every test builds its own store in a TempDir.
"""

import json
import os
import pathlib
import re
import sys
import tempfile
import unittest
from unittest import mock
from scripts.tests import store_isolation

REPO = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO))

from scripts.pipeline import orga_export, store  # noqa: E402

_METRICS_URL = mock.patch.dict(os.environ, {"MECLAW_PIPELINE_METRICS_URL": ""})


def setUpModule():
    store_isolation.isolate()
    _METRICS_URL.start()


def tearDownModule():
    _METRICS_URL.stop()
    store_isolation.restore()


# --- Orga's grammar, as tables (README "Executable tasks", "Conditions") -----

EXEC_KEYS = {"runner", "root", "workspace", "test_argv", "timeout_min", "retries", "evidence"}
EVIDENCE_KINDS = {"file", "message", "commit", "fact", "object", "external", "url", "turn"}
EXEC_ARG_MAX = 1000
EXEC_ARGV_MAX = 32
EXEC_TIMEOUT = (1, 120)
EXEC_RETRIES = (0, 3)
EXEC_WORKSPACE = re.compile(r"^[A-Za-z0-9-]{1,40}$")
EDGE_DATA_KEYS = {"dep", "lag_min", "when"}
DEPS = {"fs", "ss", "ff", "sf"}
GUARD_KEYS = {"outcome", "state", "evidence"}
OUTCOMES = {"success", "failure", "any"}
TERMINAL = {"task": {"done", "dropped"}, "issue": {"closed", "dropped"},
            "appointment": {"held", "cancelled", "moved"},
            "workpackage": {"finished", "stopped"}}
GRAPH_KEYS = {"member_ref", "basis", "truncated", "items", "edges", "parties"}
ITEM_KEYS = {"id", "key", "type", "state", "audience", "audience_set", "due_at",
             "starts_at", "ends_at", "parent", "doc", "evidence"}

# orga-eingaben.md E-DT-21: the x_pipeline fields Orga needs, word for word.
EINGABEN_FIELDS = ("needs", "inputs", "stage", "round", "verdict", "cache_from", "keep_ws")
# The plan's mapping keeps the original of what it maps (E-DT-6, E-DT-24) and
# marks what it derived (dispatch: tasks without doc.exec).
MAPPING_FIELDS = ("argv", "evidence", "timeout_min", "derived")

WAVE = "welle-x-2026-10-08"
LONG = "binary(" + "a" * 1500 + ")"


def check_exec(e):
    """The findings of one doc.exec against the table above ([] = clean)."""
    bad = [k for k in e if k not in EXEC_KEYS]
    if not isinstance(e.get("runner"), str) or not e["runner"].strip():
        bad.append("runner")
    root = e.get("root")
    if not isinstance(root, str) or not root.startswith("/") or ".." in root.split("/"):
        bad.append("root")
    argv = e.get("test_argv")
    if not isinstance(argv, list) or not 1 <= len(argv) <= EXEC_ARGV_MAX or not all(
            isinstance(a, str) and a.strip() and len(a) <= EXEC_ARG_MAX for a in argv):
        bad.append("test_argv")
    if "workspace" in e and not EXEC_WORKSPACE.match(str(e["workspace"])):
        bad.append("workspace")
    if "timeout_min" in e and not EXEC_TIMEOUT[0] <= e["timeout_min"] <= EXEC_TIMEOUT[1]:
        bad.append("timeout_min")
    if "retries" in e and not EXEC_RETRIES[0] <= e["retries"] <= EXEC_RETRIES[1]:
        bad.append("retries")
    ev = e.get("evidence", ["commit"])
    if not ev or len(set(ev)) != len(ev) or not set(ev) <= EVIDENCE_KINDS:
        bad.append("evidence")
    return bad


def check_guard(data, pred_type):
    """The findings of one depends_on edge's data ([] = clean)."""
    data = data or {}
    bad = [k for k in data if k not in EDGE_DATA_KEYS]
    if data.get("dep", "fs") not in DEPS:
        bad.append("dep")
    if "lag_min" in data and not isinstance(data["lag_min"], int):
        bad.append("lag_min")
    when = data.get("when")
    if when is None:
        return bad
    if not isinstance(when, dict) or not when or ("outcome" in when and "state" in when):
        return bad + ["guard_shape"]
    bad += ["guard_key" for k in when if k not in GUARD_KEYS]
    if "outcome" in when and when["outcome"] not in OUTCOMES:
        bad.append("guard_outcome")
    if "state" in when and not set(when["state"]) <= TERMINAL[pred_type]:
        bad.append("guard_state_type")
    if "evidence" in when and when["evidence"] not in EVIDENCE_KINDS:
        bad.append("guard_evidence")
    return bad


class Fixture:
    """One wave pass (three stations) plus one pass before the wave."""

    def __init__(self, path):
        self.db = store.open(path)
        db = self.db
        old = store.item_create(db, "workpackage", "old", doc={"mode": "strand", "repo": "meclaw",
                                                               "run": "run-old"},
                                at="2026-10-01T10:00:00Z")
        self.old = old["id"]
        wp = store.item_create(db, "workpackage", "pass", at="2026-10-08T10:00:00Z", doc={
            "repo": "meclaw", "mode": "strand", "stage": "strand", "strand": "s1", "wave": None,
            "run": "run-abc", "rev": "0123456789abcdef0123456789abcdef01234567",
            "plan": ["precheck", "tests", "docs"], "lane": "lane-a"})
        self.wp = wp["id"]
        v = store.item_transition(db, wp["id"], "planned", "active", 1, at="2026-10-08T10:00:01Z")
        store.item_transition(db, wp["id"], "active", "finished", v, at="2026-10-08T10:09:00Z")
        self.tasks = {}
        self.runs = {}
        specs = {
            "precheck": ({"exec": {"runner": "lane-a", "root": "/ws/tree",
                                   "test_argv": ["scripts/gate.sh", "strand", "--only", "precheck"],
                                   "timeout_min": 15, "retries": 0, "evidence": ["log"], "extra": 1},
                          "station": {"name": "precheck", "scope": "x", "needs": {"cpu": 1},
                                      "inputs": ["scripts/**"], "stage": ["S", "I"], "keep_ws": True}},
                         "GREEN", None, {"kind": "log", "path": "target/gate/precheck.log"}, 61.2),
            "tests": ({"exec": {"runner": "lane-a", "root": "/ws/tree",
                                "test_argv": ["scripts/test-tier.sh", "filter", LONG],
                                "timeout_min": 180, "retries": 0, "evidence": ["log", "cache"]},
                       "station": {"name": "tests"}},
                      "CACHED", "tr-older", {"kind": "cache", "run": "tr-older"}, 2.0),
            "docs": ({"scope": "docs/", "station": "docs"},
                     "RED", None, {"kind": "receipt", "path": "plans/w/receipts/g1"}, 3.0),
        }
        for name, (doc, verdict, cache_from, evidence, secs) in specs.items():
            t = store.item_create(db, "task", name, parent=wp["id"], doc=doc, at="2026-10-08T10:00:02Z")
            self.tasks[name] = t
            r = store.run_open(db, t["id"], "lane-a", key=t["key"], at="2026-10-08T10:01:00Z")
            store.run_close(db, r, verdict, run_s=secs, cache_from=cache_from, evidence=evidence,
                            at="2026-10-08T10:02:00Z")
            self.runs[name] = r
        store.result_append(db, self.runs["tests"], "tests/part-1", "nextest-binary",
                            {"part": "part-1", "verdict": "CACHED", "secs": 2.0},
                            at="2026-10-08T10:02:00Z")
        store.edge_link(db, self.tasks["tests"]["id"], self.tasks["precheck"]["id"], "depends_on",
                        "test", {"dep": "fs", "lag_min": 0, "when": {"outcome": "success"}},
                        at="2026-10-08T10:00:03Z")
        with store.tx(db):
            db.execute("INSERT INTO calibration VALUES (?,?,?,?,?,?,?,?,?)",
                       (self.tasks["precheck"]["id"], "task", "lane-a", None, None, 61.2,
                        "2026-10-08T10:02:00Z", "precheck@meclaw", None))
        for kind, event in (("status", "pass_planned"), ("finding", "watchdog_fired"),
                            ("note", "station_log"), ("chain", "chain_link")):
            store.journal_add(db, "2026-10-08T10:03:00Z", wp["id"], "run-abc", kind, event,
                              f"{event} run=run-abc", {"run": "run-abc"})
        store.journal_add(db, "2026-10-01T10:03:00Z", old["id"], "run-old", "status", "pass_planned",
                          "old", {"run": "run-old"})


class ExportOrga(unittest.TestCase):
    def test_a_pre_task_runs_in_the_kits_python(self):
        """Re-check tv-bn M7p H4: the derived `pre` argv starts in `lane.PYTHON` --
        a bare `python3 scripts/pipeline.py pre` ran past the -I entry lock."""
        from scripts.pipeline import lane
        doc = orga_export._task({"doc": {}, "title": "pre"}, {"mode": "pre"}, [], {})[0]
        self.assertEqual(lane.python_argv("scripts/pipeline.py", "pre"), doc["exec"]["test_argv"])


    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.dir = pathlib.Path(self.tmp.name)
        self.fx = Fixture(self.dir / "p.db")
        self.graph, self.runs = orga_export.build(
            self.fx.db, wave=WAVE, since="2026-10-08T00:00:00Z", until="2026-10-09T00:00:00Z")
        self.items = {i["id"]: i for i in self.graph["items"]}

    def tearDown(self):
        store.close(self.fx.db)
        self.tmp.cleanup()

    def task(self, name):
        return self.items[self.fx.tasks[name]["id"]]

    def test_export_orga_items_have_exec_and_guards_in_grammar(self):
        self.assertLessEqual(set(self.graph), GRAPH_KEYS)
        self.assertNotIn(self.fx.old, self.items)            # before the wave window
        wp = self.items[self.fx.wp]
        self.assertEqual(("workpackage", "finished", "meclaw strand s1"),
                         (wp["type"], wp["state"], wp["title"]))
        for it in self.graph["items"]:
            self.assertLessEqual(ITEM_KEYS, set(it))
            self.assertEqual("self", it["audience"])
            if it["type"] == "task":
                self.assertIn("exec", it["doc"], it["key"])
                self.assertEqual([], check_exec(it["doc"]["exec"]), it["key"])
        self.assertIn("exec", self.task("docs")["doc"]["x_pipeline"]["derived"])
        deps = [e for e in self.graph["edges"] if e["kind"] == "depends_on"]
        succ = {e["src"] for e in deps}
        for name in ("tests", "docs"):                        # the store's edge + the plan order
            self.assertIn(self.fx.tasks[name]["id"], succ)
        for e in deps:
            self.assertIn(e["src"], self.items)
            self.assertIn(e["dst"], self.items)
            self.assertEqual([], check_guard(e["data"], self.items[e["dst"]]["type"]), e)

    def test_export_orga_x_pipeline_fields_match_eingaben_list(self):
        self.assertEqual(EINGABEN_FIELDS, orga_export.X_PIPELINE_FIELDS)
        self.assertEqual(MAPPING_FIELDS, orga_export.MAPPING_FIELDS)
        allowed = set(EINGABEN_FIELDS) | set(MAPPING_FIELDS)
        rows = list(self.graph["items"]) + list(self.runs["task_runs"])
        for row in rows:
            xp = (row.get("doc") or {}).get("x_pipeline", {}) if "doc" in row else row.get("x_pipeline", {})
            self.assertLessEqual(set(xp), allowed, row.get("key"))
        pre = self.task("precheck")["doc"]["x_pipeline"]
        self.assertEqual({"cpu": 1}, pre["needs"])
        self.assertEqual(["scripts/**"], pre["inputs"])
        self.assertEqual(["S", "I"], pre["stage"])
        self.assertTrue(pre["keep_ws"])
        self.assertEqual(("GREEN", 1), (pre["verdict"], pre["round"]))
        self.assertEqual("tr-older", self.task("tests")["doc"]["x_pipeline"]["cache_from"])
        self.assertEqual("strand", self.items[self.fx.wp]["doc"]["x_pipeline"]["stage"])
        run = {r["item"]: r for r in self.runs["task_runs"]}[self.fx.tasks["tests"]["id"]]
        self.assertEqual({"round": 1, "verdict": "CACHED", "cache_from": "tr-older"}, run["x_pipeline"])

    def test_export_orga_maps_evidence_kinds_and_long_argv(self):
        pre, tests, docs = (self.task(n)["doc"] for n in ("precheck", "tests", "docs"))
        self.assertEqual((["file"], ["log"]), (pre["exec"]["evidence"], pre["x_pipeline"]["evidence"]))
        self.assertEqual((["file", "external"], ["log", "cache"]),
                         (tests["exec"]["evidence"], tests["x_pipeline"]["evidence"]))
        self.assertEqual(["scripts/test-tier.sh", "filter", "@argv:tests"], tests["exec"]["test_argv"])
        self.assertEqual(["scripts/test-tier.sh", "filter", LONG], tests["x_pipeline"]["argv"])
        self.assertEqual((120, 180), (tests["exec"]["timeout_min"], tests["x_pipeline"]["timeout_min"]))
        self.assertEqual(["file"], docs["exec"]["evidence"])
        for it in self.graph["items"]:
            for entry in it["evidence"]:
                self.assertIn(entry["kind"], EVIDENCE_KINDS)
        self.assertEqual([{"kind": "external", "target": "run:tr-older"}], self.task("tests")["evidence"])
        self.assertEqual("file", self.task("docs")["evidence"][0]["kind"])
        self.assertEqual("commit", self.items[self.fx.wp]["evidence"][0]["kind"])
        for r in self.runs["task_runs"]:
            for entry in r["evidence"] or []:
                self.assertIn(entry["kind"], EVIDENCE_KINDS)
        self.assertEqual({"note", "finding", "status"}, {j["kind"] for j in self.runs["journal"]})
        cal = self.runs["calibration"]
        self.assertEqual([(2, 61)], [(c["actual_min"], c["actual_s"]) for c in cal
                                     if c["item"] == self.fx.tasks["precheck"]["id"]])
        self.assertEqual(1, len(self.runs["task_run_results"]))

    def test_export_orga_exec_has_only_orga_keys(self):
        for it in self.graph["items"]:
            ex = (it["doc"] or {}).get("exec")
            if ex is not None:
                self.assertLessEqual(set(ex), EXEC_KEYS, it["key"])
        self.assertNotIn("extra", self.task("precheck")["doc"]["exec"])
        self.assertNotIn("in_capture", json.dumps(self.graph))
        for it in self.graph["items"]:
            self.assertLessEqual(set(it["doc"] or {}), {"exec", "x_pipeline"}, it["key"])

    def test_export_writes_two_files_named_after_the_wave(self):
        out = self.dir / "out"
        g, r = orga_export.export(self.fx.db, wave=WAVE, out_dir=out,
                                  since="2026-10-08T00:00:00Z", until="2026-10-09T00:00:00Z")
        self.assertEqual((out / f"{WAVE}.graph.json", out / f"{WAVE}.runs.json"), (g, r))
        self.assertEqual(self.graph, json.loads(g.read_text()))
        g2, _ = orga_export.export(self.fx.db, run="run-abc", out_dir=out)
        self.assertEqual("run-abc.graph.json", g2.name)
        self.assertEqual(4, len(json.loads(g2.read_text())["items"]))


if __name__ == "__main__":
    unittest.main()
