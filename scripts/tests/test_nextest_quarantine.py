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
import textwrap
import tomllib
import unittest

REPO = pathlib.Path(__file__).resolve().parents[2]
CONFIG = REPO / ".config" / "nextest.toml"

BLOCK = re.compile(r"(?m)^\[\[profile\.default\.overrides\]\][ \t]*$")
RETRIES = re.compile(r"(?m)^retries\s*=\s*(.+?)\s*$")


def entry_comment(body: str) -> str:
    """The comment lines of ONE entry: the ones ABOVE its first key, VERBATIM.

    A block body reaches to the next `[[`, so it carries whatever stands between
    the entry's last key and that header -- in this file a section head, not an
    entry comment (`LOUD WHEN GREEN` travels inside the #763 block). Reading all
    `#` lines of a body therefore lets a neighbour's `GH #<n>` answer for an
    entry that has none. The format of this file puts the comment above the keys
    ("Format per entry"), so the first key is the end of it.

    The return value is a SLICE of `body`, never a re-joined list of lines. A
    re-join drops an empty line that stood INSIDE the comment, and a caller that
    then searches the file for what it was handed finds nothing: the bite test
    below blinds a number by replacing that string, so its `str.replace` missed
    and the test went red over the formatting alone (measured: red, with a
    19 286-character message). That formatting is the very one OR-K.K5.2 names
    as the reason this boundary sits at the first key, so the reader has to
    survive it. `end` therefore tracks an offset, not a list."""
    end = 0  # how far the comment reaches into `body`, in characters
    pos = 0
    for ln in body.splitlines(keepends=True):
        if ln.startswith("#"):
            end = pos + len(ln)
        elif ln.strip():
            break  # the first key line ends this entry's comment
        pos += len(ln)
    return body[:end]


def retry_count(body: str) -> int:
    """How many retries the entry buys, 0 for none.

    `retries = 0` is not a quarantine debt -- it spells out the default of the
    profile, so it owes no issue. A value this reader cannot read as a plain
    number (the `retries = { count = ... }` table form) is counted as a debt."""
    m = RETRIES.search(body)
    if m is None:
        return 0
    raw = m.group(1)
    return int(raw) if raw.isdigit() else 1


def buys_retries(text: str) -> bool:
    """Whether the file buys retries at all -- read WITHOUT the entry reader.

    This exists so the bite test can tell apart two states that look identical
    from `entry_bodies`: a file with no retry entry left (rule 3 got what it
    wanted, which is a skip) and a reader that stopped finding the entries that
    ARE there (a defect, which is a failure). Asking the reader under test would
    let it answer for the file it is supposed to read. Every `retries` line of
    the file counts, the two profile defaults included -- they say `retries = 0`
    and drop out, and if one of them ever did not, a failure demanding a look is
    the safe direction for a lock."""
    return any(raw.strip() != "0" for raw in RETRIES.findall(text))


def entry_bodies(text: str) -> list[str]:
    """The body of every `[[profile.default.overrides]]` entry, header excluded.

    A body reaches to the next `[[`, which is why `entry_comment` and not this
    function decides what an entry's comment is."""
    return [block.split("\n[[", 1)[0] for block in BLOCK.split(text)[1:]]


def entry_name(body: str) -> str:
    """The binary an entry names -- short enough to put in a failure message.

    A failure that dumps a body dumps up to twenty comment lines with it, and a
    whole file if the assertion happens to compare files; the binary id says
    which entry is meant in one line."""
    m = re.search(r"binary_id\(=([^)]*)\)", body)
    return m.group(1) if m else "<an entry that names no binary_id>"


