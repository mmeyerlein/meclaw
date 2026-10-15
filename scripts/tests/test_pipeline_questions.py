"""GH #1149: an ASK of the pipeline gate prints the owner's question block.

Since W3 the question text lived only in `scripts/gate_legacy.sh`; the
pipeline answered ASK with the summary and nothing to do next.
"""
import io
import types
import unittest
from unittest import mock

from scripts.pipeline import gate, questions
from scripts.tests import store_isolation


def _finish(verdict, stations, mode="integration"):
    o = types.SimpleNamespace(mode=mode, archive=None, cache="on")
    out = io.StringIO()
    p = gate._Pass(o, "/nonexistent-repo", {"stations": [], "rev": "abc"}, out, io.StringIO())
    p.pass_ended = {"data": {"verdict": verdict}}
    v2 = {"mode": mode, "rev": "abc1234", "secs": 1, "verdict": verdict, "stations": stations}
    with mock.patch.object(gate._Pass, "write_legacy"):
        rc = p.finish(v2, "/nonexistent-dest", "local", 1)
    return rc, out.getvalue()

def setUpModule():  # noqa: N802 -- unittest's name
    store_isolation.isolate()


def tearDownModule():  # noqa: N802 -- unittest's name
    store_isolation.restore()


class TestOwnerQuestion(unittest.TestCase):
    def test_ask_prints_the_three_answers_with_the_mode(self):
        rc, text = _finish("ASK", [{"name": "persona-receipt", "secs": 1, "verdict": "ASK",
                                    "reason": "sources moved"}])
        self.assertEqual(rc, 4)
        self.assertIn("--engine local --budget-eur <EUR>", text)
        self.assertIn("--engine paid --budget-eur <EUR>", text)
        self.assertIn('scripts/gate.sh integration --decide persona-receipt=without:"OR-<id>: <reason>"', text)

    def test_green_and_red_print_no_question(self):
        for verdict in ("GREEN", "RED"):
            _, text = _finish(verdict, [{"name": "persona-receipt", "secs": 1, "verdict": "NOTE"}])
            self.assertNotIn("Three answers", text)

    def test_every_asking_station_has_a_question(self):
        self.assertEqual(set(gate.ASKING_STATIONS), set(questions.BY_STATION))


if __name__ == "__main__":
    unittest.main()
