"""Unit tests for the pipeline plan (`scripts/pipeline/plan.py`, W3).

The plan is `gate_plan.py --format json --explain`, enriched additively:
needs, inputs, stage, after, when, timeout_min, env, the `build` station and
the nextest parts. The resolver runs in-process here against the fixture
workspace (`fixtures/pipeline_w3/`); `cargo metadata` is the fixture JSON.
"""

import json
import os
import pathlib
import subprocess
import sys
import unittest
from unittest import mock

HERE = pathlib.Path(__file__).resolve().parent
SCRIPTS = HERE.parent
sys.path.insert(0, str(SCRIPTS))

import gate_plan as gp  # noqa: E402
from scripts.pipeline import cache, plan as pl  # noqa: E402
from scripts.tests.test_pipeline_cache import fixture_metadata, fixture_tree, _git  # noqa: E402
from scripts.tests import store_isolation  # noqa: E402

NEW_KEYS = {"needs", "inputs", "stage", "after", "when", "timeout_min", "env", "reason",
            "cmds", "run", "keep_ws"}
NEEDS_KEYS = {"cpu", "colony", "browser_chromium", "browser_webkit", "export", "cargo"}

SAMPLES = [
    (["crates/meclaw-cells/src/lib.rs"], "strand"),
    (["crates/meclaw-cells/tests/mock_openai.rs"], "strand"),
    (["crates/meclaw-core/src/lib.rs", "docs/guide.md"], "strand"),
    (["templates/memory-hive/template.json", "workshop/evals/scenarios/cases/a.json"], "strand"),
    (["templates/display/csp.json"], "integration"),
    (["Cargo.lock"], "integration"),
    (["docs/guide.md", "README.md"], "release"),
    ([], "strand"),
    (["scripts/test-tier.sh"], "ci"),
]


def resolver(repo_root, mode, files):
    return gp.to_json(files, mode, repo_root, explain=True)


class PlanCase(unittest.TestCase):

    def setUp(self):
        self.root = fixture_tree(self)
        self.meta = fixture_metadata(self.root)

    def plan(self, files, mode="strand", **kw):
        return pl.build_plan(self.root, mode, "HEAD", files=files, resolver=resolver,
                             metadata=self.meta, **kw)

    def by_name(self, doc):
        return {s["name"]: s for s in doc["stations"]}


