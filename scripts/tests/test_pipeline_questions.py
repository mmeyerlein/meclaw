"""No station asks: the owner-question frame is gone (d54s kit F1, R-54-8).

GH #1593 (ADR-0054) removed `persona-receipt`, the one station that asked;
with no asking station the frame (verdict ASK, exit 4, `--decide`, the
question block of `questions.py`) was dead code and is gone too.
"""
import importlib.util
import inspect
import unittest

from scripts.pipeline import cache, executor, gate
from scripts.tests import store_isolation


def setUpModule():  # noqa: N802 -- unittest's name
    store_isolation.isolate()


def tearDownModule():  # noqa: N802 -- unittest's name
    store_isolation.restore()


class TestNoOwnerQuestion(unittest.TestCase):
    def test_the_question_module_is_gone(self):
        self.assertIsNone(importlib.util.find_spec("scripts.pipeline.questions"))

    def test_no_verdict_asks(self):
        self.assertFalse(hasattr(gate, "ASKING_STATIONS"))
        self.assertNotIn("ASK", gate.EXIT_BY_VERDICT)
        self.assertNotIn("ASK", executor.EXIT)
        self.assertEqual("GREEN", executor.pass_verdict([{"verdict": "GREEN"}]))

    def test_the_executor_reads_no_owner_answer(self):
        src = inspect.getsource(executor)
        self.assertNotIn("_decision_for", src)
        self.assertNotIn('get("decide")', src)


class TestNoPersonaReceipt(unittest.TestCase):
    """GH #1593: the executor has no arm for the station, the cache no entry."""

    def test_the_executor_has_no_persona_arm(self):
        src = inspect.getsource(executor)
        self.assertNotIn("persona-receipt", src)
        self.assertNotIn("PERSONA-RECEIPT", src)

    def test_the_cache_names_no_persona_receipt(self):
        self.assertNotIn("persona-receipt", cache.NEVER)
        self.assertIn("persona-cases", cache.INPUTS)


if __name__ == "__main__":
    unittest.main()
