"""Tests for the dashboard `dev-tempo` and its alert rules (W0, V0 section 8).

Both files live under `workshop/grafana/` -- operations, not product -- and
`workshop/` never travels; in the public tree this module skips itself in
`setUpModule()` (the display-lab form).
"""

import json
import re
import os
import pathlib
import sys
import unittest
from unittest import mock
from scripts.tests import store_isolation

REPO = pathlib.Path(__file__).resolve().parents[2]
GRAFANA = REPO / "workshop" / "grafana"
sys.path.insert(0, str(REPO))


# gate-selftest loads every module into one process: no fixture event may
# reach the metrics target of the caller's environment.
_METRICS_URL = mock.patch.dict(os.environ, {"MECLAW_PIPELINE_METRICS_URL": ""})
_STARTED = []


def setUpModule():  # noqa: N802 -- unittest's name
    store_isolation.isolate()
    if not GRAFANA.is_dir():
        raise unittest.SkipTest("workshop/grafana/ is not in this tree")
    _METRICS_URL.start()
    _STARTED.append(True)


def tearDownModule():  # noqa: N802 -- unittest's name
    if _STARTED:
        _STARTED.clear()
        _METRICS_URL.stop()
    store_isolation.restore()


class GrafanaCase(unittest.TestCase):
    def test_dashboard_has_the_v0_panels_and_uid(self):
        dash = json.loads((GRAFANA / "dev-tempo.json").read_text())
        self.assertEqual(dash["uid"], "dev-tempo")
        panels = [p for p in dash["panels"] if p.get("type") != "row"]
        self.assertEqual(sorted(p["id"] for p in panels), list(range(1, 14)))
        for p in panels:
            self.assertTrue(p["targets"], p["title"])
            for t in p["targets"]:
                self.assertIn('db="devtempo"', t["expr"], p["title"])
                self.assertEqual(t["datasource"]["uid"], "${DS_VM}")
        names = [v["name"] for v in dash["templating"]["list"]]
        self.assertEqual(names, ["repo", "lane", "stage"])
        self.assertNotIn('"uid": "victoriametrics"', json.dumps(dash))

    def test_alert_rules_name_contact_point(self):
        doc = json.loads((GRAFANA / "dev-tempo-alerts.json").read_text())
        titles = [r["title"] for r in doc["rules"]]
        self.assertEqual(len(titles), 4)  # W0 three + the W2 stray-target rule
        self.assertTrue(doc["contact_point"])  # the existing mail contact point, by name
        for r in doc["rules"]:
            self.assertEqual(r["notification_settings"]["receiver"], doc["contact_point"])
            self.assertTrue(r["uid"].startswith("dev-tempo-"))
            self.assertIn('db="devtempo"', r["data"][0]["model"]["expr"])


    def test_stray_rule_counts_the_24_hours_once(self):
        # Review Zb I2: the metric only counts strays past STRAY_FINDING_AGE_S;
        # a second 24 h window in the rule would alarm after ~48 h.
        doc = json.loads((GRAFANA / "dev-tempo-alerts.json").read_text())
        rule = [r for r in doc["rules"] if r["uid"] == "dev-tempo-stray-target"][0]
        expr = rule["data"][0]["model"]["expr"]
        self.assertIn("pipe_lane_stray_targets", expr)
        self.assertNotIn("24h", expr)
        self.assertNotIn("min_over_time", expr)
        self.assertEqual(rule["for"], "0s")
        self.assertLessEqual(rule["data"][0]["relativeTimeRange"]["from"], 3600)

    def test_dashboard_quantiles_are_per_run(self):
        # p50/p90 over the runs of a day, not over 5-minute resamples (time-weighted)
        dash = json.loads((GRAFANA / "dev-tempo.json").read_text())
        exprs = [t["expr"] for p in dash["panels"] for t in p.get("targets", [])]
        self.assertFalse([e for e in exprs if "quantile_over_time" in e], exprs)
        q = [e for e in exprs if "histogram_quantile" in e]
        self.assertEqual(len(q), 3)
        for e in q:
            self.assertIn("histogram_over_time(", e)
            self.assertIn("vmrange", e)


    def test_pre_runs_never_dilute_the_gate_panels(self):
        # W4 re-check: record_pre ends a `mode: pre` pass, so pipe_pass
        # carries pre runs; every gate panel names its modes or drops pre.
        dash = json.loads((GRAFANA / "dev-tempo.json").read_text())
        for p in dash["panels"]:
            for t in p.get("targets", []):
                for sel in re.findall(r"pipe_pass_\w+\{([^}]*)\}", t["expr"]):
                    self.assertTrue("mode=" in sel or 'mode!="pre"' in sel or "mode=~" in sel,
                                    (p["title"], sel))

if __name__ == "__main__":
    unittest.main()
