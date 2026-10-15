"""GH #1150: a RED gate line names the first finding and the log path."""
import os
import tempfile
import unittest

from scripts.pipeline import executor, finding
from scripts.tests import store_isolation


class _LogRoot(unittest.TestCase):
    """Review R3 M6: one root per test class, created and removed with the class.

    A module-wide root lived from import to module teardown; in a full discover run it was
    gone before the tests ran (FileNotFoundError, gate run-984b139bec57). The class owns it.
    """

    @classmethod
    def setUpClass(cls):  # noqa: N802 -- unittest's name
        super().setUpClass()
        root = tempfile.TemporaryDirectory(prefix="finding-")
        cls.addClassCleanup(root.cleanup)
        cls.root = root.name

    def _log(self, text):
        return _log(self.root, text)


def _log(root, text):
    d = tempfile.mkdtemp(dir=root)
    os.makedirs(os.path.join(d, "logs"))
    p = os.path.join(d, "logs", "st.log")
    with open(p, "w", encoding="utf-8") as fh:
        fh.write(text)
    return p

def setUpModule():  # noqa: N802 -- unittest's name
    store_isolation.isolate()


def tearDownModule():  # noqa: N802 -- unittest's name
    store_isolation.restore()


class TestFirstFinding(_LogRoot):
    def test_unittest_failure_names_the_test(self):
        p = self._log("....\nFAIL: test_a (scripts.tests.test_x.Case.test_a)\nTraceback\n"
                 '  File "scripts/tests/test_x.py", line 9, in test_a\n')
        self.assertEqual(finding.first_finding(p), "scripts.tests.test_x.Case.test_a")

    def test_nextest_failure_names_the_binary_and_test(self):
        p = self._log("        PASS [   0.1s] meclaw::a ok\n        FAIL [   2.3s] meclaw::gate some::test\n")
        self.assertEqual(finding.first_finding(p), "meclaw::gate some::test")

    def test_nextest_failure_with_a_progress_counter_names_the_test(self):
        """#1164 (Z M-INT-F): nextest prints `( 3232/10378)` before the binary; the
        label read `at ( 3232/10378)` and skipped `(    1/10378)` (padded) entirely."""
        p = self._log("        FAIL [  65.4s] (    1/10378) meclaw-cells::round_2 gh906::a\n"
                      "        FAIL [   6.8s] ( 3232/10378) meclaw-cells::gh1001 gh1001_cost\n")
        self.assertEqual(finding.first_finding(p), "meclaw-cells::round_2 gh906::a")

    def test_a_counter_without_a_name_is_no_finding(self):
        """#1164 re-check: a cut line `( 3232/10378)` with no binary and test after
        it must not become the label; the next real FAIL line names the place."""
        p = self._log("        FAIL [   6.8s] ( 3232/10378)\n"
                      "        FAIL [   4.7s] ( 6251/10378) meclaw-cells::misc_8 seed::corpus\n")
        self.assertEqual(finding.first_finding(p), "meclaw-cells::misc_8 seed::corpus")

    def test_rustc_error_names_path_line(self):
        p = self._log("error[E0308]: mismatched types\n  --> crates/x/src/lib.rs:12:5\n")
        self.assertEqual(finding.first_finding(p), "crates/x/src/lib.rs:12:5")

    def test_traceback_names_the_failing_frame(self):
        """Review R3 M3: the innermost frame of the traceback, not the outermost."""
        p = self._log("Traceback (most recent call last):\n"
                 '  File "scripts/outer.py", line 3, in <module>\n    main()\n'
                 '  File "scripts/inner.py", line 9, in main\n    boom()\n'
                 "ValueError: x\n"
                 'later: File "scripts/other.py", line 1\n')
        self.assertEqual(finding.first_finding(p), "scripts/inner.py:9")

    def test_rustc_error_beats_an_earlier_warning(self):
        """Review R3 M3: a warning's `-->` never outranks a later error's."""
        p = self._log("warning: unused import\n  --> crates/a/src/lib.rs:1:5\n"
                 "error[E0308]: mismatched types\n  --> crates/b/src/lib.rs:7:9\n")
        self.assertEqual(finding.first_finding(p), "crates/b/src/lib.rs:7:9")

    def test_drift_names_the_changed_file_and_hunk(self):
        p = self._log("  DRIFT   templates/lib\n          CHANGED   templates/lib/template.json\n"
                 "          @@ -24 +24 @@\n")
        self.assertEqual(finding.first_finding(p), "templates/lib/template.json:24")

    def test_self_test_pin(self):
        p = self._log("    PASS a\nFAIL some_pin   <<< AssertionError: x\n")
        self.assertEqual(finding.first_finding(p), "some_pin")

    def test_logs_live_under_the_class_root(self):
        """Review R3 M6: every log of this class goes away with the class."""
        self.assertTrue(self._log("x").startswith(self.root + os.sep))

    def test_red_reason_always_carries_the_log(self):
        p = self._log("nothing to point at\n")
        self.assertEqual(finding.red_reason("", p), "log logs/st.log")
        self.assertEqual(finding.red_reason("timeout", p), "timeout -- log logs/st.log")


class TestExecutorRedLine(_LogRoot):
    def test_the_executor_writes_the_finding_into_a_red_line(self):
        p = self._log("FAIL: test_a (scripts.tests.test_x.Case.test_a)\n")
        ex = executor._Run.__new__(executor._Run)
        ex.mode, ex.aborted = "strand", None
        verdict, reason, _ = ex._judge({"name": "st"}, 1, False, p, None)
        self.assertEqual(verdict, "RED")
        self.assertIn("at scripts.tests.test_x.Case.test_a -- log logs/st.log", reason)


if __name__ == "__main__":
    unittest.main()