class PlanShape(PlanCase):

    def test_plan_adds_needs_inputs_after_additively(self):
        for files, mode in SAMPLES:
            old = gp.to_json(files, mode, self.root)
            # GH #1191: the one change the plan makes to the resolver's rows
            pl.split_alone(old["stations"])
            new = self.plan(files, mode)
            for key in ("mode", "classes", "crates"):
                self.assertEqual(old[key], new[key], (files, mode, key))
            self.assertEqual([r["name"] for r in old["stations"]],
                             [s["name"] for s in new["stations"] if s.get("build") is None])
            rows = self.by_name(new)
            for row in old["stations"]:
                st = rows[row["name"]]
                for key, value in row.items():
                    if key == "cmds":
                        # V0 § 2.1 names `cmds` as [{argv, cwd}]; the legacy list of
                        # argv lists is the `argv` column of it.
                        self.assertEqual([c["argv"] for c in st["cmds"]], value)
                    elif key == "env":
                        # V0 § 2.1: `env: lane|none`; legacy `env: true` is `lane`.
                        self.assertEqual(st["env"], "lane" if value else "none")
                    else:
                        self.assertEqual(st[key], value, (files, mode, row["name"], key))
                self.assertLessEqual(NEW_KEYS, set(st), row["name"])
                self.assertEqual(set(st["needs"]), NEEDS_KEYS)
                for v in st["needs"].values():
                    self.assertIsInstance(v, int)
                self.assertTrue(st["inputs"], row["name"])
                self.assertTrue(5 <= st["timeout_min"] <= 120)
                for cmd in st["cmds"]:
                    self.assertEqual(set(cmd), {"argv", "cwd"})
            self.assertIn("plan_reason", new)
            self.assertIsInstance(new["critical_path_s"], float)
            self.assertEqual(len(new["rev"]), 40)

    def test_needs_cargo_only_for_target_lock_stations(self):
        doc = self.plan(["Cargo.lock"], "release")
        rows = self.by_name(doc)
        for name in ("fmt", "clippy", "doctests", "deny", "deny-advisories",
                     "unwrap-budget", "build"):
            self.assertEqual(rows[name]["needs"]["cargo"], 1, name)
        for name in ("tests", "browser:display", "slow-tests", "scenarios:memory",
                     "roadmap-anchors"):
            self.assertEqual(rows[name]["needs"]["cargo"], 0, name)
        # GH #1162: the release export stations build in a target of their own
        # (MECLAW_EXPORT_WORKDIR / MECLAW_EXPORT_CHECK_TARGET), not in the
        # pass's: no share of its lock, a side width of cpu, one export at a time.
        self.assertEqual(pl.OWN_TARGET, {"export-tree-tests", "export-audit"})
        for name in sorted(pl.OWN_TARGET):
            self.assertEqual({k: rows[name]["needs"][k] for k in ("cargo", "export", "cpu")},
                             {"cargo": 0, "export": 1, "cpu": pl.EXPORT_CPU}, name)
        self.assertEqual(pl.EXPORT_CPU, 8)
        dry = self.by_name(self.plan(["Cargo.lock"], "integration"))
        self.assertEqual(dry["export-audit"]["needs"]["cargo"], 0)
        self.assertEqual(rows["tests"]["needs"]["cpu"], pl.CPU_LANE)
        # clippy books four threads in every mode (GH #1162), so it fits beside
        # `tests` once the build is done; the threads it books are the threads
        # it runs (`executor._book_threads`).
        self.assertEqual(pl.CLIPPY_CPU, 4)
        for mode_rows in (rows, dry, self.by_name(self.plan(["Cargo.lock"], "strand"))):
            self.assertEqual(mode_rows["clippy"]["needs"]["cpu"], pl.CLIPPY_CPU)
        self.assertEqual(pl.resolve_cpu(pl.CPU_LANE, 24), 24)
        self.assertFalse(hasattr(pl, "CPU_HALF"), "a half booking ran at full width")
        self.assertEqual(pl.resolve_cpu(4, 24), 4)

    def test_every_plan_books_a_valid_cpu(self):
        # Admission refuses a station without a thread count; no plan the
        # tree builds may carry one.
        from scripts.pipeline import schedule
        files = ["Cargo.lock", "scripts/pipeline/plan.py", "docs/config.md",
                 "crates/meclaw-colony/src/lib.rs"]
        for mode in ("strand", "integration", "release"):
            with self.subTest(mode=mode):
                doc = self.plan(files, mode)
                self.assertTrue(doc["stations"])
                schedule.check_cpu(doc["stations"])
        for name in list(pl.NEEDS) + ["export-audit", "fmt-unknown"]:
            for mode in ("strand", "integration", "release"):
                schedule.check_cpu([{"name": name, "needs": pl._needs(name, {}, mode)}])

    def test_plan_inputs_cover_resolver_triggers(self):
        """Every (class, path) the resolver names as a trigger lies in the
        station's declared inputs -- over-declaration is allowed, under-declaration
        is RED (the `test_filter` rule)."""
        samples = [([p], m) for p in (
            "crates/meclaw-cells/src/lib.rs", "crates/meclaw-cells/src/recall.rs",
            "crates/meclaw-cells/tests/gh3_llm.rs", "crates/meclaw-cells/tests/mock_openai.rs",
            "crates/meclaw-core/src/lib.rs", "crates/meclaw/src/main.rs",
            "crates/meclaw-testing/src/lib.rs", "Cargo.lock", "Cargo.toml", "deny.toml",
            ".config/nextest.toml", "rust-toolchain.toml", ".cargo/config.toml",
            "templates/README.md", "templates/memory-hive/template.json",
            "templates/display/csp.json", "templates/display/compose/scenarios/s.json",
            "templates/builder/template.json", "examples/organism/grow-1.json",
            "docs/guide.md", "README.md", "workshop/evals/scenarios/cases/a.json",
            "workshop/evals/engine.py", "workshop/evals/p5-longmemeval/judge.py",
            "workshop/evals/builder-scenarios/a.py", "workshop/evals/conversation-guide/a.py",
            "workshop/tools/display-lab/a.py", "workshop/tools/display-layout-browser.mjs",
            "workshop/tools/llm-conformance/a.py", "workshop/evals/persona/a.py",
            "scripts/strand.sh", "scripts/tests/test_x.py", "plans/export-fixtures/a.py",
            ".github/gates/unwrap_budget.txt", ".github/fixtures/a.txt",
            "crates/meclaw-cells/tests/710_the_sheet_holds_in_both_engines_browser.rs",
        ) for m in ("strand", "release")]
        checked = 0
        for files, mode in samples:
            doc = self.plan(files, mode)
            gaps = cache.uncovered_triggers(doc, self.root, self.meta)
            self.assertEqual(gaps, [], (files, mode))
            checked += sum(1 for s in doc["stations"] for r in s["reason"]
                           if r.endswith(": " + files[0]))
        self.assertGreater(checked, 60)
        # Under-declaration is caught: clippy without its rust classes.
        doc = self.plan(["crates/meclaw-cells/src/lib.rs"])
        rows = self.by_name(doc)
        rows["clippy"]["inputs"] = ["workspace"]
        rows["tests"]["inputs"] = ["glob:.config/nextest.toml"]
        rows["tests"]["parts"] = ["meclaw-other"]
        gaps = cache.uncovered_triggers(doc, self.root, self.meta)
        self.assertIn(("clippy", "rust_src: crates/meclaw-cells/src/lib.rs"), gaps)
        self.assertIn(("tests", "rust_src: crates/meclaw-cells/src/lib.rs"), gaps)

    def test_plan_build_station_precedes_tests(self):
        doc = self.plan(["crates/meclaw-cells/tests/gh3_llm.rs"])
        names = [s["name"] for s in doc["stations"]]
        rows = self.by_name(doc)
        self.assertLess(names.index("build"), names.index("tests"))
        self.assertIn("build", rows["tests"]["after"])
        self.assertIn("build", rows["slow-tests"]["after"])
        self.assertIn("tests", rows["slow-tests"]["after"])
        b = rows["build"]
        self.assertEqual(b["needs"]["cargo"], 1)
        self.assertEqual(rows["tests"]["needs"]["cargo"], 0)
        # test_targets: the crate's test target names, for the merged calls (#1166)
        self.assertIn("gh3_llm", b["build"]["test_targets"]["meclaw-cells"])
        self.assertEqual({k: v for k, v in b["build"].items() if k != "test_targets"},
                         {"meclaw_bin": False, "workspace": False, "packages": [],
                          "tests": [["meclaw-cells", "gh3_llm"]], "lib_bins": [],
                          "extra": []})
        self.assertEqual([c["argv"] for c in b["cmds"]], [
            ["cargo", "nextest", "run", "--no-run", "-p", "meclaw-cells", "--test", "gh3_llm"]])
        self.assertEqual(rows["tests"]["parts"], ["meclaw-cells::gh3_llm"])
        self.assertEqual(rows["tests"]["nextest"],
                         {"profile": "default", "filter": rows["tests"]["scope"]})
        # rdeps(X) builds the packages that depend on X, and the parts are their binaries.
        doc = self.plan(["crates/meclaw-core/src/lib.rs"])
        rows = self.by_name(doc)
        self.assertEqual(rows["build"]["build"]["packages"],
                         ["meclaw", "meclaw-cells", "meclaw-core"])
        self.assertIn("meclaw-core", rows["tests"]["parts"])
        self.assertIn("meclaw::bin/meclaw", rows["tests"]["parts"])
        self.assertIn("meclaw-cells::gh5_reads_doc", rows["tests"]["parts"])
        self.assertNotIn("meclaw-other", rows["tests"]["parts"])
        # A scenario station needs the binary: build builds it first.
        doc = self.plan(["workshop/evals/scenarios/cases/a.json"])
        rows = self.by_name(doc)
        self.assertTrue(rows["build"]["build"]["meclaw_bin"])
        self.assertIn(["cargo", "build", "--bin", "meclaw"],
                      [c["argv"] for c in rows["build"]["cmds"]])
        self.assertIn("build", rows["scenarios:memory"]["after"])
        self.assertIn("artefact:meclaw", rows["scenarios:memory"]["inputs"])
        # The empty diff: the t0 floor builds lib and bin targets only.
        rows = self.by_name(self.plan([]))
        self.assertEqual(rows["build"]["build"]["lib_bins"],
                         ["meclaw", "meclaw-cells", "meclaw-core", "meclaw-other",
                          "meclaw-testing"])
        # `meclaw` is a bin-only package: its one nextest binary is `meclaw::bin/meclaw`.
        self.assertEqual(rows["tests"]["parts"],
                         ["meclaw-cells", "meclaw-core", "meclaw-other",
                          "meclaw-testing", "meclaw::bin/meclaw"])
        # Browser locks are extra build targets, their station a nextest station.
        rows = self.by_name(self.plan(["templates/display/csp.json"], "integration"))
        self.assertEqual(rows["build"]["build"]["extra"],
                         [["meclaw-cells", "710_the_sheet_holds_in_both_engines_browser"]])
        self.assertEqual(rows["browser:display"]["parts"],
                         ["meclaw-cells::710_the_sheet_holds_in_both_engines_browser"])
        self.assertEqual(rows["browser:display"]["nextest"]["profile"], "browser")
        self.assertEqual(rows["browser:display"]["needs"]["cargo"], 0)

    def test_crate_isolation_waits_for_build_on_four_threads(self):
        # #1166 re-check M-1: the single-crate check holds the target lock, so it
        # starts after `build` (the tests wait for build, nobody waits for it) and
        # books 4 threads, which leaves the lane to the stations beside it.
        doc = self.plan(["crates/meclaw-core/src/lib.rs"], "release")
        rows = self.by_name(doc)
        iso = rows["crate-isolation"]
        self.assertIn("build", iso["after"])
        self.assertEqual((1, 4), (iso["needs"]["cargo"], iso["needs"]["cpu"]))
        self.assertEqual(30, iso["timeout_min"])
        self.assertEqual(["R"], iso["stage"])
        self.assertTrue(all(c["argv"][:2] == ["cargo", "check"] for c in iso["cmds"]))

    def test_release_plans_the_full_suite_whatever_narrows_its_row(self):
        # KR-TB-1 (d), GH #1162 OR-TB.S4.39: a one-file release plans all() from the
        # resolver, and the plan keeps it all() even when a row arrives narrowed
        # (a narrowing of the map, `narrow.apply`, a future stage) -- the release
        # runs the full suite by rule, the narrowing switch on or off.
        def narrowed(repo_root, mode, files):
            doc = resolver(repo_root, mode, files)
            row = next(r for r in doc["stations"] if r["name"] == "tests")
            old = row["select"]["filter"]
            row["select"] = dict(row["select"], all=False, rdeps=["meclaw-core"],
                                 filter="rdeps(meclaw-core)")
            row["cmd"] = [("rdeps(meclaw-core)" if a == old else a) for a in row["cmd"]]
            return doc
        for res, env in ((resolver, "1"), (narrowed, "1"), (narrowed, "0")):
            with mock.patch.dict(os.environ, {"MECLAW_GATE_NARROW": env}):
                doc = pl.build_plan(self.root, "release", "HEAD",
                                    files=["crates/meclaw-core/src/lib.rs"], resolver=res,
                                    metadata=self.meta, narrow=True)
            tests = self.by_name(doc)["tests"]
            filt = tests["nextest"]["filter"]
            self.assertTrue(filt.startswith("all()"), (res.__name__, env, filt))
            self.assertIn(filt, [a for c in tests["cmds"] for a in c["argv"]], res.__name__)

    def test_only_a_station_with_a_declared_prior_carries_one(self):
        # GH #1162 OR-TB.S4.38: without a measurement `crate-isolation` ranked by its
        # 30 min timeout as the first station of the first release (V6 sim +32.8 s warm,
        # +103.4 s A2). It carries the cold S6-ISO duration as `prior_s`; every other
        # station has none and keeps its timeout as the fallback. GH #1191: so does
        # `browser:alone` -- by its 15 min timeout it ranked like the path to
        # `slow-tests` and held an empty lane up to RESERVE_AGE_S before `tests`.
        doc = self.plan(["crates/meclaw-core/src/lib.rs"], "release")
        priors = {s["name"]: s["prior_s"] for s in doc["stations"] if "prior_s" in s}
        self.assertEqual({"crate-isolation": 273.3, "browser:alone": 60}, priors)
        self.assertEqual(30, self.by_name(doc)["crate-isolation"]["timeout_min"])

    def test_plan_export_audit_runs_after_the_stations_it_reads(self):
        # GH #1162: the audit waits for the four stations whose verdicts it
        # reads from the gate receipt (make_export.GATE_RECEIPT_STATIONS, the
        # equality is locked in plans/export-fixtures/test_drift_gate.py), no
        # longer for every station -- last in the pass it put 88 s (dry) and
        # 424.7 s (release) on the critical path. The pass still sums every
        # verdict; a red station after the audit leaves the pass red.
        for mode in ("integration", "release"):
            doc = self.plan(["docs/guide.md", "crates/meclaw-core/src/lib.rs"], mode)
            names = [s["name"] for s in doc["stations"]]
            rows = self.by_name(doc)
            audit = rows["export-audit"]
            reads = set(pl.AUDIT_READS) & set(names)
            # recall-harness plans in integration only for its own diffs (R-P2).
            self.assertLessEqual(set(pl.AUDIT_READS) - reads,
                                 set() if mode == "release" else {"recall-harness"}, mode)
            also = set(pl.AUDIT_AFTER_ALSO) & set(names)
            self.assertEqual(set(audit["after"]), reads | also, mode)
            self.assertNotIn("tests", audit["after"], mode)
            pos = {n: i for i, n in enumerate(names)}
            for name in audit["after"]:
                self.assertLess(pos[name], pos["export-audit"], name)
            self.assertEqual(audit["when"], {"outcome": "any"})
            # Weg d (GH #1162): phase 2 `build` runs whatever phase 1 says -- a
            # break there breaks phase 2 too and stays RED in it; a consumer never
            # starts without a build (OR-TB.S4.20). Every other station: success.
            regardless = {"export-audit"} | ({"build"} if "build:browser" in names else set())
            for name in names:
                if name not in regardless:
                    self.assertEqual(rows[name]["when"], {"outcome": "success"}, name)
            if "build:browser" in names:
                self.assertEqual(rows["build"]["when"], {"outcome": "any"}, mode)
                self.assertEqual(rows["build:browser"]["when"], {"outcome": "success"}, mode)
            self.assertIn("export-selftest", audit["after"])
            if mode == "release":
                self.assertIn("export-selftest", rows["export-tree-tests"]["after"])
                self.assertEqual(rows["export-tree-tests"]["needs"]["export"], 1)
            self.assertIn("corpus-committed", rows["corpus"]["after"])
            self.assertIn("precheck", rows["clippy"]["after"])
            self.assertEqual(rows["precheck"]["after"], [])

    def test_plan_corpus_feeds_recall_harness(self):
        rows = self.by_name(self.plan(["docs/guide.md"], "release"))
        self.assertIn("corpus", rows["recall-harness"]["after"])
        self.assertIn("build", rows["recall-harness"]["after"])

    def test_only_cuts_the_run_not_the_plan(self):
        full = self.plan(["crates/meclaw-cells/tests/gh3_llm.rs"])
        cut = self.plan(["crates/meclaw-cells/tests/gh3_llm.rs"], only=["tests"])
        self.assertEqual([s["name"] for s in full["stations"]],
                         [s["name"] for s in cut["stations"]])
        rows = self.by_name(cut)
        self.assertTrue(rows["tests"]["run"])
        self.assertTrue(rows["build"]["run"])
        self.assertFalse(rows["clippy"]["run"])
        self.assertEqual(cut["only"], ["tests"])

    def test_only_tests_names_the_browser_units_it_runs_nowhere(self):
        # OR-TB.S10.B.2 (tb-s3 L-2): `tests` subtracts every browser unit (rule
        # 11) and `browser:display` runs them -- `--only tests` cuts that station,
        # so its units run in no station. Fail-closed visible: a NOTE names them.
        files = ["templates/display/csp.json", "crates/meclaw-cells/src/lib.rs"]
        unit = "meclaw-cells::710_the_sheet_holds_in_both_engines_browser"
        cut = self.plan(files, "integration", only=["tests"])
        rows = self.by_name(cut)
        self.assertTrue(rows["tests"]["run"])
        self.assertFalse(rows["browser:display"]["run"])
        mine = [n for n in cut.get("notes") or [] if "OR-TB.S10.B.2" in n]
        self.assertEqual(["--only cuts browser:display: browser units %s run in no station "
                          "-- `tests` subtracts them (OR-TB.S10.B.2)" % unit], mine)
        both = self.plan(files, "integration", only=["tests", "browser:display"])
        self.assertEqual([], [n for n in both.get("notes") or [] if "OR-TB.S10.B.2" in n])
        alone = self.plan(files, "integration", only=["clippy"])
        self.assertEqual([], [n for n in alone.get("notes") or [] if "OR-TB.S10.B.2" in n])

    def test_station_cap_is_three_times_p95(self):
        """TV-K5 (b), ADR-0050: a cap is a hang marker of at least 3x the P95 of
        the station's duration, clamped to [5, 120] min; a calibration without a
        P95 takes its P90 (the old rule was P90 x 2)."""
        doc = self.plan(["crates/meclaw-cells/src/lib.rs"],
                        calibration={"clippy": {"p95_s": 600.0, "p90_s": 500.0, "p50_s": 300.0},
                                     "fmt": {"p95_s": 3.0, "p50_s": 2.0},
                                     "doctests": {"p90_s": 400.0},
                                     "tests@meclaw": {"p95_s": 7200.0, "p50_s": 100.0}})
        rows = self.by_name(doc)
        self.assertEqual(rows["clippy"]["timeout_min"], 30)
        self.assertEqual(rows["fmt"]["timeout_min"], 5)
        self.assertEqual(rows["tests"]["timeout_min"], 120)
        self.assertEqual(pl.hang_cap_min(601.0), 31)
        self.assertEqual(pl.hang_cap_min(None), None)
        # Re-check tv-p0 B2: the strand plan holds no `doctests`, so the P90
        # fallback is pinned on `_timeout` itself (400 s x 3 = 20 min).
        self.assertEqual(20, pl._timeout("doctests", {"doctests": {"p90_s": 400.0}}))
        self.assertEqual(20, pl._timeout("doctests", {"doctests": {"p95_s": 400.0,
                                                                   "p90_s": 100.0}}))

    def test_critical_path_follows_after(self):
        stations = [{"name": "a", "after": []}, {"name": "b", "after": ["a"]},
                    {"name": "c", "after": ["a"]}, {"name": "d", "after": ["b", "c"]}]
        secs, names = pl.critical_path(stations, {"a": 1.0, "b": 5.0, "c": 2.0, "d": 1.0})
        self.assertEqual(secs, 7.0)
        self.assertEqual(names, ["a", "b", "d"])
        with self.assertRaises(ValueError):
            pl.critical_path([{"name": "x", "after": ["y"]}, {"name": "y", "after": ["x"]}], {})


