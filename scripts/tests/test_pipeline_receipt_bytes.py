"""GH #1152: `pipeline receipt <run>` is byte-identical to the archived receipt.

The projection wrote keys in insertion order while the archive was sorted, and
the journal's `started` (the core's `pass_planned`) and a round's start (the
admission's `run_dispatched`) differed from the executor's own clock. One
serializer (`receipt.dumps`) and one start (handed to the executor in the
dispatch) make the two documents one.
"""
import contextlib
import io
import os
import unittest

from scripts.pipeline import receipt, store
from scripts.tests.test_pipeline_gate_cli import GateCase
from scripts.tests import store_isolation

def setUpModule():  # noqa: N802 -- unittest's name
    store_isolation.isolate()


def tearDownModule():  # noqa: N802 -- unittest's name
    store_isolation.restore()


class TestReceiptBytes(GateCase):
    def test_the_journal_projection_is_the_archived_receipt_byte_for_byte(self):
        self.python_plan()
        archive = self.tmp / "archive"
        rc, out, err = self.gate("strand", "--no-pre", "--reason", "test", "--archive", str(archive))
        self.assertEqual(rc, 0, out + err)
        archived = (archive / os.readlink(archive / "latest") / "receipt.json").read_bytes()
        db = store.open()
        run = db.execute("SELECT run FROM journal WHERE event = 'pass_planned'").fetchone()[0]
        buf = io.StringIO()
        with contextlib.redirect_stdout(buf):
            self.assertEqual(0, receipt._cmd_receipt(type("A", (), {
                "db": None, "run": run, "legacy": False, "lines": False})()))
        self.assertEqual(archived.decode("utf-8"), buf.getvalue())

    def test_one_serializer_sorted_with_a_final_newline(self):
        self.assertEqual('{\n  "a": 1,\n  "b": 2\n}\n', receipt.dumps({"b": 2, "a": 1}))


if __name__ == "__main__":
    unittest.main()
