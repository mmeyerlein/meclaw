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

import json
import os
import pathlib
import subprocess
import sys
import tempfile
import types
import unittest

HERE = pathlib.Path(__file__).resolve().parent
SCRIPTS = HERE.parent
sys.path.insert(0, str(SCRIPTS))

from scripts import gate_plan as gp  # noqa: E402
from scripts.pipeline import cache, merge, plan as pl, schedule  # noqa: E402
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


class APythonOnlyStrandPlansNoBuildAtTheMerge(unittest.TestCase):
    """OR-TM-8 (K15, pin d): a strand that moves a module below a directory no
    `import` reaches (`workshop/tools/llm-conformance/`) plans no `build` and no
    browser station -- and its merge candidate, the `--no-ff` commit the merge
    point builds, plans exactly the same stations."""

    READER = ('#[test]\nfn a() {\n    let t = std::path::Path::new("workshop/tools");\n'
              '    let _ = std::process::Command::new("python3").env("PYTHONPATH", t);\n}\n')
    CARGO_ONLY = {"build", "build:browser", "browser:display", "browser:alone", "tests"}
    IMPORTS = "gh661_the_briefing_states_the_operator_set_rule"

    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root = os.path.join(tmp.name, "tree")
        os.makedirs(self.root)
        _git(self.root, "init", "-q")
        _write(self.root, "README.md", "one\n")
        _write(self.root, pl.CORPUS_TOOL, TOOL)
        _write(self.root, pl.CORPUS, "corpus of one\n")
        _write(self.root, "crates/x/Cargo.toml", '[package]\nname = "x"\nversion = "0.1.0"\n')
        # the readers under the names of the real ones: RUNTIME_DIR_READS_ONLY
        # narrows a NAMED reader only (gh661 the importable modules, gh1209 the
        # drivers), an unnamed one reads every file below (fix round stapel)
        _write(self.root, "crates/x/tests/misc_5/main.rs", "mod %s;\n" % self.IMPORTS)
        _write(self.root, "crates/x/tests/misc_5/%s.rs" % self.IMPORTS, self.READER)
        _write(self.root, "crates/x/tests/gh1209_no_stage_draws_a_filter_browser.rs",
               self.READER.replace('process::Command::new("python3").env("PYTHONPATH", t)',
                                   "fs::read_dir(t)"))
        _write(self.root, "workshop/tools/seed.py", "X = 1\n")
        _write(self.root, "workshop/tools/llm-conformance/fake_provider.py", "Z = 1\n")
        self.base = _commit(self.root, "base")
        _git(self.root, "checkout", "-q", "-b", "strand")
        _write(self.root, "workshop/tools/llm-conformance/fake_provider.py", "Z = 2\n")
        self.strand = _commit(self.root, "strand: the fake provider moved")
        _git(self.root, "checkout", "-q", "master")
        _git(self.root, "merge", "-q", "--no-ff", "-m", "Merge strand", "strand")
        self.merge = _git(self.root, "rev-parse", "HEAD")

    def names(self, rev, mode):
        _git(self.root, "checkout", "-q", rev)
        doc = pl.build_plan(self.root, mode, self.base, metadata=META)
        return [s["name"] for s in doc["stations"]]

    def test_the_strand_and_its_merge_candidate_plan_the_same_stations(self):
        strand = self.names(self.strand, "strand")
        merge = self.names(self.merge, "merge")
        self.assertIn("llm-conformance", strand)
        self.assertFalse(self.CARGO_ONLY & set(strand), strand)
        self.assertEqual(strand, merge)

    def test_the_root_module_beside_it_still_plans_its_readers(self):
        # the other half of the rule: a module the import reaches plans as before
        _git(self.root, "checkout", "-q", "-b", "strand2", self.base)
        _write(self.root, "workshop/tools/seed.py", "X = 2\n")
        rev = _commit(self.root, "strand: the seed module moved")
        self.assertIn("tests", self.names(rev, "strand"))


