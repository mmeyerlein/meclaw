"""GH #1199 (v2-r3-mergegate): the merge candidate plans what its strand planned.

Measured on KR (05214ae, 16.10.): the merge-candidate gate took 894 s against
293 s for the strand gate of the same diff -- `tests` 852 s, `browser:display`
303 s. The plans had the same 22 stations; the merge point had regenerated the
librarian corpus (`templates/builder-librarian/store/seed/docs.jsonl`), which
the strand leaves stale on purpose and the strand's `corpus` station rewrites in
the run's tree before anything reads it. The strand keyed the stale blob, the
merge the regenerated one: every test part with `dir:templates` in its closure
(163 of 163) and every browser part (19 of 19) missed, and the selection grew
by the corpus readers, which moved the filter digest of every part as well.

The pins: for a strand whose diff moves a corpus source, the plan carries the
corpus the run will test (`derived`), the resolver sees the same file list as
for the merge candidate, and the cache keys of both trees are the same.
"""

import os
import pathlib
import subprocess
import sys
import tempfile
import unittest

HERE = pathlib.Path(__file__).resolve().parent
SCRIPTS = HERE.parent
sys.path.insert(0, str(SCRIPTS))

from scripts import gate_plan as gp  # noqa: E402
from scripts.pipeline import cache, plan as pl  # noqa: E402
from scripts.tests import store_isolation  # noqa: E402

FINGERPRINT = {"rustc": "rustc 1.80.0", "cargo": "cargo 1.80.0", "nextest": "0.9.143",
               "node": "v20.0.0", "playwright": "1.40.0", "linker": "cc 13",
               "python": "3.12.0", "cargo_config": "e3b0c44298fc1c14"}
META = {"packages": [], "workspace_members": []}

# A generator with the real tool's interface (`--out`, else the committed path):
# the corpus is a pure function of README.md, as the real one is of its sources.
TOOL = '''import argparse, os
CORE = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
OUT = os.path.join(CORE, "templates/builder-librarian/store/seed/docs.jsonl")
ap = argparse.ArgumentParser()
ap.add_argument("--check", action="store_true")
ap.add_argument("--out", default=None)
a = ap.parse_args()
with open(os.path.join(CORE, "README.md"), encoding="utf-8") as fh:
    content = "corpus of " + fh.read()
if a.check:
    with open(OUT, encoding="utf-8") as fh:
        raise SystemExit(0 if fh.read() == content else 1)
with open(a.out or OUT, "w", encoding="utf-8", newline="") as fh:
    fh.write(content)
'''


def _git(root, *args):
    return subprocess.run(
        ["git", "-c", "user.name=fixture", "-c", "user.email=fixture@example.invalid",
         "-c", "commit.gpgsign=false", "-c", "core.hooksPath=/dev/null",
         "-c", "init.defaultBranch=master", *args],
        cwd=root, check=True, capture_output=True, text=True).stdout.strip()


def _write(root, rel, text):
    path = pathlib.Path(root, rel)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def _commit(root, msg):
    _git(root, "add", "-A")
    _git(root, "commit", "-q", "-m", msg)
    return _git(root, "rev-parse", "HEAD")


class MergeGateParity(unittest.TestCase):
    """A base, the strand on it (README moved, corpus stale) and the merge
    candidate (the strand plus the corpus the merge point regenerates)."""

    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root = os.path.join(tmp.name, "tree")
        os.makedirs(self.root)
        _git(self.root, "init", "-q")
        _write(self.root, "README.md", "one\n")
        _write(self.root, pl.CORPUS_TOOL, TOOL)
        _write(self.root, pl.CORPUS, "corpus of one\n")
        _write(self.root, "templates/shelf/template.json", "{}\n")
        self.base = _commit(self.root, "base")
        _write(self.root, "README.md", "two\n")
        self.strand = _commit(self.root, "strand: a corpus source moved, the corpus is stale")
        _write(self.root, pl.CORPUS, "corpus of two\n")
        self.merge = _commit(self.root, "merge point: the corpus regenerated")
        self.fresh = _git(self.root, "rev-parse", "HEAD:" + pl.CORPUS)

    def at(self, rev):
        _git(self.root, "checkout", "-q", rev)

    def resolver_files(self, rev):
        """The plan of `rev` and the file list its resolver saw."""
        self.at(rev)
        seen = []

        def resolver(repo_root, mode, files):
            seen.append(sorted(files))
            return gp.to_json(files, mode, repo_root, explain=True)
        doc = pl.build_plan(self.root, "strand", self.base, resolver=resolver, metadata=META)
        return doc, seen[0]

    def keys(self, doc):
        station = {"name": "catalogue", "scope": "s", "cargo": False, "run": True,
                   "cmds": [{"argv": ["python3", "scripts/check_catalogue.py"], "cwd": ""}],
                   "inputs": ["dir:templates"], "env": "none"}
        probe = dict(doc, stations=[station])
        return {k: v["key"] for k, v in
                cache.keys_for_plan(probe, self.root, FINGERPRINT, "lane-a", metadata=META).items()}

    def test_the_strand_plans_the_corpus_its_run_writes(self):
        doc, _files = self.resolver_files(self.strand)
        self.assertEqual({pl.CORPUS: self.fresh}, doc.get("derived"))
        self.assertFalse(_git(self.root, "status", "--porcelain"),
                         "deriving the corpus writes nothing into the tree")

    def test_the_merge_candidate_derives_nothing_it_already_holds(self):
        doc, _files = self.resolver_files(self.merge)
        self.assertNotIn("derived", doc)

    def test_the_resolver_sees_the_same_files_for_strand_and_merge(self):
        _doc_s, strand = self.resolver_files(self.strand)
        _doc_m, merge = self.resolver_files(self.merge)
        self.assertIn(pl.CORPUS, merge)
        self.assertEqual(merge, strand)

    def test_the_strand_keys_are_the_merge_candidate_keys(self):
        doc_s, _ = self.resolver_files(self.strand)
        strand = self.keys(doc_s)
        doc_m, _ = self.resolver_files(self.merge)
        merge = self.keys(doc_m)
        self.assertEqual({("catalogue", "")}, set(merge))
        self.assertEqual(merge, strand)

    def test_a_cut_corpus_station_derives_nothing(self):
        """`--only` without `corpus`: the run never rewrites the corpus, so the
        keys stay on the tree as it is."""
        self.at(self.strand)
        doc = pl.build_plan(self.root, "strand", self.base, only=["catalogue"],
                            resolver=lambda r, m, f: gp.to_json(f, m, r, explain=True),
                            metadata=META)
        self.assertNotIn("derived", doc)

    def test_the_consumers_wait_for_the_corpus_they_read(self):
        """The `corpus` station rewrites the corpus in the run's tree; a test
        that starts beside it reads either blob. With a derived corpus every
        build consumer runs after it."""
        self.at(self.strand)

        def resolver(repo_root, mode, files):
            doc = gp.to_json(files, mode, repo_root, explain=True)
            doc["stations"].append({"name": "scenarios:builder", "scope": "18 cases",
                                    "cargo": True, "cmd": ["python3", "x.py"],
                                    "reason": ["probe"]})
            return doc
        doc = pl.build_plan(self.root, "strand", self.base, resolver=resolver, metadata=META)
        by = {s["name"]: s for s in doc["stations"]}
        self.assertIn("corpus", by)
        self.assertIn("corpus", by["scenarios:builder"]["after"])


def setUpModule():
    store_isolation.isolate()


def tearDownModule():
    store_isolation.restore()


if __name__ == "__main__":
    unittest.main()
