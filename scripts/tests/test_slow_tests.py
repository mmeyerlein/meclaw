"""The slow lock (`scripts/slow_tests.py`, GH #1046).

A test that needs more than a third of its nextest budget is red with its name
and its seconds. Measured behind it: the scenario test of the display hive used
213 s of 240 s at 12 threads, unseen, until 24 threads killed it.
"""

import os
import pathlib
import subprocess
import sys
import tempfile
import textwrap
import tomllib
import unittest

SCRIPTS = pathlib.Path(__file__).resolve().parent.parent
REPO = SCRIPTS.parent
sys.path.insert(0, str(SCRIPTS))

import slow_tests as st  # noqa: E402

CONFIG = textwrap.dedent("""\
    [profile.default]
    slow-timeout = { period = "30s", terminate-after = 8 }

    [profile.ci]
    slow-timeout = { period = "60s", terminate-after = 5 }

    [[profile.default.overrides]]
    filter = 'binary_id(=meclaw-cells::long_one) & (test(=a) + test(=b))'
    slow-timeout = { period = "60s", terminate-after = 10 }

    [[profile.default.overrides]]
    filter = 'binary_id(=meclaw-cells::marks_only)'
    success-output = "immediate-final"
""")


def junit(rows):
    body = "".join('<testcase name="%s" classname="%s" time="%s"/>' % (n, b, t)
                   for b, n, t in rows)
    return ('<?xml version="1.0"?><testsuites><testsuite name="x">%s</testsuite>'
            '</testsuites>' % body)