class ChangedFiles(unittest.TestCase):

    def test_changed_files_is_the_gate_sh_diff(self):
        root = fixture_tree(self)
        _git(root, "checkout", "-q", "-b", "strand")
        pathlib.Path(root, "docs/guide.md").write_text("v2\n", encoding="utf-8")
        _git(root, "commit", "-q", "-am", "edit")
        pathlib.Path(root, "docs/other.md").write_text("dirty\n", encoding="utf-8")
        pathlib.Path(root, "docs/new.md").write_text("new\n", encoding="utf-8")
        _git(root, "mv", "README.md", "README2.md")
        self.assertEqual(pl.changed_files(root, "strand", None),
                         ["README.md", "README2.md", "docs/guide.md", "docs/new.md",
                          "docs/other.md"])
        base = subprocess.run(["git", "rev-parse", "master"], cwd=root, capture_output=True,
                              text=True, check=True).stdout.strip()
        self.assertEqual(pl.changed_files(root, "strand", base),
                         pl.changed_files(root, "strand", None))
        self.assertIn("crates/meclaw/src/main.rs", pl.changed_files(root, "ci", None))

    def test_a_committed_rename_lists_both_sides(self):
        # Re-check tb-s2 h1 m1: `git diff --name-only` without --no-renames named
        # only the target -- a Rust file moved under plans/ read as "no Rust source".
        root = fixture_tree(self)
        _git(root, "checkout", "-q", "-b", "strand")
        pathlib.Path(root, "plans/archive").mkdir(parents=True, exist_ok=True)
        _git(root, "mv", "crates/meclaw/src/main.rs", "plans/archive/main.rs")
        _git(root, "commit", "-q", "-m", "move")
        self.assertEqual(["crates/meclaw/src/main.rs", "plans/archive/main.rs"],
                         pl.changed_files(root, "strand", None))


