"""Tests for `scripts/pipeline/metrics.py` and `grafana_push.py`.

Metrics are a side channel: a line has the form of V0 section 8 with the time
of its event, a dead target never fails a run (it becomes `metrics_failed`),
and a second push of the same rows sends the same points. The Grafana upload
takes its password from the caller's environment and never opens a `.env`.
"""

import http.server
import os
import pathlib
import re
import sys
import tempfile
import threading
import unittest
from unittest import mock
from scripts.tests import kit_time, store_isolation

REPO = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO))

from scripts.pipeline import events, metrics, store  # noqa: E402

RUN = "run-0123456789ab"
AT = "2026-10-08T12:00:00Z"


class _Sink(http.server.BaseHTTPRequestHandler):
    bodies = []
    status = 204

    def do_POST(self):  # noqa: N802 -- http.server's name
        n = int(self.headers.get("Content-Length") or 0)
        type(self).bodies.append(self.rfile.read(n).decode())
        self.send_response(type(self).status)
        self.end_headers()

    def log_message(self, *args):
        pass


def _server(status):
    handler = type("Sink", (_Sink,), {"bodies": [], "status": status})
    srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    return srv, handler, f"http://127.0.0.1:{srv.server_address[1]}/write?db=devtempo"


# gate-selftest loads every module into one process: no fixture event of this
# module may reach the metrics target of the caller's environment.
_METRICS_URL = mock.patch.dict(os.environ, {"MECLAW_PIPELINE_METRICS_URL": ""})


def setUpModule():  # noqa: N802 -- unittest's name
    store_isolation.isolate()
    _METRICS_URL.start()


def tearDownModule():  # noqa: N802 -- unittest's name
    _METRICS_URL.stop()
    store_isolation.restore()


