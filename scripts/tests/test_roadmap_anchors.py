"""`scripts/check_roadmap_anchors.py` against a roadmap written for the test.

The gate had no test file of its own -- the one doc gate without one -- and
that is how a paragraph under a horizon passed it for five commits (GH #774):
`parse_roadmap` collected bullets and dropped everything else without a word.
"""

import importlib.util
import pathlib
import sys
import tempfile
import unittest
from unittest import mock
from scripts.tests import store_isolation

REPO = pathlib.Path(__file__).resolve().parents[2]
SCRIPT = REPO / "scripts" / "check_roadmap_anchors.py"


def load_script():
    spec = importlib.util.spec_from_file_location("check_roadmap_anchors", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    written, sys.dont_write_bytecode = sys.dont_write_bytecode, True
    try:
        spec.loader.exec_module(module)
    finally:
        sys.dont_write_bytecode = written
    return module


ISSUE = "https://github.com/example/repo/issues/1"

CLEAN = f"""# Roadmap

A preamble above the first heading is not read.

## Now

- One entry, anchored: [#1]({ISSUE}).
  Its continuation line is part of it.

## Next

- Another, by register. (register: some-id)

## Shipped

Prose here is fine: Shipped is not a stream.
"""

STRAY = CLEAN.replace("## Now\n\n", "## Now\n\nBetween waves. Nothing to see here.\n\n")


class RoadmapAnchorsTest(unittest.TestCase):
    def setUp(self):
        self.mod = load_script()
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.path = pathlib.Path(self._tmp.name) / "ROADMAP.md"

    def roadmap(self, text):
        self.path.write_text(text, encoding="utf-8")
        return self.path

    def test_a_clean_roadmap_has_two_bullets_and_no_stray(self):
        p = self.roadmap(CLEAN)
        self.assertEqual([b.stream for b in self.mod.parse_roadmap(p)], ["now", "next"])
        self.assertEqual(self.mod.stray_lines(p), [])

    def test_a_paragraph_under_a_horizon_is_a_stray(self):
        strays = self.mod.stray_lines(self.roadmap(STRAY))
        self.assertEqual([(s.stream, s.line_no) for s in strays], [("now", 7)])
        self.assertIn("Between waves", strays[0].text)

    def test_a_continuation_and_a_blank_line_are_not_strays(self):
        text = CLEAN.replace(
            "- Another", "- Another\n  still the same bullet\n\n- Third [#1](%s)" % ISSUE)
        self.assertEqual(self.mod.stray_lines(self.roadmap(text)), [])
        self.assertEqual(len(self.mod.parse_roadmap(self.path)), 3)

    def test_an_indented_line_after_a_blank_is_a_stray(self):
        # The whole bullet line is replaced, not its head: `- Another` is a
        # prefix of it, and a partial replacement would fold the rest of the
        # bullet into the stray (OR-K.K2.2).
        text = CLEAN.replace("- Another, by register. (register: some-id)",
                             "- Another, by register. (register: some-id)\n\n  orphaned indent")
        strays = self.mod.stray_lines(self.roadmap(text))
        self.assertEqual([s.text for s in strays], ["orphaned indent"])


class _Reply:
    """A stand-in for `urlopen`'s response: `body` as the API sent it."""

    def __init__(self, body):
        self.body = body

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        return False

    def read(self):
        return self.body


class OpenIssuesTest(unittest.TestCase):
    """GH #1294: without `gh` and without auth, `?state=open` answered `[]` and
    every anchor -- #618, open on the tracker -- was "not open": an empty or
    malformed answer is an unreachable tracker (a counted skip), never the
    truth. An answer with content judges as before."""

    def setUp(self):
        self.mod = load_script()
        for patch in (mock.patch.object(self.mod.shutil, "which", return_value=None),
                      mock.patch.dict(self.mod.os.environ, {}, clear=True)):
            patch.start()
            self.addCleanup(patch.stop)

    def numbers(self, body):
        with mock.patch.object(self.mod.urllib.request, "urlopen",
                               return_value=_Reply(body)):
            return self.mod.open_issue_numbers("example/repo")

    def test_an_empty_answer_is_unreachable_not_all_closed(self):
        nums, how = self.numbers(b"[]")
        self.assertIsNone(nums, how)
        self.assertIn("no open issue", how)

    def test_a_malformed_answer_is_unreachable(self):
        for body in (b'{"message": "Bad credentials"}', b"[1, 2]"):
            nums, how = self.numbers(body)
            self.assertIsNone(nums, (body, how))

    def test_an_answer_with_content_judges(self):
        nums, how = self.numbers(b'[{"number": 618}, {"number": 7, "pull_request": {}}]')
        self.assertEqual({618}, nums)
        self.assertEqual("api.github.com", how)


def setUpModule():
    store_isolation.isolate()


def tearDownModule():
    store_isolation.restore()


if __name__ == "__main__":
    unittest.main()