class ADocsOnlyMergeCandidatePlansTheStrandPlan(unittest.TestCase):
    """OR-TM-9 (K16, 16.10.): the merge gate of K-APPID (4bb6d28c1: AGENTS.md
    and two files under plans/) ran 23/23 stations in 1739 s after 875 s of
    admission. The strand receipt covered the tip, so the gate planned from the
    tip (`merge.gate_base`, GH #1167 b): tip..candidate was master's move since
    the strand base (74 commits, 132 files of code) -- or, with master unmoved,
    nothing, which the resolver floors to `build` + t0. A strand whose gate
    planned no cargo station merges on its own diff: no cargo and no lane
    station, the plan of its strand gate. A code strand keeps the tip."""

    LANE_ONLY = {"build", "build:browser", "browser:display", "browser:alone", "tests",
                 "slow-tests", "clippy", "doctests"}
    # A wave of its own, no real one: a test naming a real wave directory or one
    # of its report files reads them (`turnaround._needles`), and every report
    # commit after a strand gate of that wave would then count as code (H5 merge
    # refused, 17.10.).
    WAVE = "plans/welle-fixture-2026-01-01"

    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.tmp = tmp.name
        self.root = os.path.join(tmp.name, "tree")
        os.makedirs(self.root)
        _git(self.root, "init", "-q")
        _write(self.root, "README.md", "one\n")
        _write(self.root, pl.CORPUS_TOOL, TOOL)
        _write(self.root, pl.CORPUS, "corpus of one\n")
        _write(self.root, "AGENTS.md", "# rules\n")
        _write(self.root, "crates/x/Cargo.toml", '[package]\nname = "x"\nversion = "0.1.0"\n')
        _write(self.root, "crates/x/src/lib.rs", "pub fn a() {}\n")
        _write(self.root, self.WAVE + "/README.md", "the wave\n")
        self.base = _commit(self.root, "base")

    def strand(self, files):
        _git(self.root, "checkout", "-q", "-b", "welle-fixture/ZZ-PARITY", self.base)
        for rel, text in files.items():
            _write(self.root, rel, text)
        return _commit(self.root, "strand")

    def docs_only(self):
        return self.strand({"AGENTS.md": "# rules\n15. every LLM request sends an app id\n",
                            self.WAVE + "/berichte/ZZ-PARITY.md": "# ZZ-PARITY\n",
                            self.WAVE + "/straenge/ZZ-PARITY.json": '{"strand": "ZZ-PARITY"}\n'})

    def candidate(self, master_moves):
        _git(self.root, "checkout", "-q", "master")
        if master_moves:
            _write(self.root, "crates/x/src/b.rs", "pub fn b() {}\n")
            _commit(self.root, "master moved: another strand's code")
        master = _git(self.root, "rev-parse", "HEAD")
        _git(self.root, "merge", "-q", "--no-ff", "-m", "Merge ZZ-PARITY", "welle-fixture/ZZ-PARITY")
        return master, _git(self.root, "rev-parse", "HEAD")

    def plan(self, rev, mode, base):
        _git(self.root, "checkout", "-q", rev)
        return pl.build_plan(self.root, mode, base, metadata=META)

    def gate_base(self, tip, master, strand_plan):
        """`merge.gate_base` with the receipt `preconditions` returns for a
        full strand gate on a clean tree, its `plan.json` beside it."""
        run = os.path.join(self.tmp, "receipts", "run-1")
        os.makedirs(run, exist_ok=True)
        with open(os.path.join(run, "plan.json"), "w", encoding="utf-8") as fh:
            json.dump(strand_plan, fh)
        s = types.SimpleNamespace(name="ZZ-PARITY", branch="welle-fixture/ZZ-PARITY")
        rec = {"path": run, "rev": tip, "line": "GATE-SUMMARY strand GREEN", "run": "run-1",
               "no_pre": None, "base": self.base, "partial": None}
        return merge.gate_base(self.root, master, [s], {"receipts": {"ZZ-PARITY": rec}})

    def test_a_docs_only_merge_candidate_plans_no_cargo_and_no_lane_station(self):
        tip = self.docs_only()
        strand = self.plan(tip, "strand", self.base)
        names = [s["name"] for s in strand["stations"]]
        self.assertFalse(self.LANE_ONLY & set(names), names)
        for master_moves in (True, False):
            with self.subTest(master_moves=master_moves):
                _git(self.root, "checkout", "-q", "master")
                _git(self.root, "reset", "-q", "--hard", self.base)
                master, cand = self.candidate(master_moves)
                base, why = self.gate_base(tip, master, strand)
                doc = self.plan(cand, "merge", base)
                merged = [s["name"] for s in doc["stations"]]
                self.assertFalse([s["name"] for s in doc["stations"]
                                  if schedule._needs(s)["cargo"] >= 1], why)
                self.assertFalse(self.LANE_ONLY & set(merged), "%s: %s" % (why, merged))
                self.assertEqual(names, merged, why)

    def test_a_code_strand_keeps_the_tip(self):
        # fail-closed stays: a strand whose gate planned cargo plans from the tip
        tip = self.strand({"crates/x/src/lib.rs": "pub fn a() {}\npub fn c() {}\n"})
        strand = self.plan(tip, "strand", self.base)
        self.assertIn("build", [s["name"] for s in strand["stations"]])
        master, _cand = self.candidate(True)
        base, why = self.gate_base(tip, master, strand)
        self.assertEqual(tip, base, why)


def setUpModule():
    store_isolation.isolate()


def tearDownModule():
    store_isolation.restore()


if __name__ == "__main__":
    unittest.main()
