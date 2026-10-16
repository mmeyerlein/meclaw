"""Kit self-tests that read the private tree -- the one mechanism (GH #1199).

The kit self-tests (`gate-selftest`) travel with the export and run in the
public CI, in a tree that lacks `plans/`, `workshop/`, `AGENTS.md`, most of
`docs/` and every test the export blocklists. A test that reads such a file
was green here and red there: 0.64.0's public CI, 23 red tests and one shard
that died at import (`test_lane_browser_deps` read `workshop/tools/package.json`
at module level).

Two answers, and only two:

* the test carries its input (a fixture under `scripts/tests/fixtures/`, listed
  in `ROOT_FILES` of the export) -- then it runs in both trees; or
* it names what it reads from the private tree through `require(...)` (or the
  `private` decorator) at the point of the read.

`require` decides by the EXPORT'S OWN DEFINITION, not by a second list: the
private tree is the one that carries `MARKER`, the export script itself, which
lives under `plans/` -- a root the export never carries (`FORBIDDEN_PREFIX`,
R2). So:

* private tree (MARKER present): `require` never skips. A missing input is an
  AssertionError -- a private-tree test can not go quiet in the tree it was
  written for (pinned in `test_private_tree.py`).
* export tree (no MARKER): a missing input skips the test with the paths named;
  a present one runs as written, so a declaration never hides a file that does
  travel.

The export side closes the loop: the release gate runs the kit self-test
stations IN the export tree (`export_tree_tests.py --kit`), so a test that
reads a non-exported file WITHOUT this declaration turns the release red
before the public CI sees it.
"""
import functools
import pathlib
import unittest

REPO = pathlib.Path(__file__).resolve().parents[2]

# The export definition: present in the private tree, never in the export
# (it lives under plans/, which R2 forbids). No second list of private paths.
MARKER = "plans/export-fixtures/make_export.py"

# `private()` without a path: the test's verdict is about the whole private
# tree (a sweep over every file, the release plan of the full catalogue), not
# about one file it reads.
WHOLE_TREE = MARKER


def is_private(repo=None):
    """True in the private tree: it carries the export definition."""
    return (pathlib.Path(repo or REPO) / MARKER).is_file()


def require(*paths, repo=None):
    """Make sure every path (repo-relative) exists before the test reads it.

    Private tree: a missing path fails the test (AssertionError), never a skip.
    Export tree: a missing path skips the test (`unittest.SkipTest`) and names
    the paths; when they all exist the call returns and the test runs."""
    root = pathlib.Path(repo or REPO)
    paths = paths or (WHOLE_TREE,)
    missing = [p for p in paths if not (root / p).exists()]
    if not missing:
        return
    if is_private(root):
        raise AssertionError(
            "private-tree input missing in the private tree (GH #1199): %s"
            % ", ".join(missing))
    raise unittest.SkipTest(
        "private tree only (GH #1199): %s does not travel with the export"
        % ", ".join(missing))


def private(*paths):
    """Decorator form of `require` for a test method or a TestCase class.

    On a class every `test*` method is wrapped, so the check runs inside the
    test (a skip or a failure of that test), never at import. A `setUpClass`
    of the class itself checks first too: it is where a class loads the
    private input it reads (an import from `workshop/`, the export script)."""
    def wrap(fn):
        @functools.wraps(fn)
        def inner(*args, **kwargs):
            require(*paths)
            return fn(*args, **kwargs)
        return inner

    def deco(obj):
        if isinstance(obj, type):
            for name in list(vars(obj)):
                if name.startswith("test") and callable(getattr(obj, name)):
                    setattr(obj, name, wrap(getattr(obj, name)))
            set_up = vars(obj).get("setUpClass")
            if isinstance(set_up, classmethod):
                obj.setUpClass = classmethod(wrap(set_up.__func__))
            return obj
        return wrap(obj)
    return deco