class CommandLine(unittest.TestCase):

    def run_cli(self, root, *args):
        return subprocess.run(
            [sys.executable, "-m", "scripts.pipeline.plan", "--repo", root, *args],
            cwd=str(SCRIPTS.parent), capture_output=True, text=True)

    def test_cli_prints_the_plan_of_the_diff(self):
        """`python3 -m scripts.pipeline.plan` runs the real resolver as a
        subprocess and plans the gate.sh diff when no files are given."""
        root = fixture_tree(self)
        _git(root, "checkout", "-q", "-b", "strand")
        pathlib.Path(root, "crates/meclaw-cells/tests/gh3_llm.rs").write_text(
            "// v2\n", encoding="utf-8")
        _git(root, "commit", "-q", "-am", "edit")
        proc = self.run_cli(root, "--mode", "strand", "--only", "tests")
        self.assertEqual(proc.returncode, 0, proc.stderr)
        doc = json.loads(proc.stdout)
        master = subprocess.run(["git", "rev-parse", "master"], cwd=root, capture_output=True,
                                text=True, check=True).stdout.strip()
        self.assertEqual(doc["base"], master)
        self.assertEqual(doc["only"], ["tests"])
        rows = {s["name"]: s for s in doc["stations"]}
        self.assertEqual(rows["tests"]["scope"], "binary_id(=meclaw-cells::gh3_llm)")
        self.assertTrue(rows["tests"]["run"] and rows["build"]["run"])
        self.assertFalse(rows["clippy"]["run"])
        # Without `--metadata` the explicit binaries are the parts.
        self.assertEqual(rows["tests"]["parts"], ["meclaw-cells::gh3_llm"])
        bad = self.run_cli(root, "--mode", "strand", "--only", "no-such-station")
        self.assertEqual(bad.returncode, 2)
        self.assertIn("no-such-station", bad.stderr)


