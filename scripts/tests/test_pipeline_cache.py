"""Unit tests for the station cache (`scripts/pipeline/cache.py`, W3, OR-DT-10).

The key of a station (or of one test binary) is the hash of its SOURCE
closure -- tracked blobs plus the overlay of the working tree, the tool
fingerprint and the build environment that changes semantics -- never of a
built binary. These tests build a tiny git workspace from
`fixtures/pipeline_w3/tree.json` and read `cargo metadata` from a fixture
JSON: no cargo runs here.
"""

import datetime
import json
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile
import unittest

HERE = pathlib.Path(__file__).resolve().parent
SCRIPTS = HERE.parent
FIX = HERE / "fixtures" / "pipeline_w3"
sys.path.insert(0, str(SCRIPTS))

from scripts.pipeline import cache, executor, gate, narrow, store  # noqa: E402
from scripts.tests import kit_time, store_isolation  # noqa: E402

FINGERPRINT = {"rustc": "rustc 1.80.0", "cargo": "cargo 1.80.0", "nextest": "0.9.143",
               "node": "v20.0.0", "playwright": "1.40.0", "linker": "cc 13",
               "python": "3.12.0", "cargo_config": "e3b0c44298fc1c14"}



def build_env(**extra):
    """A known stations' environment (PATH, HOME) plus `extra`
    (`test_lane_fingerprint.clean_env`)."""
    from scripts.tests.test_lane_fingerprint import clean_env
    return clean_env(**extra)


def _git(root, *args):
    subprocess.run(
        ["git", "-c", "user.name=fixture", "-c", "user.email=fixture@example.invalid",
         "-c", "commit.gpgsign=false", "-c", "core.hooksPath=/dev/null",
         "-c", "init.defaultBranch=master", *args],
        cwd=root, check=True, capture_output=True, text=True)


def fixture_tree(case, name="tree", parent=None):
    """A committed copy of the fixture workspace; returns its absolute path."""
    if parent is None:
        tmp = tempfile.TemporaryDirectory()
        case.addCleanup(tmp.cleanup)
        parent = tmp.name
    root = pathlib.Path(parent) / name
    files = json.loads((FIX / "tree.json").read_text(encoding="utf-8"))
    for rel, text in files.items():
        path = root / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")
    _git(root, "init", "-q")
    _git(root, "add", "-A")
    _git(root, "commit", "-q", "-m", "fixture")
    return str(root)


def fixture_metadata(root, home="/nonexistent-home"):
    """The fixture `cargo metadata` document, rooted at `root`."""
    text = (FIX / "metadata.json").read_text(encoding="utf-8")
    return json.loads(text.replace("@ROOT@", root).replace("@HOME@", home))


def simple_plan(stations, mode="strand"):
    return {"mode": mode, "repo": "meclaw", "stations": stations}


def tests_station(parts, profile="default"):
    return {"name": "tests", "scope": "x", "cargo": True, "run": True,
            "cmds": [{"argv": ["scripts/test-tier.sh", "filter", "x"], "cwd": ""}],
            "inputs": list(cache.INPUTS["tests"]), "env": "none",
            "parts": list(parts), "nextest": {"profile": profile, "filter": "x"}}


def plain_station(name, argv, run=True):
    return {"name": name, "scope": "s", "cargo": False, "run": run,
            "cmds": [{"argv": list(argv), "cwd": ""}], "inputs": list(cache.INPUTS.get(name, [])),
            "env": "none"}