class MetricsCase(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.db = store.open(pathlib.Path(self.tmp.name) / "pipeline.db")

    def tearDown(self):
        store.close(self.db)
        self.tmp.cleanup()

    def emit_pass(self, run=RUN):
        events.emit(self.db, "pass_planned", "status",
                    {"run": run, "mode": "strand", "repo": "meclaw", "rev": "a" * 40,
                     "base": "b" * 40, "stations": ["precheck"], "reason": {},
                     "cache_known": 0, "dirty": False, "diff": "small"}, run=run, at=AT)
        events.emit(self.db, "run_dispatched", "note",
                    {"run": run, "runner": "lane-a", "tree": None, "attempt": 1, "round": 1,
                     "deadline_at": None, "ws": None}, run=run, at=AT)
        events.emit(self.db, "station_ended", "status",
                    {"run": run, "station": "precheck", "verdict": "GREEN", "secs": 3,
                     "exit": 0, "scope": "form", "build_s": 1.5}, run=run,
                    at="2026-10-08T12:00:03Z")
        events.emit(self.db, "pass_ended", "status",
                    {"run": run, "verdict": "GREEN", "secs": 3, "lane_seconds": 3,
                     "critical_path_s": 3, "rounds": 1, "cache_hits": 0,
                     "stations": {"green": 1, "red": 0, "cached": 0, "flaky": 0, "skip": 0,
                                  "note": 0, "ask": 0, "aborted": 0}}, run=run,
                    at="2026-10-08T12:00:04Z")

    def test_metrics_line_format(self):
        with mock.patch.dict(os.environ, {metrics.URL_ENV: ""}):
            self.emit_pass()
        self.assertEqual(metrics.lines(self.db), [
            "pipe_station,repo=meclaw,mode=strand,station=precheck,runner=lane-a,"
            "verdict=GREEN,stage=strand secs=3,build_s=1.5,round=1,cached=0,slow_n=0 "
            "1791460803000000000",
            "pipe_pass,repo=meclaw,mode=strand,runner=lane-a,verdict=GREEN,diff=small "
            "secs=3,lane_seconds=3,critical_path_s=3,rounds=1,cache_hits=0,stations=1 "
            "1791460804000000000",
        ])
        self.assertEqual(metrics.line("m", (("t", "a b,c=d"),), (("f", 1),), AT),
                         r"m,t=a\ b\,c\=d f=1 1791460800000000000")
        self.assertIn(metrics.hook, events.HOOKS)

    def test_metrics_failure_does_not_fail_run(self):
        srv, handler, url = _server(500)
        try:
            with mock.patch.dict(os.environ, {metrics.URL_ENV: url}):
                self.emit_pass()  # no exception: the run goes on
        finally:
            srv.shutdown()
        failed = [r for r in events.read_run(self.db, RUN) if r["event"] == "metrics_failed"]
        self.assertEqual(len(failed), 1)  # the breaker: the first failure ends the pushes
        self.assertEqual(failed[0]["data"]["url_host"], "127.0.0.1")
        self.assertNotIn("http", failed[0]["data"]["url_host"])
        self.assertEqual(len(handler.bodies), 1)
        # a closed port is the same: an event, no crash
        with mock.patch.dict(os.environ, {metrics.URL_ENV: "http://127.0.0.1:9/write"}):
            self.emit_pass("run-ffffffffffff")
        self.assertEqual(sum(r["event"] == "metrics_failed"
                             for r in events.read_run(self.db, "run-ffffffffffff")), 1)

    def test_metrics_breaker_after_first_failure(self):
        # a dead target costs one timeout per run, not one per event; another
        # run tries again, and `push --since` sends the skipped lines later
        srv, handler, url = _server(500)
        try:
            with mock.patch.dict(os.environ, {metrics.URL_ENV: url}):
                self.emit_pass()
                self.emit_pass("run-eeeeeeeeeeee")
        finally:
            srv.shutdown()
        self.assertEqual(len(handler.bodies), 2)  # one try per run
        self.assertEqual(len(metrics.lines(self.db, runs=[RUN])), 2)

    def test_pipeline_test_modules_isolate_the_metrics_url(self):
        # gate-selftest loads every module into ONE process: a module that
        # emits events must not push its fixtures to the URL of the caller
        import importlib
        names = sorted(f.stem for f in (REPO / "scripts" / "tests").glob("test_pipeline_*.py"))
        self.assertIn("test_pipeline_events", names)
        with mock.patch.dict(os.environ, {metrics.URL_ENV: "http://127.0.0.1:9/write"}):
            for name in names:
                mod = importlib.import_module(f"scripts.tests.{name}")
                # W0b: every module, this one included, has its setUpModule
                self.assertTrue(hasattr(mod, "setUpModule"), name)
                if mod.__name__ == __name__:
                    continue  # this module's patcher already runs (setUpModule)
                try:
                    mod.setUpModule()
                except unittest.SkipTest:
                    pass
                try:
                    self.assertEqual(metrics.url(), "", name)
                finally:
                    if hasattr(mod, "tearDownModule"):
                        mod.tearDownModule()
                self.assertEqual(metrics.url(), "http://127.0.0.1:9/write", name)

    def test_isolation_check_leaves_no_foreign_url(self):
        # the check above must not start this module's own patcher a second
        # time: its tearDownModule would then restore the check's dead URL
        import subprocess
        code = ("import os, unittest\n"
                "os.environ.pop('MECLAW_PIPELINE_METRICS_URL', None)\n"
                "unittest.main(module='scripts.tests.test_pipeline_metrics', exit=False,\n"
                "              argv=['x', '-q', 'MetricsCase.test_pipeline_test_modules_"
                "isolate_the_metrics_url'])\n"
                "print('URL=%r' % os.environ.get('MECLAW_PIPELINE_METRICS_URL'))\n")
        out = subprocess.run([sys.executable, "-c", code], cwd=REPO, capture_output=True,
                             text=True, check=False, timeout=2 * kit_time.HANG)
        self.assertIn("URL=None", out.stdout, out.stdout + out.stderr)

    # Re-check tb-s3-store2 m-n2: `pipe_pass` kept the stream's GREEN of a pass
    # the store lost a record of. The finding turns its line ABORTED (field
    # `lost` = 1) for every push and pushes that line once when it arrives.
    def test_a_store_lost_pass_is_pushed_aborted(self):
        with mock.patch.dict(os.environ, {metrics.URL_ENV: ""}):
            self.emit_pass()
        srv, handler, url = _server(204)
        try:
            with mock.patch.dict(os.environ, {metrics.URL_ENV: url}):
                events.emit(self.db, "process_finding", "finding",
                            {"rule": events.STORE_LOST_RULE, "subject": RUN, "value": 1,
                             "threshold": 0, "since": "2026-10-08T12:00:05Z",
                             "records": ["close"]}, run=RUN, at="2026-10-08T12:00:05Z")
        finally:
            srv.shutdown()
        want = ("pipe_pass,repo=meclaw,mode=strand,runner=lane-a,verdict=ABORTED,diff=small "
                "secs=3,lane_seconds=3,critical_path_s=3,rounds=1,cache_hits=0,stations=1,"
                "lost=1 1791460804000000000")
        # the retro hook pushes its `pipe_finding` line beside it
        self.assertEqual([b for b in handler.bodies if b.startswith("pipe_pass")], [want + "\n"])
        self.assertEqual([ln for ln in metrics.lines(self.db) if ln.startswith("pipe_pass")],
                         [want])

    def test_metrics_push_since_is_idempotent(self):
        with mock.patch.dict(os.environ, {metrics.URL_ENV: ""}):
            self.emit_pass()
        srv, handler, url = _server(204)
        try:
            first = metrics.push(url, metrics.lines(self.db, since="2026-10-08T00:00:00Z"))
            second = metrics.push(url, metrics.lines(self.db, since="2026-10-08T00:00:00Z"))
        finally:
            srv.shutdown()
        self.assertEqual((first, second), (2, 2))
        self.assertEqual(handler.bodies[0], handler.bodies[1])
        self.assertEqual(metrics.lines(self.db, since="2026-10-08T12:00:04Z")[0][:9],
                         "pipe_pass")

    def test_diff_class(self):
        libs = {"meclaw-cells", "meclaw-core"}
        self.assertEqual(metrics.diff_class(["crates/meclaw-core/src/a.rs"], libs), "lib")
        self.assertEqual(metrics.diff_class(["scripts/a.py", "crates/x/tests/t.rs"], libs),
                         "small")
        self.assertEqual(metrics.diff_class(["a", "b", "c", "d"], libs), "other")

    def test_grafana_push_never_reads_env_file(self):
        src = (REPO / "scripts" / "pipeline" / "grafana_push.py").read_text()
        self.assertNotRegex(src, r"\.env\b")
        self.assertNotRegex(src, r"\bopen\(")
        self.assertIn("GRAFANA_ADMIN_PASS", src)
        self.assertIsNone(re.search(r"\b\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}\b", src))


if __name__ == "__main__":
    unittest.main()