class SlowLock(unittest.TestCase):
    def run_lock(self, rows, profile="default", config=CONFIG, debt="", mode=None):
        # The gate exports MECLAW_GATE_MODE to every station, gate-selftest
        # included; inherited, it would decide what these tests see.
        env = {k: v for k, v in os.environ.items() if k != "MECLAW_GATE_MODE"}
        if mode:
            env["MECLAW_GATE_MODE"] = mode
        with tempfile.TemporaryDirectory() as d:
            j, c = os.path.join(d, "junit.xml"), os.path.join(d, "nextest.toml")
            o = os.path.join(d, "slow-debt.toml")
            pathlib.Path(j).write_text(junit(rows))
            pathlib.Path(c).write_text(config)
            pathlib.Path(o).write_text(debt)
            out = subprocess.run(
                [sys.executable, str(SCRIPTS / "slow_tests.py"), "--junit", j,
                 "--config", c, "--profile", profile, "--debt", o],
                capture_output=True, text=True, env=env)
        return out.returncode, out.stdout

    def test_a_test_over_a_third_of_its_budget_is_named(self):
        # 90 s at 30 s x 8 = 240 s: over the mark of 80 s.
        rc, out = self.run_lock([("meclaw-cells::slow", "the_slow_one", "90.0"),
                                 ("meclaw-cells::slow", "the_quick_one", "1.0")])
        self.assertEqual(rc, 1, out)
        self.assertIn("meclaw-cells::slow the_slow_one 90.0s > 80s", out)
        self.assertNotIn("the_quick_one", out)

    def test_a_test_under_the_mark_passes(self):
        rc, out = self.run_lock([("meclaw-cells::slow", "the_slow_one", "79.0")])
        self.assertEqual(rc, 0, out)
        self.assertIn("0 of 1 tests over the mark", out)

    def test_the_mark_comes_from_the_active_profile(self):
        # ci: 60 s x 5 = 300 s -> 100 s. The same 90 s is fine there.
        rc, out = self.run_lock([("meclaw-cells::slow", "the_slow_one", "90.0")], "ci")
        self.assertEqual(rc, 0, out)
        rc, out = self.run_lock([("meclaw-cells::slow", "the_slow_one", "101.0")], "ci")
        self.assertEqual(rc, 1, out)
        self.assertIn("> 100s", out)

    def test_an_override_moves_the_mark_for_its_tests_only(self):
        # long_one/a: 60 s x 10 = 600 s -> 200 s; long_one/c stays at 80 s.
        rc, out = self.run_lock([("meclaw-cells::long_one", "a", "150.0"),
                                 ("meclaw-cells::long_one", "c", "150.0"),
                                 ("meclaw-cells::marks_only", "m", "10.0")])
        self.assertEqual(rc, 1, out)
        self.assertIn("long_one c 150.0s > 80s", out)
        self.assertNotIn("long_one a ", out)

    def test_the_default_overrides_reach_another_profile(self):
        # nextest applies the default profile's overrides after the profile's own.
        rc, out = self.run_lock([("meclaw-cells::long_one", "b", "150.0")], "ci")
        self.assertEqual(rc, 0, out)

    def test_no_junit_is_red(self):
        with tempfile.TemporaryDirectory() as d:
            c = os.path.join(d, "nextest.toml")
            pathlib.Path(c).write_text(CONFIG)
            out = subprocess.run(
                [sys.executable, str(SCRIPTS / "slow_tests.py"), "--junit",
                 os.path.join(d, "missing.xml"), "--config", c],
                capture_output=True, text=True)
        self.assertEqual(out.returncode, 2, out.stdout)
        self.assertIn("no JUnit file", out.stdout)

    def test_a_filter_it_cannot_read_is_red(self):
        config = CONFIG + textwrap.dedent("""\
            [[profile.default.overrides]]
            filter = 'kind(test) & package(meclaw)'
            slow-timeout = { period = "60s", terminate-after = 10 }
        """)
        rc, out = self.run_lock([("meclaw-cells::slow", "x", "1.0")], config=config)
        self.assertEqual(rc, 2, out)
        self.assertIn("cannot read the filter", out)

    def test_a_debt_with_an_issue_is_named_and_not_red(self):
        debt = textwrap.dedent("""\
            [[debt]]
            test = "meclaw-cells::slow the_slow_one"
            issue = 4711
            measured = "90 s"
        """)
        rc, out = self.run_lock([("meclaw-cells::slow", "the_slow_one", "90.0"),
                                 ("meclaw-cells::slow", "another", "95.0")], debt=debt)
        self.assertEqual(rc, 1, out)          # `another` owes nothing on file
        self.assertIn("SLOW-DEBT meclaw-cells::slow the_slow_one 90.0s", out)
        self.assertIn("GH #4711", out)
        self.assertIn("SLOW-LOCK meclaw-cells::slow another 95.0s", out)
        rc, out = self.run_lock([("meclaw-cells::slow", "the_slow_one", "90.0")], debt=debt)
        self.assertEqual(rc, 0, out)

    def test_a_paid_debt_says_so(self):
        debt = '[[debt]]\ntest = "meclaw-cells::slow the_slow_one"\nissue = 4711\n'
        rc, out = self.run_lock([("meclaw-cells::slow", "the_slow_one", "10.0")], debt=debt)
        self.assertEqual(rc, 0, out)
        self.assertIn("SLOW-DEBT PAID? meclaw-cells::slow the_slow_one", out)

    def test_a_debt_without_an_issue_is_red(self):
        debt = '[[debt]]\ntest = "meclaw-cells::slow the_slow_one"\n'
        rc, out = self.run_lock([("meclaw-cells::slow", "the_slow_one", "90.0")], debt=debt)
        self.assertEqual(rc, 2, out)
        self.assertIn("needs `test` and an `issue`", out)

    def test_the_verdict_line_counts_what_is_over_and_what_is_owed(self):
        """The last line is the reason of the GATE line (`gate.sh`): the debt
        stands in the summary, not only in the station log (review I4)."""
        debt = '[[debt]]\ntest = "meclaw-cells::slow the_slow_one"\nissue = 4711\n'
        rc, out = self.run_lock([("meclaw-cells::slow", "the_slow_one", "90.0")], debt=debt)
        self.assertEqual(out.splitlines()[-1], "SLOW-VERDICT 0 over 1/3, 1 owed (GH #4711)")
        rc, out = self.run_lock([("meclaw-cells::slow", "the_slow_one", "90.0"),
                                 ("meclaw-cells::slow", "another", "95.0")], debt=debt)
        self.assertEqual(out.splitlines()[-1], "SLOW-VERDICT 1 over 1/3, 1 owed (GH #4711)")
        rc, out = self.run_lock([("meclaw-cells::slow", "the_slow_one", "9.0")])
        self.assertEqual(out.splitlines()[-1], "SLOW-VERDICT 0 over 1/3, none owed")
        rc, out = self.run_lock([("meclaw-cells::slow", "x", "1.0")],
                                debt='[[debt]]\ntest = "a b"\n')
        self.assertEqual(rc, 2, out)
        self.assertTrue(out.splitlines()[-1].startswith("SLOW-VERDICT broken: "), out)

    def test_a_debt_whose_test_never_ran_is_named_in_the_passes(self):
        """A renamed or deleted test would keep its debt line forever, silent
        (review M1). Its binary ran and the test is not in it: in integration
        and release that is red; a strand stays quiet (its filter decides what
        ran)."""
        debt = '[[debt]]\ntest = "meclaw-cells::slow gone"\nissue = 4711\n'
        rows = [("meclaw-cells::slow", "the_quick_one", "1.0")]
        for mode in ("integration", "release"):
            rc, out = self.run_lock(rows, debt=debt, mode=mode)
            self.assertEqual(rc, 1, out)
            self.assertIn("SLOW-DEBT UNKNOWN meclaw-cells::slow gone -- GH #4711", out)
            self.assertEqual(out.splitlines()[-1],
                             "SLOW-VERDICT 0 over 1/3, none owed, 1 unknown debt", out)
        rc, out = self.run_lock(rows, debt=debt, mode="strand")
        self.assertEqual(rc, 0, out)
        self.assertNotIn("UNKNOWN", out)
        # A binary that did not run at all says nothing about its tests.
        rc, out = self.run_lock([("meclaw-cells::other", "x", "1.0")], debt=debt,
                                mode="integration")
        self.assertEqual(rc, 0, out)

    def test_the_repo_owes_no_slow_test(self):
        """GH #1048 paid the list off: every test comes in under a third of its
        budget. A new debt is a decision: edit this lock in the same commit and
        name the issue. Without it any issue number would turn a slow test
        silently green and nothing would notice the list growing (review I4)."""
        owed = st.debts(str(REPO / ".config" / "slow-debt.toml"))
        self.assertEqual(sorted(owed), [])

    def test_the_junit_path_has_one_source(self):
        """`test-tier.sh` deletes the file this lock reads; both ask
        `junit_path`, or a moved store would leave the lock an old file
        (review M2)."""
        out = subprocess.run([sys.executable, str(SCRIPTS / "slow_tests.py"),
                              "--print-junit-path", "--profile", "ci"],
                             capture_output=True, text=True, check=True).stdout
        with open(REPO / ".config" / "nextest.toml", "rb") as f:
            config = tomllib.load(f)
        self.assertEqual(out.strip(), st.junit_path(config, "ci"))
        tier = (SCRIPTS / "test-tier.sh").read_text()
        self.assertIn('slow_tests.py" --print-junit-path', tier)
        self.assertNotIn("target/nextest/", tier)

    def test_every_debt_of_the_repo_has_an_issue(self):
        st.debts(str(REPO / ".config" / "slow-debt.toml"))

    def test_every_override_of_the_repo_parses(self):
        """The real file: every slow-timeout override is inside the subset."""
        with open(REPO / ".config" / "nextest.toml", "rb") as f:
            config = tomllib.load(f)
        for profile in config["profile"]:
            st.budgets(config, profile)
        self.assertIn("junit", config["profile"]["default"],
                      "the default profile writes no JUnit file -- the lock reads it")

    def test_the_junit_is_looked_for_in_the_store(self):
        """nextest writes the JUnit file into its store under the workspace root,
        not into CARGO_TARGET_DIR (first gate of GH #1046 looked in the wrong place)."""
        path = st.junit_path({"profile": {"default": {"junit": {"path": "junit.xml"}}}},
                             "default")
        self.assertEqual(path, os.path.join(st.ROOT, "target", "nextest", "default",
                                            "junit.xml"))
        moved = st.junit_path({"store": {"dir": "elsewhere"}}, "ci")
        self.assertEqual(moved, os.path.join(st.ROOT, "elsewhere", "ci", "junit.xml"))

    def test_the_filter_subset(self):
        f = st.compile_filter("binary_id(=a::b) & (test(=x) + test(~yz)) | not test(/^q/)")
        self.assertTrue(f("a::b", "x"))
        self.assertTrue(f("a::b", "ayzb"))
        self.assertFalse(f("a::b", "qq"))
        self.assertTrue(f("other", "zz"))   # the `not test(/^q/)` arm


if __name__ == "__main__":
    unittest.main()