class CommandLineGraphError(unittest.TestCase):
    """Re-check v2-kit c23 M-3: a graph the narrowing could not build ends the
    CLI with `plan: <reason>` and exit 2, as `gate: no plan` does -- no traceback."""

    def test_a_failed_graph_build_is_one_line_and_exit_2(self):
        import contextlib  # noqa: PLC0415
        import io  # noqa: PLC0415
        from scripts.pipeline import graph as graph_mod  # noqa: PLC0415
        err = io.StringIO()
        boom = graph_mod.GraphBuildError("auto build of the graph of abc failed: OSError: x")
        with mock.patch.object(pl, "build_plan", side_effect=boom), \
                contextlib.redirect_stderr(err):
            rc = pl.main(["--mode", "strand", "--repo", "/nonexistent-repo"], out=io.StringIO())
        self.assertEqual(2, rc)
        self.assertEqual("plan: auto build of the graph of abc failed: OSError: x\n",
                         err.getvalue())


class Gh1002Alone(PlanCase):
    """GH #1191 (welle-tb S1e, K4): gh1002 measures a 6 s bound on Fast 3G with the
    CPU slowed 4x; beside `tests` it measured the lane's load (6/10 isolated on
    build04, gate bd490c7ec). It leaves the shared parts of `browser:display` and
    runs in a station of its own that books the lane exclusively."""

    def display_row(self, cmds):
        return {"name": "browser:display", "scope": "sheet+colony", "cargo": True,
                "cmds": cmds, "env": True, "reason": ["display_browser: x"],
                "parallel": {"by": "cmd", "profiles": list(gp.BROWSER_PROFILES)}}

    def test_gh1002_leaves_the_shared_parts_for_a_station_of_its_own(self):
        rows = [self.display_row(gp.browser_cmds(True))]
        notes = pl.split_alone(rows)
        self.assertEqual(["browser:display", pl.ALONE], [r["name"] for r in rows], notes)
        shared, alone = rows
        for argv in shared["cmds"]:
            self.assertNotIn("gh1002", argv[2], "gh1002 is back in a shared part")
            self.assertIn(gp.CSP_LOCK, argv[2])
        self.assertEqual(2, len(shared["cmds"]))
        self.assertNotIn("meclaw-cells::gh1002_slow_3g_connects_browser",
                         shared["select"]["extra"])
        self.assertEqual(["scripts/test-tier.sh", "filter", gp.SLOW3G_LOCK, "--run-ignored", "all"],
                         alone["cmd"])
        self.assertEqual(["meclaw-cells::gh1002_slow_3g_connects_browser"],
                         alone["select"]["extra"])
        self.assertIn(gp.SLOW3G_LOCK, gp.ALONE_LOCKS)

    def test_a_narrowed_row_of_gh1002_alone_leaves_the_plan_to_its_station(self):
        rows = [self.display_row(gp.browser_cmds(
            True, keep=["crates/meclaw-cells/tests/gh1002_slow_3g_connects_browser.rs"]))]
        pl.split_alone(rows)
        self.assertEqual([pl.ALONE], [r["name"] for r in rows])
        # a narrowed row without gh1002 stays as it is
        csp = "crates/meclaw-cells/tests/gh867_a_display_runs_under_a_strict_csp_browser.rs"
        rows = [self.display_row(gp.browser_cmds(True, keep=[csp]))]
        before = [list(c) for c in rows[0]["cmds"]]
        self.assertEqual([], pl.split_alone(rows))
        self.assertEqual(["browser:display"], [r["name"] for r in rows])
        self.assertEqual(before, rows[0]["cmds"])

    def test_the_station_of_gh1002_books_the_lane_alone(self):
        rows = self.by_name(self.plan(["templates/display/csp.json"], "integration"))
        alone = rows[pl.ALONE]
        self.assertIs(True, alone["alone"])
        self.assertEqual(60, alone["prior_s"])
        self.assertEqual(pl.CPU_LANE, alone["needs"]["cpu"])
        self.assertEqual(1, alone["needs"]["browser_chromium"])
        self.assertEqual(0, alone["needs"]["browser_webkit"])
        self.assertEqual(0, alone["needs"]["cargo"])
        self.assertEqual("browser", alone["nextest"]["profile"])
        self.assertIn("build", alone["after"])
        self.assertEqual(rows["browser:display"]["stage"], alone["stage"])
        self.assertNotIn("alone", rows["browser:display"])
        for c in rows["browser:display"]["cmds"]:
            self.assertNotIn("gh1002", " ".join(c["argv"]))
        # `only` naming the station it came from takes it along
        rows = self.by_name(self.plan(["templates/display/csp.json"], "integration",
                                      only=["browser:display"]))
        self.assertTrue(rows[pl.ALONE]["run"])


def setUpModule():
    store_isolation.isolate()


def tearDownModule():
    store_isolation.restore()


if __name__ == "__main__":
    unittest.main()