class CacheKeys(unittest.TestCase):

    def keys(self, root, plan, fingerprint=None, metadata=None):
        meta = fixture_metadata(root) if metadata is None else metadata
        return cache.keys_for_plan(plan, root, fingerprint or FINGERPRINT, "lane-a",
                                   metadata=meta)

    def test_the_cargo_configuration_outside_the_tree_changes_every_key(self):
        """Re-check bundle N kit-fp M2 (N-K2): `$CARGO_HOME/config.toml` with rustflags
        builds otherwise; CARGO_HOME stays out of `build_env` (where), its file is
        `cargo_config` -- a key tool (KEY_TOOLS), so every key moves with it."""
        from scripts.pipeline import executor
        self.assertIn("cargo_config", cache.KEY_TOOLS)
        root = fixture_tree(self)
        plan = simple_plan([tests_station(["meclaw-cells"]),
                            plain_station("gate-selftest", ["python3", "-m", "unittest"])])
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        home = os.path.join(tmp.name, "cargo-home")
        os.mkdir(home)

        def keys():
            fp = dict(FINGERPRINT, **executor.build_fingerprint(build_env(CARGO_HOME=home), root))
            return {k: v["key"] for k, v in self.keys(root, plan, fp).items()}

        plain = keys()
        self.assertEqual({("tests", "meclaw-cells"), ("gate-selftest", "")}, set(plain))
        with open(os.path.join(home, "config.toml"), "w") as fh:
            fh.write('[build]\nrustflags = ["-C", "target-cpu=native"]\n')
        configured = keys()
        for k in plain:
            self.assertNotEqual(plain[k], configured[k], k)

    def test_cache_key_is_source_closure_not_binary_hash(self):
        """Two trees at two paths, CARGO_INCREMENTAL 0 and 1, different bytes in
        target/: one key. RUSTFLAGS does change it -- it changes the build."""
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        a = fixture_tree(self, "a", tmp.name)
        b = fixture_tree(self, "deeper/b", tmp.name)
        for root, blob in ((a, b"\x7fELF one"), (b, b"\x7fELF two, other bytes")):
            out = pathlib.Path(root) / "target" / "debug"
            out.mkdir(parents=True)
            (out / "meclaw").write_bytes(blob)
            (out / "deps").mkdir()
            (out / "deps" / "gh3_llm-0123").write_bytes(blob * 3)
        plan = simple_plan([tests_station(["meclaw-cells::gh3_llm", "meclaw-cells"]),
                            plain_station("scenarios:memory",
                                          ["python3", "workshop/evals/scenarios/run_scenarios.py"])])
        # the build part from its one producer (re-check tv-bn M7r A-M2), never by hand
        fp0 = dict(FINGERPRINT, **executor.build_fingerprint(
            build_env(CARGO_INCREMENTAL="0", CARGO_BUILD_JOBS="4"), a))
        fp1 = dict(FINGERPRINT, **executor.build_fingerprint(
            build_env(CARGO_INCREMENTAL="1", CARGO_BUILD_JOBS="24"), b))
        ka = self.keys(a, plan, fp0)
        kb = self.keys(b, plan, fp1)
        self.assertEqual(set(ka), {("tests", "meclaw-cells::gh3_llm"), ("tests", "meclaw-cells"),
                                   ("scenarios:memory", "")})
        for k in ka:
            self.assertEqual(ka[k]["key"], kb[k]["key"], k)
            self.assertNotIn("target/debug/meclaw", json.dumps(ka[k]["inputs"]))
        kc = self.keys(a, plan, dict(FINGERPRINT, **executor.build_fingerprint(
            build_env(RUSTFLAGS="-C debuginfo=0"), a)))
        self.assertNotEqual(ka[("tests", "meclaw-cells")]["key"],
                            kc[("tests", "meclaw-cells")]["key"])

    def fake_rustc(self, version="9.9.9"):
        """A rustc that answers its own version (orchestrator OR-T12-18 A-M2: the key's
        rustc is the building one -- a missing one keys nothing)."""
        d = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, d, True)
        path = os.path.join(d, "rustc")
        with open(path, "w") as fh:
            fh.write("#!/bin/sh\necho 'rustc %s (fake)'\n" % version)
        os.chmod(path, 0o755)
        return path

    def test_the_key_takes_the_rustc_that_builds(self):
        """Orchestrator OR-T12-18 A-M2: an update of the compiler behind the same RUSTC
        path is a change of the key; the key's rustc is RUSTC's, never PATH's."""
        root = fixture_tree(self)
        meta = fixture_metadata(root)
        plan = simple_plan([tests_station(["meclaw-cells"])])
        rustc = self.fake_rustc("9.9.9")
        from scripts.pipeline import executor

        def lookup():
            keys, _hits, fp = gate._lookup(None, plan, root, meta, FINGERPRINT, "lane-a", "strand", "off",
                                           executor.stations_build(None, build_env(RUSTC=rustc), root))
            return {k: v["key"] for k, v in keys.items()}, fp

        before, fp = lookup()
        self.assertEqual("rustc 9.9.9 (fake)", fp["rustc"])
        with open(rustc, "w") as fh:   # the same path, another compiler
            fh.write("#!/bin/sh\necho 'rustc 9.9.10 (fake)'\n")
        after, fp = lookup()
        self.assertEqual("rustc 9.9.10 (fake)", fp["rustc"])
        self.assertEqual(set(before), set(after))
        for k in before:
            self.assertNotEqual(before[k], after[k], k)

    def test_the_mode_keys_only_the_stations_whose_verdict_reads_it(self):
        """Orchestrator OR-T12-21: MECLAW_GATE_MODE (a verdict variable,
        `lane_sync.sh VERDICT_ENV`) never in the whole build environment -- there a
        strand's GREEN never answered the integration pass of the same tree, every
        Cargo station ran twice. It enters the key of a station that declares it
        (`env:<NAME>`) and of a part whose own code names it (`env:part`); a Cargo
        station keyed under strand is CACHED under integration, a mode-reading one
        is not. Through the real path (`gate._lookup`, `executor.stations_build`)."""
        root = fixture_tree(self)
        meta = fixture_metadata(root)
        reader = pathlib.Path(root) / "crates/meclaw-cells/tests/gh3_llm.rs"
        reader.write_text(reader.read_text() + '\n// std::env::var("MECLAW_GATE_MODE")\n')
        moded = plain_station("gate-selftest", ["python3", "-m", "unittest"])
        moded["inputs"] = moded["inputs"] + ["env:MECLAW_GATE_MODE"]
        plan = simple_plan([tests_station(["meclaw-cells::gh3_llm", "meclaw-cells"]),
                            plain_station("clippy", ["cargo", "clippy", "--workspace"]),
                            moded])
        from scripts.pipeline import executor

        def keys(mode):
            got, _hits, _fp = gate._lookup(None, plan, root, meta, FINGERPRINT, "lane-a", mode, "off",
                                           executor.stations_build(None, build_env(MECLAW_GATE_MODE=mode), root))
            return got

        strand, integration = keys("strand"), keys("integration")
        self.assertEqual(set(strand), {("tests", "meclaw-cells::gh3_llm"), ("tests", "meclaw-cells"),
                                       ("clippy", ""), ("gate-selftest", "")})
        for same in (("clippy", ""), ("tests", "meclaw-cells")):
            self.assertEqual(strand[same]["key"], integration[same]["key"], same)
        for other in (("tests", "meclaw-cells::gh3_llm"), ("gate-selftest", "")):
            self.assertNotEqual(strand[other]["key"], integration[other]["key"], other)
        # the strand's GREEN, looked up by the integration pass: CACHED only where
        # the verdict does not read the mode
        db = store.open(pathlib.Path(root).parent / "pipeline.db")
        self.addCleanup(db.close)
        cache.record(db, "run-000000000001", "lane-a", strand,
                     [{"station": s, "part": p, "verdict": "GREEN"} for s, p in strand])
        hits = cache.lookup(db, integration, "meclaw", "integration", "lane-a", gate._now_iso())
        self.assertEqual({("clippy", ""), ("tests", "meclaw-cells")},
                         {(h["station"], h.get("part") or "") for h in hits})
        # a name that is no verdict variable: no key (fail-closed)
        moded["inputs"] = moded["inputs"] + ["env:MECLAW_NOT_A_VERDICT"]
        self.assertNotIn(("gate-selftest", ""), keys("strand"))

    def test_the_kit_suite_reads_no_verdict_variable(self):
        """Orchestrator OR-T12-21: the kit suite (gate-selftest) keys without the
        verdict variables, so importing it drops them -- a case that needs one sets
        it itself (`scripts/tests/__init__.py`)."""
        from scripts.pipeline import lane
        names = lane.verdict_env()
        code = ("import os, sys; sys.path.insert(0, sys.argv[1]); import scripts.tests; "
                "print(sorted(n for n in sys.argv[2:] if n in os.environ))")
        env = dict(os.environ, **{n: "x" for n in names})
        res = subprocess.run([sys.executable, "-I", "-c", code, str(pathlib.Path(__file__).resolve().parents[2])]
                             + list(names), env=env, capture_output=True, text=True,
                             timeout=2 * kit_time.HANG)
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertEqual("[]", res.stdout.strip())

    def test_a_change_of_the_build_environment_is_never_cached(self):
        """Re-check tv-bn M7r A-M2: `build_env` had no producer -- a RUSTFLAGS or a
        CARGO_PROFILE_* entered no key, and a register written before a change keyed
        the old environment. Through the real path (`gate._lookup`, the register's
        fingerprint under the stations' environment, no `build_env` by hand)."""
        root = fixture_tree(self)
        meta = fixture_metadata(root)
        plan = simple_plan([tests_station(["meclaw-cells"]),
                            plain_station("gate-selftest", ["python3", "-m", "unittest"])])

        def lookup(register, env):
            from scripts.pipeline import executor
            keys, hits, fp = gate._lookup(None, plan, root, meta, register, "lane-a", "strand",
                                          "off", executor.stations_build(None, env, root))
            self.assertEqual([], hits)
            return {k: v["key"] for k, v in keys.items()}, fp

        plain = build_env()
        mold = build_env(RUSTFLAGS="--cfg tokio_unstable -C link-arg=-fuse-ld=mold")
        lto = build_env(CARGO_PROFILE_TEST_LTO="fat")
        base, fp = lookup(FINGERPRINT, plain)
        self.assertEqual(2, len(base))
        self.assertEqual("rust-lld", fp["linker"])
        # re-check tv-bn M7t A-M1: each of the measured gaps is a change of the key
        gaps = [build_env(**{n: v}) for n, v in (
            ("CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS", "-C link-arg=-fuse-ld=mold"),
            ("CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER", "clang"), ("RUSTC", self.fake_rustc()),
            ("CARGO_BUILD_RUSTC", self.fake_rustc()), ("RUSTC_BOOTSTRAP", "1"), ("CC", "clang"),
            ("CFLAGS", "-O3"),
            # orchestrator OR-T12-18 A-M1 (re-check tv-bn M7w A-M1): the whole environment
            ("LIBSQLITE3_FLAGS", "-DX"), ("SQLITE_MAX_EXPR_DEPTH", "0"), ("ICU4X_DATA_DIR", "/opt/icu"),
            ("RING_PREGENERATE_ASM", "1"), ("CC_x86_64-unknown-linux-gnu", "clang"), ("CPATH", "/opt/inc"),
            ("C_INCLUDE_PATH", "/opt/inc"), ("LIBRARY_PATH", "/opt/lib"), ("ZQX_INVENTED_BY_THE_PIN", "1"))]
        for env in [mold, lto] + gaps:
            other, _fp = lookup(FINGERPRINT, env)
            self.assertEqual(set(base), set(other))
            for k in base:
                self.assertNotEqual(base[k], other[k], (k, env))
        # how fast is not what: the same keys
        self.assertEqual(base, lookup(FINGERPRINT, build_env(CARGO_INCREMENTAL="0",
                                                             CARGO_BUILD_JOBS="2"))[0])
        # a register from before the change keys the stations' environment of now
        _keys, stale = lookup(FINGERPRINT, mold)
        self.assertEqual("mold", stale["linker"])
        self.assertEqual(base, lookup(stale, plain)[0])
        # the doubt after the run: other build environment, no record -- also
        # where the linker stays the same (a CARGO_PROFILE_* alone)
        keys = {("gate-selftest", ""): {"inputs": []}}
        self.assertIsNotNone(gate._cache_doubt(keys, fp, stale))
        self.assertIsNotNone(gate._cache_doubt(keys, fp, lookup(FINGERPRINT, lto)[1]))
        self.assertIsNone(gate._cache_doubt(keys, fp, dict(fp)))

    def test_a_fingerprint_without_a_tool_version_keys_nothing(self):
        """Re-check tv-bn M7r A-M1: no tool version, no key; no answer, no key at all."""
        root = fixture_tree(self)
        plan = simple_plan([plain_station("gate-selftest", ["python3", "-m", "unittest"])])
        good = dict(FINGERPRINT, build_env={})
        self.assertIn(("gate-selftest", ""), self.keys(root, plan, good))
        for tool in cache.KEY_TOOLS:
            with self.subTest(tool=tool):
                self.assertEqual({}, self.keys(root, plan, dict(good, **{tool: None})))
        self.assertEqual({}, self.keys(root, plan, dict(good, unanswered="exit 3")))

    def test_cache_closure_covers_dep_info(self):
        """Every file a real dep-info names (outside ~/.cargo, ~/.rustup and the
        target dir) lies in the declared closure of its binary."""
        root = fixture_tree(self)
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        home = os.path.join(tmp.name, "home")
        meta = fixture_metadata(root, home)
        deps = pathlib.Path(root) / "target" / "debug" / "deps"
        deps.mkdir(parents=True)
        for d in (FIX / "deps").iterdir():
            text = d.read_text(encoding="utf-8").replace("@ROOT@", root).replace("@HOME@", home)
            (deps / d.name).write_text(text, encoding="utf-8")
        checked, gaps = cache.dep_info_gaps(os.path.join(root, "target"), root, meta,
                                            home=home)
        self.assertEqual(gaps, [])
        # The include_str! target and the module file are counted, the
        # generated OUT_DIR file and the registry are not.
        self.assertIn(("meclaw-core", "templates/display/csp.json"), checked)
        self.assertIn(("meclaw-cells::gh3_llm", "crates/meclaw-cells/tests/mock_openai.rs"),
                      checked)
        self.assertFalse(any("target/" in p for _part, p in checked))
        self.assertFalse(any(".cargo" in p for _part, p in checked))
        # Negative control: a file the closure does not declare is a gap.
        (deps / "gh3_llm-1a2b3c4d5e6f7081.d").write_text(
            "%s/target/debug/deps/gh3_llm-1a2b3c4d5e6f7081: %s/crates/meclaw-cells/tests/gh3_llm.rs"
            " %s/docs/other.md\n" % (root, root, root), encoding="utf-8")
        _checked, gaps = cache.dep_info_gaps(os.path.join(root, "target"), root, meta,
                                             home=home)
        self.assertEqual(gaps, [("meclaw-cells::gh3_llm", "docs/other.md")])

    def test_cache_key_changes_with_input_blob(self):
        root = fixture_tree(self)
        plan = simple_plan([tests_station(["meclaw-cells::gh5_reads_doc",
                                           "meclaw-other::other_t"]),
                            plain_station("scenarios:memory",
                                          ["python3", "workshop/evals/scenarios/run_scenarios.py"])])
        before = self.keys(root, plan)
        # A doc the test names as a literal (rule 8 inverse), dirty in the tree.
        pathlib.Path(root, "docs/guide.md").write_text("# Guide v2\n", encoding="utf-8")
        after = self.keys(root, plan)
        self.assertNotEqual(before[("tests", "meclaw-cells::gh5_reads_doc")]["key"],
                            after[("tests", "meclaw-cells::gh5_reads_doc")]["key"])
        self.assertEqual(before[("tests", "meclaw-other::other_t")]["key"],
                         after[("tests", "meclaw-other::other_t")]["key"])
        # A runtime start (`target/debug/meclaw` literal) pulls the binary's
        # closure: a change in its crate moves the key of the test that starts it.
        pathlib.Path(root, "crates/meclaw/src/main.rs").write_text(
            "fn main() { println!(); }\n", encoding="utf-8")
        third = self.keys(root, plan)
        self.assertNotEqual(after[("tests", "meclaw-cells::gh5_reads_doc")]["key"],
                            third[("tests", "meclaw-cells::gh5_reads_doc")]["key"])
        self.assertNotEqual(after[("scenarios:memory", "")]["key"],
                            third[("scenarios:memory", "")]["key"])
        self.assertEqual(after[("tests", "meclaw-other::other_t")]["key"],
                         third[("tests", "meclaw-other::other_t")]["key"])
        # An untracked file inside a crate is part of the overlay.
        pathlib.Path(root, "crates/meclaw-other/src/new.rs").write_text("\n", encoding="utf-8")
        fourth = self.keys(root, plan)
        self.assertNotEqual(third[("tests", "meclaw-other::other_t")]["key"],
                            fourth[("tests", "meclaw-other::other_t")]["key"])
        # A tool upgrade moves every key.
        fifth = self.keys(root, plan, dict(FINGERPRINT, rustc="rustc 1.81.0"))
        for k in fourth:
            self.assertNotEqual(fourth[k]["key"], fifth[k]["key"], k)

    def test_cache_key_stable_across_worktree_paths(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        a = fixture_tree(self, "wt-one", tmp.name)
        b = fixture_tree(self, "x/y/wt-two", tmp.name)
        plan = simple_plan([
            tests_station(["meclaw-cells::gh1_uses_hive", "meclaw-cells::gh3_llm",
                           "meclaw-core", "meclaw::bin/meclaw"]),
            plain_station("clippy", ["cargo", "clippy", "--workspace", "--all-targets"]),
            plain_station("export-audit", ["python3", "plans/export-fixtures/make_export.py",
                                           "--gate-receipt", "{receipt}"]),
            plain_station("precheck", ["python3", "scripts/precheck.py"]),
            plain_station("persona-receipt", ["python3", "fingerprint.py"]),
            plain_station("corpus-committed", ["python3", "x.py"]),
        ], mode="integration")
        ka, kb = self.keys(a, plan), self.keys(b, plan)
        self.assertEqual({k: v["key"] for k, v in ka.items()},
                         {k: v["key"] for k, v in kb.items()})
        for never in cache.NEVER:
            self.assertFalse([k for k in ka if k[0] == never], never)
        self.assertIn(("clippy", ""), ka)

    def test_fmt_key_covers_crate_examples(self):
        # B-M3: `cargo fmt --all --check` checks the examples of every crate too.
        root = fixture_tree(self)
        meta = fixture_metadata(root)
        core = next(p for p in meta["packages"] if p["name"] == "meclaw-core")
        example = dict(next(t for t in core["targets"] if "test" in t["kind"]),
                       kind=["example"], crate_types=["bin"], name="phase_demo",
                       src_path=root + "/crates/meclaw-core/examples/phase_demo.rs")
        core["targets"].append(example)
        demo = pathlib.Path(root, "crates/meclaw-core/examples/phase_demo.rs")
        demo.parent.mkdir(parents=True, exist_ok=True)
        demo.write_text("fn main() {}\n", encoding="utf-8")
        plan = simple_plan([plain_station("fmt", ["cargo", "fmt", "--all", "--", "--check"])])
        before = self.keys(root, plan, metadata=meta)
        demo.write_text("fn main()   {}\n", encoding="utf-8")
        after = self.keys(root, plan, metadata=meta)
        self.assertNotEqual(before[("fmt", "")]["key"], after[("fmt", "")]["key"])

    def test_cache_metadata_failure_caches_nothing(self):
        root = fixture_tree(self)
        plan = simple_plan([tests_station(["meclaw-core"])])

        def broken(_root):
            raise cache.MetadataError("offline resolution failed")

        self.assertEqual(cache.keys_for_plan(plan, root, FINGERPRINT, "lane-a",
                                             metadata_fn=broken), {})


class CacheTable(unittest.TestCase):
    """`INPUTS` is the floor of every key and decides what is cacheable; the
    plan's own `inputs` can only add to it."""

    def keys(self, root, plan):
        return cache.keys_for_plan(plan, root, FINGERPRINT, "lane-a",
                                   metadata=fixture_metadata(root))

    def test_cache_table_covers_resolver_triggers(self):
        """Every `<label>: <path>` trigger of a station planned by the resolver
        lies in `INPUTS` of that station (the nextest stations are held to the
        resolver by the closures of their parts, see the plan tests)."""
        import gate_plan as gp
        root = fixture_tree(self)
        meta = fixture_metadata(root)
        samples = ["crates/meclaw-cells/src/lib.rs", "crates/meclaw-cells/tests/gh3_llm.rs",
                   "Cargo.lock", "deny.toml", ".config/nextest.toml", "rust-toolchain.toml",
                   "templates/README.md", "templates/memory-hive/template.json",
                   "templates/display/compose/scenarios/s.json", "templates/builder/t.json",
                   "examples/organism/grow-1.json", "examples/meclaw-os/a.json",
                   "workshop/evals/scenarios/cases/a.json", "workshop/evals/engine.py",
                   "workshop/evals/p5-longmemeval/judge.py", "workshop/evals/builder-scenarios/a.py",
                   "workshop/evals/conversation-guide/a.py", "workshop/tools/display-lab/a.py",
                   "workshop/tools/display-layout-browser.mjs", "workshop/tools/llm-conformance/a.py",
                   "workshop/evals/persona/a.py", "scripts/strand.sh", "scripts/tests/test_x.py",
                   "plans/export-fixtures/a.py", ".github/gates/unwrap_budget.txt",
                   "crates/meclaw-cells/src/recall.rs", "docs/guide.md",
                   # rule 10 (welle-tb S1b): crate files outside src/ and tests/
                   "crates/meclaw-cells/schemas/body.json", "crates/meclaw-cells/examples/demo.rs",
                   "crates/meclaw-cells/benches/hot.rs", "crates/meclaw-cells/README.md",
                   "crates/meclaw-cells/migrations/0001.sql", "crates/meclaw-cells/config.toml"]
        checked = 0
        for path in samples:
            for mode in ("strand", "integration", "release"):
                doc = gp.to_json([path], mode, root, explain=True)
                stations = [{"name": st["name"], "reason": st.get("reason", []),
                             "cmds": [{"argv": a} for a in st.get("cmds", [])]}
                            for st in doc["stations"]
                            if st["name"] in cache.INPUTS and st["name"] not in
                            ("tests", "browser:display")]
                checked += sum(len(st["reason"]) for st in stations)
                gaps = cache.uncovered_triggers({"stations": stations}, root, meta)
                self.assertEqual(gaps, [], (path, mode))
        self.assertGreater(checked, 100)

    def test_cache_parts_key_ignores_per_diff_trigger_globs(self):
        """The plan adds `glob:<path>` for each trigger of a diff to `tests`;
        the closures of the parts already hold those paths, so the key of a
        part must not depend on which diff planned it."""
        root = fixture_tree(self)
        plain = tests_station(["meclaw-cells::gh5_reads_doc", "meclaw-other::other_t"])
        with_globs = dict(plain, inputs=plain["inputs"] + ["glob:docs/guide.md",
                                                           "glob:templates/memory-hive/template.json"])
        a = self.keys(root, simple_plan([plain]))
        b = self.keys(root, simple_plan([with_globs]))
        self.assertEqual({k: v["key"] for k, v in a.items()},
                         {k: v["key"] for k, v in b.items()})

    def test_cache_narrowed_part_never_serves_whole_part(self):
        """Review Z-I gap C-A: a part run under a narrowed filter
        (`binary_id(=X) & test(/^m::/)`) is GREEN only for module m. The
        resolved nextest filter and the narrowing state (on/off, map rev)
        are in the key, so a later unnarrowed request of the same part
        (merge gate, integration) misses the cache."""
        root = fixture_tree(self)
        part = "meclaw-cells::gh5_reads_doc"
        whole = tests_station([part])
        whole["nextest"] = {"profile": "default", "filter": "rdeps(meclaw-cells)"}
        narrowed = dict(whole, nextest={"profile": "default",
                                        "filter": "(binary_id(=%s) & test(/^m::/))" % part},
                        select={"narrow": {"on": True, "map": "f004e6c36"}})
        same_filter_on = dict(whole, select={"narrow": {"on": True, "map": "f004e6c36"}})
        other_map = dict(narrowed, select={"narrow": {"on": True, "map": "aaaaaaaaa"}})
        k_whole = self.keys(root, simple_plan([whole]))[("tests", part)]["key"]
        k_narrow = self.keys(root, simple_plan([narrowed]))[("tests", part)]["key"]
        self.assertNotEqual(k_whole, k_narrow)
        self.assertNotEqual(k_whole, self.keys(root, simple_plan([same_filter_on]))[("tests", part)]["key"])
        self.assertNotEqual(k_narrow, self.keys(root, simple_plan([other_map]))[("tests", part)]["key"])
        self.assertEqual(k_whole, self.keys(root, simple_plan([dict(whole)]))[("tests", part)]["key"])
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        db = store.open(pathlib.Path(tmp.name) / "pipeline.db")
        self.addCleanup(db.close)
        cache.record(db, "run-000000000001", "lane-a", self.keys(root, simple_plan([narrowed])),
                     [{"station": "tests", "part": part, "verdict": "GREEN"}])
        now = datetime.datetime.now(datetime.timezone.utc)
        self.assertEqual(cache.lookup(db, self.keys(root, simple_plan([whole])), "meclaw",
                                      "merge", "lane-b", now), [])
        hit = cache.lookup(db, self.keys(root, simple_plan([narrowed])), "meclaw", "strand",
                           "lane-b", now)
        self.assertEqual([h["part"] for h in hit], [part])

    def test_narrowed_merge_cache_key_differs_from_unnarrowed(self):
        """V12 (welle-tb S1 part B, GH #1159): a narrowed and an unnarrowed
        merge never share a key -- also when the narrowed merge replaced no
        rdeps and so planned the very filter of the unnarrowed one. The
        narrowing state of a narrowed merge (`narrow.state`) is `on` whenever
        the merge was narrowed with a map; an unnarrowed merge carries none."""
        root = fixture_tree(self)
        part = "meclaw-cells::gh5_reads_doc"
        unnarrowed = tests_station([part])
        unnarrowed["nextest"] = {"profile": "default", "filter": "binary_id(=%s)" % part}
        term = "stage:merge narrowed parent=0123456789 strand-run=tr-000000000001"
        same_filter = dict(unnarrowed, select={"narrow": narrow.state(True, "f004e6c36", term)})
        cut = dict(unnarrowed, nextest={"profile": "default",
                                        "filter": "(binary_id(=%s) & test(/^m::/))" % part},
                   select={"narrow": narrow.state(True, "f004e6c36", term)})
        off = dict(unnarrowed, select={"narrow": narrow.state(False, "", None)})
        k_off = self.keys(root, simple_plan([unnarrowed]))[("tests", part)]["key"]
        self.assertEqual(k_off, self.keys(root, simple_plan([off]))[("tests", part)]["key"])
        for narrowed in (same_filter, cut):
            self.assertTrue(narrowed["select"]["narrow"]["on"])
            k = self.keys(root, simple_plan([narrowed]))[("tests", part)]["key"]
            self.assertNotEqual(k_off, k)

    def test_cache_table_is_the_floor_and_decides_cacheability(self):
        root = fixture_tree(self)
        narrow = plain_station("gate-selftest", ["python3", "-m", "unittest"])
        narrow["inputs"] = ["gate_infra"]
        undeclared = plain_station("slow-tests", ["python3", "scripts/slow_tests.py"])
        undeclared["inputs"] = ["glob:**"]
        before = self.keys(root, simple_plan([narrow, undeclared]))
        self.assertNotIn(("slow-tests", ""), before)
        # A file outside `gate_infra` that the selftest reads (the table says
        # `tree`) moves the key although the plan declared less.
        pathlib.Path(root, "scripts/pipeline_helper.py").write_text("X = 1\n", encoding="utf-8")
        after = self.keys(root, simple_plan([narrow, undeclared]))
        self.assertNotEqual(before[("gate-selftest", "")]["key"],
                            after[("gate-selftest", "")]["key"])

    def test_cache_wildcard_label_input(self):
        root = fixture_tree(self)
        st = plain_station("scenarios:builder", ["python3", "x.py"])
        st["inputs"] = ["template:*"]
        st["reason"] = ["template:builder: templates/builder/template.json"]
        self.assertEqual(cache.uncovered_triggers(simple_plan([st]), root,
                                                  fixture_metadata(root)), [])
        st["inputs"] = ["template:display"]
        self.assertEqual(cache.uncovered_triggers(simple_plan([st]), root,
                                                  fixture_metadata(root)),
                         [("scenarios:builder", "template:builder: templates/builder/template.json")])


class DepInfoCli(unittest.TestCase):

    def test_check_dep_info_cli_exit_codes(self):
        root = fixture_tree(self)
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        home = os.path.join(tmp.name, "home")
        meta_file = os.path.join(tmp.name, "metadata.json")
        with open(meta_file, "w", encoding="utf-8") as fh:
            json.dump(fixture_metadata(root, home), fh)
        deps = pathlib.Path(root) / "target" / "debug" / "deps"
        deps.mkdir(parents=True)
        for d in (FIX / "deps").iterdir():
            text = d.read_text(encoding="utf-8").replace("@ROOT@", root).replace("@HOME@", home)
            (deps / d.name).write_text(text, encoding="utf-8")
        argv = [sys.executable, "-m", "scripts.pipeline.cache", "check-dep-info",
                "--target", os.path.join(root, "target"), "--root", root,
                "--metadata", meta_file, "--home", home]
        cwd = str(SCRIPTS.parent)
        ok = subprocess.run(argv, cwd=cwd, capture_output=True, text=True)
        self.assertEqual(ok.returncode, 0, ok.stderr)
        self.assertEqual(ok.stdout, "")
        (deps / "gh3_llm-1a2b3c4d5e6f7081.d").write_text(
            "%s/target/debug/deps/gh3_llm-1a2b3c4d5e6f7081: %s/docs/other.md\n" % (root, root),
            encoding="utf-8")
        bad = subprocess.run(argv, cwd=cwd, capture_output=True, text=True)
        self.assertEqual(bad.returncode, 1, bad.stderr)
        self.assertEqual(bad.stdout, "meclaw-cells::gh3_llm\tdocs/other.md\n")


class CacheStore(unittest.TestCase):

    def setUp(self):
        # W0a's store.open takes a path (WAL file), never ":memory:".
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.db = store.open(pathlib.Path(tmp.name) / "pipeline.db")
        self.addCleanup(self.db.close)

    def keys(self):
        return {("tests", "a::x"): {"key": "k-tests-x", "inputs": ["artefact:binary:a::x"],
                                     "repo": "meclaw"},
                ("tests", "a::y"): {"key": "k-tests-y", "inputs": [], "repo": "meclaw"},
                ("clippy", ""): {"key": "k-clippy", "inputs": ["rust_src"], "repo": "meclaw"},
                ("fmt", ""): {"key": "k-fmt", "inputs": [], "repo": "meclaw"},
                ("deny", ""): {"key": "k-deny", "inputs": [], "repo": "meclaw"},
                ("export-audit", ""): {"key": "k-audit", "inputs": [], "repo": "meclaw"},
                ("precheck", ""): {"key": "k-pre", "inputs": [], "repo": "meclaw"}}

    def test_cache_never_stores_red(self):
        keys = self.keys()
        cache.record(self.db, "run-000000000001", "lane-a", keys, [
            {"station": "tests", "part": "a::x", "verdict": "GREEN"},
            {"station": "tests", "part": "a::y", "verdict": "RED"},
            {"station": "clippy", "part": "", "verdict": "FLAKY"},
            {"station": "fmt", "part": "", "verdict": "SKIP"},
            {"station": "deny", "part": "", "verdict": "ABORTED"},
            {"station": "export-audit", "part": "", "verdict": "GREEN"},
            {"station": "precheck", "part": "", "verdict": "GREEN"},
            {"station": "unknown", "part": "", "verdict": "GREEN"},
        ])
        rows = self.db.execute("SELECT station, part, verdict, run, runner FROM cache "
                               "ORDER BY station, part").fetchall()
        # FLAKY is a finding (VG TV-F1): only GREEN is ever cached.
        self.assertEqual([tuple(r) for r in rows], [
            ("tests", "a::x", "GREEN", "run-000000000001", "lane-a")])
        known = cache.lookup(self.db, keys, "meclaw", "strand", "lane-b",
                             store.now())
        self.assertEqual(sorted((k["station"], k["part"], k["key"], k["from_run"])
                                for k in known),
                         [("tests", "a::x", "k-tests-x", "run-000000000001")])

    def test_release_cache_only_same_lane_24h(self):
        keys = self.keys()
        now = datetime.datetime(2026, 10, 8, 12, 0, 0, tzinfo=datetime.timezone.utc)

        def put(key, runner, hours_ago, station="tests", part="a::x", repo="meclaw"):
            at = (now - datetime.timedelta(hours=hours_ago)).strftime("%Y-%m-%dT%H:%M:%SZ")
            self.db.execute(
                "INSERT OR REPLACE INTO cache VALUES (?, ?, ?, 'GREEN', ?, ?, ?, ?)",
                (key, station, part, "run-%s" % runner, runner, at,
                 json.dumps({"repo": repo, "inputs": []})))

        put("k-tests-x", "lane-b", 2)                       # other lane, fresh
        put("k-tests-y", "lane-a", 25, part="a::y")         # same lane, too old for R
        put("k-clippy", "lane-a", 1, station="clippy", part="")   # same lane, fresh
        put("k-fmt", "lane-a", 24 * 8, station="fmt", part="")   # older than 7 days
        put("k-deny", "lane-a", 1, station="deny", part="", repo="coding")  # other repo

        def found(mode, lane):
            return sorted(k["key"] for k in cache.lookup(self.db, keys, "meclaw", mode,
                                                          lane, now))

        self.assertEqual(found("release", "lane-a"), ["k-clippy"])
        self.assertEqual(found("release", "lane-b"), ["k-tests-x"])
        self.assertEqual(found("strand", "lane-a"), ["k-clippy", "k-tests-x", "k-tests-y"])
        self.assertEqual(found("integration", "lane-c"),
                         ["k-clippy", "k-tests-x", "k-tests-y"])


class CacheBrowserDuty(unittest.TestCase):
    """GH #1161/#1167: a station that books an engine runs its proofs with
    MECLAW_REQUIRE_BROWSERS=1 once the lane fingerprint carries the `browsers`
    probe -- an engine that does not start is RED, not SKIP. A green run from
    before (build02: WebKit SKIPped for a missing libGL.so.1) must never answer
    such a run as CACHED."""

    PROBED = dict(FINGERPRINT, browsers={"chromium": "ok", "webkit": "ok"})

    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.db = store.open(pathlib.Path(tmp.name) / "pipeline.db")
        self.addCleanup(self.db.close)
        self.root = fixture_tree(self)

    def plan(self):
        st = plain_station("browser:display", ["node", "workshop/tools/display-browser-lab.mjs"])
        st["needs"] = {"browser_chromium": 1, "browser_webkit": 1, "colony": 2, "cpu": 4}
        memory = plain_station("scenarios:memory",
                               ["python3", "workshop/evals/scenarios/run_scenarios.py"])
        return simple_plan([st, memory])

    def keys(self, fingerprint):
        return cache.keys_for_plan(self.plan(), self.root, fingerprint, "lane-a",
                                   metadata=fixture_metadata(self.root))

    def hits(self, keys):
        return [(k["station"], k["part"]) for k in
                cache.lookup(self.db, keys, "meclaw", "strand", "lane-a", store.now())]

    def put_old(self, key, meta):
        self.db.execute("INSERT OR REPLACE INTO cache VALUES (?, ?, ?, 'GREEN', ?, ?, ?, ?)",
                        (key, "browser:display", "", "run-000000000009", "lane-b",
                         store.now(), json.dumps(meta)))

    def test_browser_duty_enters_the_key_of_a_booking_station_only(self):
        pre, duty = self.keys(FINGERPRINT), self.keys(self.PROBED)
        b = ("browser:display", "")
        # Re-check H2 M-N5: without the probe a booking station has no key at all
        self.assertNotIn(b, pre)
        self.assertEqual(duty[b]["browsers"]["required"], "1")
        self.assertEqual(duty[b]["browsers"]["probe"], {"chromium": "ok", "webkit": "ok"})
        # a station that books no engine keeps its key: no cache-wide drop
        m = ("scenarios:memory", "")
        self.assertNotIn("browsers", duty[m])
        self.assertEqual(pre[m]["key"], duty[m]["key"])
        # another probe answer is another key
        gl = self.keys(dict(FINGERPRINT, browsers={"chromium": "ok",
                                                   "webkit": "libraries not found: libGL.so.1"}))
        self.assertNotEqual(gl[b]["key"], duty[b]["key"])

    def test_old_entry_without_the_duty_never_hits(self):
        keys = self.keys(self.PROBED)
        b = ("browser:display", "")
        # an entry from before the duty field, stored under the very key (fail-closed)
        self.put_old(keys[b]["key"], {"repo": "meclaw", "inputs": []})
        self.assertEqual(self.hits(keys), [])

    def test_duty_0_entry_never_serves_duty_1(self):
        b = ("browser:display", "")
        pre = self.keys(FINGERPRINT)
        cache.record(self.db, "run-000000000001", "lane-b", pre,
                     [{"station": "browser:display", "part": "", "verdict": "GREEN"}])
        # Re-check H2 M-N5: an unprobed run stores nothing for a booking station
        self.assertEqual(self.hits(pre), [])
        self.assertEqual(self.db.execute(
            "SELECT COUNT(*) FROM cache WHERE station = 'browser:display'").fetchone()[0], 0)
        duty = self.keys(self.PROBED)
        self.assertEqual(self.hits(duty), [])
        # and a row under the duty-1 key that says duty 0 does not hit either
        self.put_old(duty[b]["key"], {"repo": "meclaw", "inputs": [],
                                      "browsers": dict(duty[b]["browsers"], required="0")})
        self.assertEqual(self.hits(duty), [])

    def test_same_duty_is_cached_as_before(self):
        duty = self.keys(self.PROBED)
        cache.record(self.db, "run-000000000002", "lane-b", duty,
                     [{"station": "browser:display", "part": "", "verdict": "GREEN"},
                      {"station": "scenarios:memory", "part": "", "verdict": "GREEN"}])
        self.assertEqual(sorted(self.hits(self.keys(dict(self.PROBED)))),
                         [("browser:display", ""), ("scenarios:memory", "")])

    def test_a_change_the_browser_proofs_read_is_no_cached_hit(self):
        # OR-TB.S7.11 (S1b re-check): the browser proofs serve `templates/web/**`
        # and source `workshop/tools/wkenv.sh` -- both are inputs of the key.
        for rel in ("templates/web/x.html", "workshop/tools/wkenv.sh"):
            with self.subTest(rel=rel):
                before = self.keys(self.PROBED)
                cache.record(self.db, "run-000000000004", "lane-b", before,
                             [{"station": "browser:display", "part": "", "verdict": "GREEN"}])
                self.assertIn(("browser:display", ""), self.hits(before))
                path = pathlib.Path(self.root) / rel
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text("changed %s\n" % rel, encoding="utf-8")
                _git(self.root, "add", "-A")
                _git(self.root, "commit", "-q", "-m", rel)
                self.assertNotIn(("browser:display", ""), self.hits(self.keys(self.PROBED)))

    def test_probe_change_during_the_run_records_no_browser_entry(self):
        from scripts.pipeline import gate
        duty = self.keys(self.PROBED)
        evs = [{"event": "station_ended", "data": {"station": s, "verdict": "GREEN"}}
               for s in ("browser:display", "scenarios:memory")]
        after = dict(self.PROBED, browsers={"chromium": "ok", "webkit": "no bundle"})
        gate._record(self.db, "run-000000000003", "lane-a", duty, self.PROBED, after, evs)
        self.assertEqual(self.hits(duty), [("scenarios:memory", "")])


class CacheBrowserDutyParts(CacheBrowserDuty):
    """Re-check tb-s7 H2 M-N4 / M-N5 at the station the plan really builds:
    `browser:display` is a nextest station with parts (`plan.NEXTEST`), its
    argv from `gate_plan.browser_cmds`, so its keys are PART keys, one per
    test binary. The duty must be in every part key and every part entry."""

    PARTS = ("meclaw-cells::710_the_sheet_holds_in_both_engines_browser",
             "meclaw-cells::gh3_llm")

    def plan(self):
        from scripts.pipeline import plan as plan_mod
        sys.path.insert(0, str(HERE.parent))
        try:
            import gate_plan
        finally:
            sys.path.remove(str(HERE.parent))
        name = "browser:display"
        st = {"name": name, "scope": "sheet+colony", "cargo": False, "run": True,
              "cmds": [{"argv": a, "cwd": ""} for a in gate_plan.browser_cmds(True)],
              "inputs": list(cache.INPUTS[name]), "env": "none",
              "needs": dict(plan_mod.NEEDS[name]), "parts": list(self.PARTS),
              "nextest": {"profile": plan_mod.NEXTEST[name], "filter": "binary(/x/)"}}
        return simple_plan([st])

    def part_keys(self, fingerprint):
        return {k: v for k, v in self.keys(fingerprint).items() if k[0] == "browser:display"}

    def put_part(self, key, part, meta):
        self.db.execute("INSERT OR REPLACE INTO cache VALUES (?, ?, ?, 'GREEN', ?, ?, ?, ?)",
                        (key, "browser:display", part, "run-000000000009", "lane-b",
                         store.now(), json.dumps(meta)))

    def test_every_part_key_carries_the_duty(self):
        duty = self.part_keys(self.PROBED)
        self.assertEqual(set(duty), {("browser:display", p) for p in self.PARTS})
        gl = self.part_keys(dict(FINGERPRINT, browsers={
            "chromium": "ok", "webkit": "libraries not found: libGL.so.1"}))
        for k, v in duty.items():
            self.assertEqual(v["browsers"], {"required": "1",
                                             "probe": {"chromium": "ok", "webkit": "ok"}})
            # the duty is in the part's hash: another probe, another key
            self.assertNotEqual(v["key"], gl[k]["key"], k)

    def test_old_part_entry_without_the_duty_never_hits(self):
        duty = self.part_keys(self.PROBED)
        for (st, part), v in duty.items():
            self.put_part(v["key"], part, {"repo": "meclaw", "inputs": []})
        self.assertEqual(self.hits(duty), [])

    def test_same_duty_parts_are_cached(self):
        duty = self.part_keys(self.PROBED)
        cache.record(self.db, "run-000000000002", "lane-b", duty,
                     [{"station": "browser:display", "part": p, "verdict": "GREEN"}
                      for p in self.PARTS])
        self.assertEqual(sorted(self.hits(self.part_keys(dict(self.PROBED)))),
                         sorted(("browser:display", p) for p in self.PARTS))

    def test_without_the_probe_no_part_has_a_key(self):
        # M-N5: lane runs key with `fingerprint_expected`; one stored before the
        # probe existed has no `browsers` -- such a run neither serves nor stores.
        self.assertEqual(self.part_keys(FINGERPRINT), {})
        for bad in ("libGL", None, ["ok"]):
            with self.subTest(browsers=bad):
                self.assertEqual(self.part_keys(dict(FINGERPRINT, browsers=bad)), {})

    # The whole-station cases of the parent run against the part plan too.
    def test_a_change_the_browser_proofs_read_is_no_cached_hit(self):
        for rel in ("templates/web/x.html", "workshop/tools/wkenv.sh"):
            with self.subTest(rel=rel):
                before = self.part_keys(self.PROBED)
                cache.record(self.db, "run-000000000004", "lane-b", before,
                             [{"station": "browser:display", "part": p, "verdict": "GREEN"}
                              for p in self.PARTS])
                self.assertTrue(self.hits(before))
                path = pathlib.Path(self.root) / rel
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text("changed %s\n" % rel, encoding="utf-8")
                _git(self.root, "add", "-A")
                _git(self.root, "commit", "-q", "-m", rel)
                self.assertEqual(self.hits(self.part_keys(self.PROBED)), [])

    def test_browser_duty_enters_the_key_of_a_booking_station_only(self):
        self.assertEqual(self.part_keys(FINGERPRINT), {})
        self.assertTrue(self.part_keys(self.PROBED))

    def test_old_entry_without_the_duty_never_hits(self):
        self.test_old_part_entry_without_the_duty_never_hits()

    def test_duty_0_entry_never_serves_duty_1(self):
        duty = self.part_keys(self.PROBED)
        for (st, part), v in duty.items():
            self.put_part(v["key"], part, {"repo": "meclaw", "inputs": [],
                                           "browsers": dict(v["browsers"], required="0")})
        self.assertEqual(self.hits(duty), [])

    def test_same_duty_is_cached_as_before(self):
        self.test_same_duty_parts_are_cached()

    def test_probe_change_during_the_run_records_no_browser_entry(self):
        from scripts.pipeline import gate
        duty = self.part_keys(self.PROBED)
        evs = [{"event": "test_result", "data": {"station": "browser:display", "part": p,
                                                  "verdict": "GREEN"}} for p in self.PARTS]
        after = dict(self.PROBED, browsers={"chromium": "ok", "webkit": "no bundle"})
        gate._record(self.db, "run-000000000003", "lane-a", duty, self.PROBED, after, evs)
        self.assertEqual(self.hits(duty), [])


class TreeIndex(unittest.TestCase):

    def test_tree_index_overlays_dirty_untracked_and_deleted(self):
        root = fixture_tree(self)
        clean = cache.tree_index(root)
        self.assertIn("docs/guide.md", clean)
        self.assertEqual(len(clean["docs/guide.md"]), 40)
        pathlib.Path(root, "docs/guide.md").write_text("changed\n", encoding="utf-8")
        pathlib.Path(root, "docs/new.md").write_text("new\n", encoding="utf-8")
        os.remove(os.path.join(root, "docs/other.md"))
        pathlib.Path(root, "target").mkdir()
        pathlib.Path(root, "target/ignored.bin").write_bytes(b"x")
        dirty = cache.tree_index(root)
        self.assertNotEqual(clean["docs/guide.md"], dirty["docs/guide.md"])
        self.assertIn("docs/new.md", dirty)
        self.assertNotIn("docs/other.md", dirty)
        self.assertNotIn("target/ignored.bin", dirty)


def setUpModule():
    store_isolation.isolate()


def tearDownModule():
    store_isolation.restore()


if __name__ == "__main__":
    unittest.main()