def retry_entries_missing_an_issue(text: str) -> list[str]:
    """Every `[[profile.default.overrides]]` entry that buys retries without
    naming the issue they are a debt of. Takes the text rather than the file, so
    a fixture can prove what the reader does and does not accept."""
    missing = []
    for body in entry_bodies(text):
        if not retry_count(body):
            continue
        if not re.search(r"GH #\d+", entry_comment(body)):
            missing.append(body.strip())
    return missing

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

    def test_every_retry_entry_names_its_issue(self):
        """Rule 2 of the head comment, held by a test: no issue, no entry.

        `tomllib` drops comments, and the comment is where the debt is named,
        so this reads the text: for every `[[profile.default.overrides]]` block
        that buys retries, the comment lines of that block name a `GH #<n>`.
        A retry nobody tracks is the shape rule 3 exists to retire (GH #804 --
        an entry that lived two runs, then went with its fix)."""
        for entry in retry_entries_missing_an_issue(self.text):
            with self.subTest(entry=entry.splitlines()[-2:]):
                self.fail("a retry entry names the issue it is a debt of")

    def test_a_neighbours_comment_cannot_rescue_an_entry(self):
        """The comment that answers for an entry is the entry's OWN.

        A block body reaches to the next `[[`, so everything between the last
        key and that header travels with the entry above it -- and in this file
        that is a section head, not an entry comment (`LOUD WHEN GREEN` sits in
        the #763 block today). An entry whose own comment lost its number would
        be rescued by the number of a neighbour. The fixture below is exactly
        that shape: one entry with retries and no issue, followed by a foreign
        comment block that carries one."""
        fixture = textwrap.dedent("""\
            [[profile.default.overrides]]
            # one line on why it is flaky -- and no issue behind it
            filter = 'binary_id(=meclaw-cells::a_binary) & test(=a_test)'
            retries = 1

            # ------------------------------------------------------------------
            # A SECTION OF ITS OWN -- its head comment cites GH #4711
            # ------------------------------------------------------------------
            [[profile.default.overrides]]
            filter = 'binary_id(=meclaw-cells::b_binary) & test(=b_test)'
            success-output = "immediate-final"
            """)
        found = retry_entries_missing_an_issue(fixture)
        self.assertEqual(len(found), 1, found)
        self.assertIn("a_binary", found[0])

    def test_an_entry_that_switches_retries_off_is_no_debt(self):
        """`retries = 0` buys nothing, so it owes nothing.

        Rule 2 is about a debt. An override that spells out the default -- no
        retries -- is the opposite of one, and demanding an issue for it would
        make the lock ask for tracking where there is nothing to track. Only a
        positive count counts; a form this reader cannot read as a number (the
        `retries = { count = ... }` table) is taken as a debt, because erring
        towards "names its issue" is the safe direction for a lock."""
        fixture = textwrap.dedent("""\
            [[profile.default.overrides]]
            # this one turns retries off for a binary that must never flake
            filter = 'binary_id(=meclaw-cells::a_binary)'
            retries = 0
            """)
        self.assertEqual(retry_entries_missing_an_issue(fixture), [])

    def test_an_entry_comment_with_an_empty_line_is_still_a_verbatim_cut(self):
        """What the reader hands back has to be findable in the file it read.

        The bite test below blinds a number by replacing the comment it was
        given in the file text, so a comment that comes back RE-JOINED -- as a
        list of `#` lines glued together, without the empty line that stood
        inside it -- makes `str.replace` miss and turns the bite test red for a
        formatting reason. That formatting is the very one OR-K.K5.2 names as
        the reason the boundary sits at the first key ("a comment divided into
        two paragraphs by an empty line"), so the reader has to survive it."""
        fixture = textwrap.dedent("""\
            [[profile.default.overrides]]
            # GH #4711 -- one line on why it is flaky

            # and a second paragraph, below an empty line
            filter = 'binary_id(=meclaw-cells::a_binary)'
            retries = 1
            """)
        body, = entry_bodies(fixture)
        comment = entry_comment(body)
        self.assertIn(comment, fixture)  # verbatim, or `replace` cannot use it
        self.assertIn("GH #4711", comment)
        self.assertIn("second paragraph", comment)
        self.assertNotIn("filter =", comment)

    def test_the_lock_bites_a_real_entry_stripped_of_its_issue(self):
        """The file is green, so the proof of the bite is a copy that is not.

        Take the shipped text, take the issue number out of ONE retry entry's
        comment, and the reader has to name that entry -- otherwise every green
        run above proves only that nothing is being read.

        WHICH entry is deliberately left to the file. Rule 3 of the head comment
        says an entry goes when its issue closes, so a proof nailed to one
        number turns the next paid-off debt into a red suite -- and a red test
        is a hard stop, which is the worst possible reward for tidying up. An
        empty quarantine is the state rule 3 aims at, so it skips rather than
        fails.

        Empty and BROKEN are two different answers, which is why the skip is
        guarded: a reader that has stopped finding the entries that are in the
        file would otherwise be waved through with the sentence that describes
        the state everyone wants, and the one test whose job it is to report a
        lock running dry would name the wrong cause. The guard has a SECOND
        cause and says so, because it fires on the file buying retries while no
        entry does: `buys_retries` counts every `retries` line, the profile
        defaults included (OR-K.K5.5), so an empty quarantine under a positive
        default in a profile head lands here as well -- the house rule broken
        in another place, not a broken reader."""
        # `assertNotEqual` below compares two file texts, and at longMessage=True
        # it puts its own `%s == %s` with safe_repr(short=False) BEFORE the
        # speaking message: the whole file twice, 19 286 characters measured, to
        # say "the replace hit nothing".
        self.longMessage = False
        entry = next((body for body in entry_bodies(self.text)
                      if retry_count(body)
                      and re.search(r"GH #\d+", entry_comment(body))), None)
        if entry is None:
            self.assertFalse(
                buys_retries(self.text),
                "the file still buys retries, but no entry does -- either the "
                "reader stopped finding the entries that are there, or a "
                "`retries` default in a profile head went positive over an "
                "empty quarantine; the profile heads are the shorter look")
            self.skipTest(".config/nextest.toml buys no retries any more, so there "
                          "is no entry to blind -- rule 3 got what it wanted")
        comment = entry_comment(entry)
        blinded = re.sub(r"GH #\d+", "GH", comment)
        stripped = self.text.replace(comment, blinded, 1)
        self.assertNotEqual(stripped, self.text,
                            f"the comment of {entry_name(entry)} is not in the file verbatim")
        found = retry_entries_missing_an_issue(stripped)
        self.assertEqual([entry_name(b) for b in found], [entry_name(entry)])

    def test_the_measured_display_audio_tests_are_the_whole_entry(self):
        """GH #763 quarantines the tests that were MEASURED flaky, not the binary.

        Four of the five tests of that file have now tripped with the same CDP
        message and passed on repetition with nothing edited; each occurrence is
        cited in the entry's comment. The fifth,
        `a_short_press_opens_the_dock_and_a_long_one_speaks`, has never been seen
        flaky and stays strict -- that is what keeps the entry from quietly
        becoming a leash on the whole binary, which is the shape this file exists
        to prevent. A sixth name belongs here only with a measurement beside it."""
        entries = [e for e in self.overrides()
                   if "gh643_audio_in_the_display_window_browser" in e["filter"]]
        self.assertEqual(len(entries), 1)
        expr = entries[0]["filter"]
        self.assertIn(
            "binary_id(=meclaw-cells::gh643_audio_in_the_display_window_browser)", expr)
        self.assertEqual(
            sorted(TEST_TERM.findall(expr)),
            ["=a_browser_holds_the_button_and_the_colony_answers",
             "=a_browser_rejoins_after_the_cell_closed_the_topic",
             "=the_release_drains_before_it_lets_go",
             "=the_ring_sends_what_it_kept_before_the_hold"])
        self.assertNotIn("a_short_press_opens_the_dock_and_a_long_one_speaks", expr)
        self.assertEqual(entries[0]["retries"], 1)


if __name__ == "__main__":
    unittest.main()
