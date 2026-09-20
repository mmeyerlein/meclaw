"""The quarantine entries of `.config/nextest.toml` against nextest's own filter grammar.

A retry entry is a debt with an issue behind it. An entry whose filterset matches
no test is worse than no entry: it parses, so the `tests` station stays green over
it, and nothing ever says that the debt is not being paid.

The grammar rule pinned here is nextest's, not this house's: `test(<matcher>)`
compares the matcher against the TEST NAME alone
(`nextest-filtering/src/expression.rs`: `Self::Test(matcher, _) =>
matcher.is_match(query.test_name)`), while the binary is a separate term,
`binary_id(<matcher>)`. A test name never contains `::` and never contains a
space, so `test(=<crate>::<binary> <test>)` -- the spelling `cargo nextest list`
PRINTS -- can equal no test name at all. One such entry shipped in this file and
did nothing for a full wave (GH #763, wave P review C1).

Measurement behind it: the entry was written from the `nextest list` output and
never held against `nextest list -E '<filter>'`, so nothing showed that both of
its arms were dead.
"""

import pathlib
import re
import tomllib
import unittest

REPO = pathlib.Path(__file__).resolve().parents[2]
CONFIG = REPO / ".config" / "nextest.toml"

# `test(=foo)`, `test(~foo)`, `test(/re/)` -- the matcher of every `test()` term,
# in the entries AND in the comments, because a comment that teaches the wrong
# spelling writes the next dead entry.
TEST_TERM = re.compile(r"test\(\s*([^)]*?)\s*\)")


class TheFilterGrammar(unittest.TestCase):
    def setUp(self):
        self.text = CONFIG.read_text(encoding="utf-8")
        self.config = tomllib.loads(self.text)

    def overrides(self):
        return self.config["profile"]["default"]["overrides"]

    def test_no_test_term_carries_a_binary(self):
        """`test()` sees the test name only -- never `<crate>::<binary> <test>`.

        Checked over the whole file, comments included: the example in the head
        comment is what the next entry is copied from."""
        for matcher in TEST_TERM.findall(self.text):
            if "<" in matcher:
                continue  # a placeholder in the prose, including the one it warns against
            with self.subTest(matcher=matcher):
                self.assertNotIn("::", matcher,
                                 "a test name has no `::` -- that is `binary_id()`")
                self.assertFalse(any(c.isspace() for c in matcher),
                                 "a test name has no space -- that is `binary_id()`")

    def test_every_retry_entry_names_its_binary(self):
        """A quarantine entry belongs to one binary. Naming it keeps the entry
        from reaching a same-named test in another binary -- and it is the only
        term that can carry `<crate>::<binary>`."""
        for entry in self.overrides():
            if "retries" not in entry:
                continue
            with self.subTest(filter=entry["filter"]):
                self.assertIn("binary_id(=", entry["filter"])

    def test_the_two_measured_display_audio_tests_are_the_whole_entry(self):
        """GH #763 quarantines the two tests that were measured flaky, not the
        binary: the other three tests of that file have never been seen flaky."""
        entries = [e for e in self.overrides()
                   if "gh643_audio_in_the_display_window_browser" in e["filter"]]
        self.assertEqual(len(entries), 1)
        expr = entries[0]["filter"]
        self.assertIn(
            "binary_id(=meclaw-cells::gh643_audio_in_the_display_window_browser)", expr)
        self.assertEqual(
            sorted(TEST_TERM.findall(expr)),
            ["=a_browser_holds_the_button_and_the_colony_answers",
             "=the_release_drains_before_it_lets_go"])
        self.assertEqual(entries[0]["retries"], 1)


if __name__ == "__main__":
    unittest.main()
