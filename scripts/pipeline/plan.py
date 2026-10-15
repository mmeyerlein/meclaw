"""The station plan of one gate pass (W3, V0 section 4).

`build_plan` asks the resolver -- `scripts/gate_plan.py --format json
--explain` -- which stations a diff needs and enriches its answer ADDITIVELY:
every field the resolver gives stays as it is, with two V0 mappings
(OR-DT.W3.a): the legacy `cmds` list of argv lists becomes `[{argv, cwd}]`
(the legacy single `cmd`, `cwd` and `cwds` stay beside it), and the legacy
`env: true` becomes `env: "lane"` (absent: `"none"`). Each station gains

    needs        {cpu, colony, browser_chromium, browser_webkit, export, cargo}
                 -- `cpu` may be `CPU_LANE` / `CPU_SHARDS`, see `resolve_cpu`;
                 `cargo` only means "holds the target lock"
    inputs       resolver classes, `glob:<pattern>`, `artefact:meclaw`,
                 `tool:<name>` -- the source the cache key is made of
    stage        the modes (S, I, R, C) that can plan the station
    after, when  the edges that were implicit in the serial order (V0 s. 4)
    timeout_min  a hang marker, never a budget (TV-K5): calibration p95 x 3
                 (p90 without a p95), clamped to [5, 120]; the gate raises it to
                 3 x the p95 of the last 30 green runs on the lane's class
                 (`schedule.with_hang_caps`). Exceeding it is RED `HANG:`.
    prior_s      only a station in PRIOR_S: the declared seconds it ranks with
                 until the store holds a measurement (GH #1162 OR-TB.S4.38)
    reason       `<class>: <path>` -- why the resolver planned it
    run, keep_ws `run: false` when `only` cuts the station from the run
    parts, nextest  nextest stations only: the parts are nextest binary ids

and a `build` station is inserted before its first consumer: it builds the
cargo targets the nextest stations reuse (`cargo nextest run --no-run`) and
`target/debug/meclaw` for the stations that read `artefact:meclaw`. With
`browser:display` in the plan, phase 1 `build:browser` comes first (GH #1162
Weg d, EARLY_BUILD): only the browser's lock binaries, in the call of the full
build that builds them; the browser starts after it, `build` follows with
`when: any` (a break in phase 1 breaks phase 2 and stays RED there).
The locks of `gate_plan.ALONE_LOCKS` leave the `browser:display` row after every
narrowing and run in the station `browser:alone` (`ALONE`, `split_alone`), which
carries `alone: true`: the executor starts it on an empty lane and nothing beside
it (GH #1191).

`cargo metadata` is never run here; `gate` passes the cached metadata of
the tree (`gate.metadata_path`). Without `metadata` the parts are the
binaries the resolver names explicitly; a selection the resolver states as
`rdeps()`, `all()` or the t0 floor cannot be split into binaries then, and
its station has no parts: it runs as one, gets no cache key (never CACHED),
and `build` falls back to `--workspace`.
"""

import argparse
import atexit
import importlib.util
import json
import math
import os
import re
import subprocess
import sys
import tempfile

# `needs.cpu` is fixed before admission picks a lane; `CPU_LANE` stands for
# "every thread of the lane" until `resolve_cpu` knows nproc. A cargo station
# runs at the lane's full width (CARGO_BUILD_JOBS), so it books all of it.
CPU_LANE = 0
# `gate-selftest` books one thread per shard of `selftest_shard.py` (GH #1126).
CPU_SHARDS = -2
SELFTEST_SHARDS_ENV = "MECLAW_SELFTEST_SHARDS"
# Three shards already reach the floor of the longest module (test_strand_sh,
# about 150 s); more shards only take threads from tests and build.
SELFTEST_SHARDS_DEFAULT = 4
# The symbolic values; any other `needs.cpu` is a thread count >= 1.
CPU_SYMBOLS = (CPU_LANE, CPU_SHARDS)
# `needs.colony` of `scenarios:memory`: the colonies of the executing lane that
# `browser:display` leaves (OR-TB.S3.12/13). A lane's colonies come from ONE source,
# `schedule.colony_capacity` (its `cores=`, welle-tb S5); the port register caps the
# slots the executor hands out (`ports.SLOT_COUNT`); `resolve_colony`
# turns the symbol into slots once the lane is known, and the executor refuses a
# station that still carries it (RED before start).
COLONY_REST = -1

NEEDS_KEYS = ("cpu", "colony", "browser_chromium", "browser_webkit", "export", "cargo")

# `clippy` books a side width, not the lane (GH #1162). The integration pass
# 7b1e0e5 (build01, 12 threads, events.jsonl) started clippy only when `tests`
# ended (00:34:27): `tests` 12 + `browser:display` 4 + `unwrap-budget` 2 filled
# the overbooked cpu (12 x 1.5 = 18) and a clippy booked at the lane's 12 never
# fit beside them -- 102 s serial after 1527 s of tests. Not the cargo lock:
# `tests` reuses the build's binaries and runs no cargo. At 4 jobs clippy took
# 101.6 s there, far inside any `tests` station; the threads it runs are the
# threads it books (`executor._book_threads`).
CLIPPY_CPU = 4
# The release export stations build in a target of their own (the export tree's,
# `MECLAW_EXPORT_WORKDIR` / `MECLAW_EXPORT_CHECK_TARGET`, never the pass's
# `target/`), so they take no share of the target lock (`cargo` 0) and run beside
# `build` and `tests` (GH #1162, M0 A7: tree tests warm 274.9 s + full audit
# 149.8 s = 424.7 s, serial after the suite before). Eight threads: on the
# 24-thread lane `tests` 24 + `browser:display` 4 + 8 = 36 = 24 x 1.5.
EXPORT_CPU = 8
OWN_TARGET = frozenset({"export-tree-tests", "export-audit"})

# Start values (W3 "capacity"); a station not named here needs one thread.
NEEDS = {
    "build": {"cargo": 1, "cpu": CPU_LANE},
    "tests": {"cpu": CPU_LANE},
    # KR-TB-1 (d): the release's scenario class (57 units, gate_plan SCENARIO_STATION)
    "scenario-tests": {"cpu": 4},
    "clippy": {"cargo": 1, "cpu": CLIPPY_CPU},
    "doctests": {"cargo": 1, "cpu": CPU_LANE},
    "deny": {"cargo": 1, "cpu": 2},
    "unwrap-budget": {"cargo": 1, "cpu": 2},
    "fmt": {"cargo": 1, "cpu": 1},
    "deny-advisories": {"cargo": 1, "cpu": 1},
    # #1166 M-1: a quarter of a 12-thread lane, so it runs beside the long
    # stations without cargo (scenarios:memory) instead of after them
    "crate-isolation": {"cargo": 1, "cpu": 4},
    # The rest beside browser, not the register's eight: with eight memory never
    # fitted beside `browser:display` and ran after it on the integration path
    # (OR-TB.S3.12, RZ-S4-1); 6 real cores -> 3, 12 -> 5 (register 8).
    "scenarios:memory": {"colony": COLONY_REST, "cpu": 4},
    "scenarios:builder": {"colony": 4, "cpu": 2},
    "scenarios:display": {"cpu": 4},
    # Three colonies at once (OR-TB.S3.14): the Chromium part two (`chromium-part` in
    # .config/nextest.toml), the WebKit part one (`webkit`). Serial Chromium took
    # 420 s on build02, over the WebKit floor of 327 s.
    "browser:display": {"browser_chromium": 1, "browser_webkit": 1, "colony": 3, "cpu": 4},
    # One Chromium test, the lane to itself (`alone`, GH #1191).
    "browser:alone": {"browser_chromium": 1, "colony": 1, "cpu": CPU_LANE},
    "export-tree-tests": {"export": 1, "cargo": 0, "cpu": EXPORT_CPU},
    "gate-selftest": {"cpu": CPU_SHARDS},
}
# `export-audit` builds the export tree in release only; integration is a dry run.
EXPORT_AUDIT_NEEDS = {
    "release": {"export": 1, "cargo": 0, "cpu": EXPORT_CPU},
    "integration": {"export": 1, "cpu": 1},
}
# The stations whose verdicts `export-audit` reads from the gate receipt
# (`make_export.GATE_RECEIPT_STATIONS`: R11 corpus, R12 scenarios:builder, R14
# recall-harness, R17 roadmap-anchors). The audit waits for these, not for every
# station: last in the pass it put 88 s (integration, dry) and 424.7 s (release)
# on the critical path (GH #1162). A station still running when the audit reads
# is no gap: the audit reads only what it waits for, and the pass's own verdict
# still sums every station.
AUDIT_READS = ("corpus", "scenarios:builder", "recall-harness", "roadmap-anchors")
# Kept from the old order at no cost: the audit's own selftest (seconds) and, in
# release, the export tree tests ("right before export-audit", GH #1101; both
# book `export` 1, so they run one after the other anyway).
AUDIT_AFTER_ALSO = ("export-selftest", "export-tree-tests")

# Stations the resolver marks `cargo` that reuse the binaries of `build`
# (nextest "reusing builds") and so never take the target lock themselves.
REUSE = frozenset({"tests", "slow-tests", "scenario-tests", "browser:display", "browser:alone"})

# Consumers of the `build` station, and the ones among them that run
# `target/debug/meclaw` (OR-DT.W3.d: `scenarios:display` needs neither).
BUILD_CONSUMERS = ("tests", "slow-tests", "scenario-tests", "scenarios:memory", "scenarios:builder",
                   "browser:display", "browser:alone", "recall-harness", "persona-cases")
MECLAW_BIN = frozenset({"scenarios:memory", "scenarios:builder", "browser:display",
                        "recall-harness", "persona-cases"})
NEXTEST = {"tests": "default", "scenario-tests": "scenario", "browser:display": "browser",
           "browser:alone": "browser"}

# Weg d (GH #1162): `browser:display` needs only its lock binaries (about 12 s
# of warm build on build04), not the whole `build` (192 s in A2). Phase 1
# `build:browser` builds exactly them, in the call of the full build that builds
# them (`executor.build_argvs` `restrict`: same `-p` set, features, profile);
# phase 2 `build` follows under the same run lock beside the browser. The
# browser waits for no precheck: a red one ends it (`executor._precheck_stops`).
EARLY_BUILD = "build:browser"
EARLY_CONSUMER = "browser:display"

# GH #1191 (welle-tb S1e, K4): the station of `gate_plan.ALONE_LOCKS`. gh1002
# asserts 6 s to `phx-connected` on Fast 3G with the CPU slowed 4x; in the shared
# Chromium part, beside `tests` and the WebKit part, it measured the lane's load
# -- 6 of 10 isolated runs green on build04 (gate bd490c7ec). `split_alone` moves
# the lock out of `browser:display` after every narrowing (where it is planned
# stays the resolver's and the narrowing's decision); the station carries
# `alone: true`, so the executor books it above any lane (`executor._needs`): it
# starts on an empty lane and nothing starts beside it. Its triggers and stages
# are those of the station it comes from (`PARENT`).
ALONE = "browser:alone"
PARENT = {ALONE: "browser:display"}

# Edges besides `precheck`, `build` and `export-audit` (V0 section 4).
AFTER = {
    "corpus": ("corpus-committed",),
    "slow-tests": ("tests",),
    "recall-harness": ("corpus",),
    "export-tree-tests": ("export-selftest",),
    # never ahead of `build`: the tests wait for build, nobody waits for this
    "crate-isolation": ("build",),
}

# Inputs beside the resolver's trigger classes. A station whose reads cannot be
# named reads the whole tree (`glob:**`): over-declaration is safe, it only
# costs cache hits; under-declaration is a stale GREEN.
EXTRA_INPUTS = {
    "precheck": ("glob:**",),
    "fmt": ("glob:rustfmt.toml", "glob:.rustfmt.toml"),
    "clippy": ("glob:clippy.toml", "glob:.clippy.toml"),
    "tests": ("workspace", "glob:.config/nextest.toml"),
    "scenario-tests": ("workspace", "glob:.config/nextest.toml"),
    "slow-tests": ("workspace", "glob:.config/nextest.toml", "glob:scripts/slow_tests.py"),
    # GH #1161 (re-check M-3): it runs the browser units `tests` gave up, so it
    # takes `tests`' triggers too (a workspace diff plans `all()`, which reaches them)
    "browser:display": ("workspace", "glob:.config/nextest.toml"),
    "browser:alone": ("workspace", "glob:.config/nextest.toml"),
    "gate-selftest": ("glob:scripts/**",),
}
WHOLE_TREE = ("glob:**",)

TOOLS_RUST = ("tool:rustc",)
TOOLS = {
    "tests": ("tool:rustc", "tool:nextest"),
    "scenario-tests": ("tool:rustc", "tool:nextest"),
    "slow-tests": ("tool:rustc", "tool:nextest", "tool:python"),
    "browser:display": ("tool:rustc", "tool:nextest", "tool:node", "tool:playwright"),
    "browser:alone": ("tool:rustc", "tool:nextest", "tool:node", "tool:playwright"),
    "build": ("tool:rustc", "tool:nextest"),
}

DEFAULT_TIMEOUT_MIN = {
    "build": 60, "tests": 60, "scenario-tests": 30, "slow-tests": 30, "clippy": 30, "doctests": 30,
    "fmt": 5, "deny": 10, "deny-advisories": 10, "unwrap-budget": 10,
    "scenarios:memory": 60, "scenarios:builder": 45, "scenarios:display": 20,
    "browser:display": 45, "recall-harness": 60, "persona-cases": 30,
    "export-tree-tests": 90, "export-audit": 60, "gate-selftest": 20,
    "crate-isolation": 30,
    "build:browser": 30,
    "browser:alone": 15,
}
TIMEOUT_DEFAULT = 10
TIMEOUT_MIN, TIMEOUT_MAX = 5, 120
HANG_FACTOR = 3   # TV-K5: a cap is at least 3x the P95 of the station's duration


def hang_cap_min(p95_s):
    """The cap in minutes for a P95 of `p95_s` seconds: 3x, clamped to
    [TIMEOUT_MIN, TIMEOUT_MAX]; None without a P95."""
    if p95_s is None:
        return None
    return max(TIMEOUT_MIN, min(TIMEOUT_MAX, int(math.ceil(float(p95_s) * HANG_FACTOR / 60.0))))

# The seconds a station without a measurement ranks and reserves with (`prior_s`,
# GH #1162 OR-TB.S4.38): a measured duration, its source beside it. A measurement in
# the store (`schedule.measured_secs` -> `calib_s`) beats it; a station without a prior
# keeps its timeout as the fallback (OR-TB.S4.21). Without one `crate-isolation` ranked
# by its 30 min timeout first in the first release and pushed `tests` back (V6 sim
# +32.8 s warm A3, +103.4 s A2).
PRIOR_S = {
    # S6-ISO, the cold value: an empty target, j4, build01/12, base cf126ffc8, 273.3 s
    # (warm, a colony diff: 51.8 s) -- plans/welle-tempo-beweis-2026-10-10/berichte/tb-s6.md
    # "Stand 10.10. 11:36", raw data receipts/s6-diag/iso/steps.tsv
    "crate-isolation": 273.3,
    # GH #1191, an estimate until the store has a measurement: one test (gh1002, Fast 3G
    # with the CPU slowed 4x, connect bound 6 s) plus the nextest start; by its 15 min
    # timeout the station ranked like the path to `slow-tests` and held an empty lane
    # up to RESERVE_AGE_S before `tests`
    "browser:alone": 60,
}

STAGE_OF_MODE = {"strand": "S", "integration": "I", "release": "R", "ci": "C"}
EMPTY_TREE = "4b825dc642cb6eb9a060e54bf8d69288fbee4904"
REPO = "meclaw"


def browser_workers(nproc, cargo_jobs):
    """VORGABEN TV-B7: browser workers on `nproc` threads beside `cargo_jobs` build jobs,
    max(1, floor((nproc - cargo_jobs) / 2)) -- more pages than cores time out for no
    reason [PwCI]. For `browser:display` nproc is its booked `cpu` and no cargo job runs
    inside that share (admission books cargo stations beside it), so the Chromium group
    of `.config/nextest.toml` is this over `NEEDS["browser:display"]`; WebKit stays serial.
    Held by scripts/tests/test_pipeline_lab_width.py (TestBrowserWorkerFormula)."""
    return max(1, (int(nproc) - int(cargo_jobs)) // 2)


def selftest_shards(nproc, env=None):
    """Shards of `gate-selftest` on a lane with `nproc` threads: `MECLAW_SELFTEST_SHARDS`
    when set (an ASCII integer >= 1, else ValueError), else min(4, nproc)."""
    env = os.environ if env is None else env
    raw = (env.get(SELFTEST_SHARDS_ENV) or "").strip()
    if raw:
        if not (raw.isascii() and raw.isdigit()) or int(raw) < 1:
            raise ValueError("%s=%r is not an integer >= 1" % (SELFTEST_SHARDS_ENV, raw))
        return int(raw)
    return max(1, min(SELFTEST_SHARDS_DEFAULT, int(nproc)))


def resolve_cpu(value, nproc):
    """`needs.cpu` in threads of a lane with `nproc` threads (at least 1, at most nproc)."""
    nproc = max(1, int(nproc))
    if value == CPU_LANE:
        n = nproc
    elif value == CPU_SHARDS:
        try:
            n = selftest_shards(nproc)
        except ValueError:  # the executor starts the booked default, never the whole lane
            n = selftest_shards(nproc, {})
    else:
        n = int(value)
    return max(1, min(n, nproc))


def resolve_colony(value, colonies):
    """`needs.colony` in slots of a lane with `colonies` colonies: `COLONY_REST` is what
    `browser:display` leaves of the slots the executor can hand out there -- the lane's
    colonies, at most the port register (`ports.SLOT_COUNT`) -- at least 1; a count
    stays as booked -- one the lane cannot serve is RED at the executor, never quietly
    narrowed. Re-check tb-s3 n1 M-neu-1: `cores=12` (build04) gave 12 - 3 = 9 over the
    register's 8, RED before start while the admission booked it."""
    if value == COLONY_REST:
        from . import ports
        slots = min(int(colonies), ports.SLOT_COUNT)
        return max(1, slots - int(NEEDS["browser:display"]["colony"]))
    return int(value)


def resolve_needs(doc, nproc, colonies=None):
    """A copy of plan `doc` with every `needs.cpu` resolved for `nproc` threads and,
    when the lane's `colonies` are known, every `needs.colony` (else a symbol stays
    and the executor refuses it)."""
    out = dict(doc)
    out["stations"] = []
    for st in doc.get("stations") or []:
        st = dict(st)
        needs = dict(st.get("needs") or {})
        if "cpu" in needs:
            needs["cpu"] = resolve_cpu(needs["cpu"], nproc)
        if colonies is not None and "colony" in needs:
            needs["colony"] = resolve_colony(needs["colony"], colonies)
        st["needs"] = needs
        out["stations"].append(st)
    return out


# --- the diff ---------------------------------------------------------------

def _git(root, *args, check=True):
    proc = subprocess.run(["git", "-C", str(root), *args], capture_output=True, text=True)
    if check and proc.returncode != 0:
        raise ValueError("git %s: %s" % (" ".join(args), proc.stderr.strip()))
    return proc


def _dirty_files(root):
    """Tracked and untracked dirty paths, both sides of a rename (gate.sh's
    `meclaw_dirty_files`)."""
    out = _git(root, "status", "--porcelain=v1", "-z", "--untracked-files=all",
               check=False).stdout
    entries = out.split("\0")
    paths, i = [], 0
    while i < len(entries):
        entry = entries[i]
        i += 1
        if len(entry) < 4:
            continue
        paths.append(entry[3:])
        if entry[0] in "RC" or entry[1] in "RC":
            if i < len(entries) and entries[i]:
                paths.append(entries[i])
            i += 1
    return paths


def _github_main_base(root):
    from . import versions   # the one parser of the export subject (Re-Check v2-fix-bump M2)
    subject = _git(root, "log", "-1", "--format=%s", "github-main", check=False).stdout
    source = versions.export_source(subject)
    if not source:
        raise ValueError("cannot read the export base from the github-main ref; "
                         "pass --base <sha>")
    proc = _git(root, "rev-parse", "--verify", "--quiet", source + "^{commit}", check=False)
    if proc.returncode != 0:
        raise ValueError("github-main names %s, which is not a commit here" % source)
    return proc.stdout.strip()


def resolve_base(repo_root, mode, base):
    """`(base, full_scope)` the way `scripts/gate.sh` settles them."""
    if not base:
        if mode == "strand":
            rev = _git(repo_root, "rev-parse", "HEAD").stdout.strip()
            base = _git(repo_root, "merge-base", "master", "HEAD", check=False).stdout.strip()
            if not base or base == rev:
                if _dirty_files(repo_root):
                    base = rev
                else:
                    proc = _git(repo_root, "rev-parse", "--verify", "--quiet", "HEAD~1",
                                check=False)
                    base = proc.stdout.strip() if proc.returncode == 0 else EMPTY_TREE
        elif mode in ("integration", "release"):
            base = _github_main_base(repo_root)
        elif mode == "ci":
            return "", True
    if mode == "ci" and base:
        known = _git(repo_root, "rev-parse", "--verify", "--quiet", base + "^{commit}",
                     check=False).returncode == 0
        if set(base) == {"0"} or not known:
            return "", True
    return base or "", False


def changed_files(repo_root, mode, base):
    """The changed paths of a pass, sorted: `git diff --name-only <base> HEAD`
    plus the dirty tree, or every tracked file when the scope is full (ci
    without a usable base) -- the same list `scripts/gate.sh` plans from."""
    base, full = resolve_base(repo_root, mode, base)
    if full:
        return sorted(set(_git(repo_root, "ls-files").stdout.splitlines()))
    diff = []
    if base:
        # Both sides of a rename (re-check tb-s2 h1 m1): a Rust file moved under
        # plans/ is still a Rust change, like `_dirty_files`.
        diff = _git(repo_root, "diff", "--name-only", "--no-renames", base, "HEAD",
                    check=False).stdout.splitlines()
    return sorted({p for p in diff + _dirty_files(repo_root) if p})


# --- the resolver -------------------------------------------------------------

def _gate_plan_path(repo_root):
    own = os.path.join(str(repo_root), "scripts", "gate_plan.py")
    if os.path.exists(own):
        return own
    return os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
                        "gate_plan.py")


# `gate_plan.RULE8_INDEX_ENV`: the file through which the resolver subprocess
# hands its rule-8 index to this process (tb-s10 w6, A1 11 L-8).
RULE8_INDEX_ENV = "GATE_PLAN_RULE8_INDEX"


def _drop_index_file(tmpdir, path):
    for p in (path, tmpdir):
        try:
            (os.rmdir if p == tmpdir else os.unlink)(p)
        except OSError:
            pass


def _rule8_index_file():
    """The hand-over file of this process (named once, in a private temp dir,
    removed at exit); a gate started inside another one shares the outer one."""
    path = os.environ.get(RULE8_INDEX_ENV)
    if not path:
        tmpdir = tempfile.mkdtemp(prefix="gate-plan-rule8-")
        path = os.environ[RULE8_INDEX_ENV] = os.path.join(tmpdir, "index.pickle")
        atexit.register(_drop_index_file, tmpdir, path)
    return path


def default_resolver(repo_root, mode, files):
    """`gate_plan.py --format json --explain` of the planned tree, as a
    subprocess. The rule-8 index it builds reaches this process through
    `_rule8_index_file` -- one build per gate, not two."""
    _rule8_index_file()
    proc = subprocess.run(
        [sys.executable, _gate_plan_path(repo_root), "--mode", mode, "--format", "json",
         "--explain", "--repo", str(repo_root), "--files-from", "-"],
        input="".join(p + "\n" for p in files), capture_output=True, text=True)
    if proc.returncode != 0:
        raise ValueError("gate_plan.py failed: %s" % proc.stderr.strip())
    return json.loads(proc.stdout)


_GP = None


def _gp():
    """The resolver module next to this package (its tables: triggers, ci exclusions)."""
    global _GP
    if _GP is None:
        path = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
                            "gate_plan.py")
        spec = importlib.util.spec_from_file_location("pipeline_gate_plan", path)
        mod = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(mod)
        _GP = mod
    return _GP


# --- cargo metadata: nextest binary ids -----------------------------------------

def _members(metadata):
    ids = set(metadata.get("workspace_members") or [])
    return [p for p in metadata.get("packages") or [] if p.get("id") in ids]


def _binary_id(pkg, target):
    kinds = target.get("kind") or []
    name = pkg["name"]
    if "bin" in kinds:
        return "%s::bin/%s" % (name, target["name"])
    if "test" in kinds:
        return "%s::%s" % (name, target["name"])
    if any(k in kinds for k in ("lib", "rlib", "dylib", "proc-macro", "cdylib", "staticlib")):
        return name
    return None


def _binaries(pkg, kinds=("lib", "bin", "test")):
    """`[(binary_id, target_name)]` of the nextest binaries of `pkg`."""
    out = []
    for t in pkg.get("targets") or []:
        if t.get("test") is False:
            continue
        tk = t.get("kind") or []
        is_lib = any(k in tk for k in ("lib", "rlib", "dylib", "proc-macro", "cdylib",
                                        "staticlib"))
        if ("lib" in kinds and is_lib) or ("bin" in kinds and "bin" in tk) \
                or ("test" in kinds and "test" in tk):
            bid = _binary_id(pkg, t)
            if bid:
                out.append((bid, t["name"]))
    return out


def _rdeps(metadata, crates):
    """The workspace packages that depend on any of `crates`, directly or not, and
    those crates themselves (nextest `rdeps()`; dev-dependencies count)."""
    members = {p["name"]: p for p in _members(metadata)}
    users = {name: set() for name in members}
    for p in members.values():
        for dep in p.get("dependencies") or []:
            if dep.get("name") in members:
                users[dep["name"]].add(p["name"])
    seen, todo = set(), [c for c in crates if c in members]
    while todo:
        c = todo.pop()
        if c in seen:
            continue
        seen.add(c)
        todo.extend(users.get(c, ()))
    return sorted(seen)


def _split(bid):
    crate, _, stem = bid.partition("::")
    return crate, stem


def select_parts(select, metadata):
    """`(parts, build)` of one nextest selection (`select` from `--explain`).

    `build` holds the cargo targets: `tests`/`extra` as [crate, binary],
    `packages` for `rdeps()`, `lib_bins` for the t0 floor, `workspace` for
    `all()`. Without metadata `rdeps()` builds the workspace, the floor
    builds `--workspace --lib --bins`, and such a selection has no parts."""
    build = {"workspace": False, "packages": [], "tests": [], "lib_bins": [], "extra": []}
    explicit = sorted(set(select.get("binaries") or []))
    extra = sorted(set(select.get("extra") or []))
    minus = [re.compile(m) for m in select.get("minus") or []]
    parts = set(explicit) | set(extra)
    resolved = True
    if metadata is None:
        if select.get("all") or select.get("rdeps"):
            build["workspace"] = True
            resolved = False
        if select.get("floor"):
            build["lib_bins"] = True
            resolved = False
        if select.get("compile"):   # narrowed rdeps: built, not run (review Z-I1 I2)
            build["workspace"] = True
    else:
        members = _members(metadata)
        # A named binary the workspace does not have cannot be built (`--test`
        # fails) nor run: the resolver's lock list is wider than a tree.
        known = {b for p in members for b, _n in _binaries(p)}
        explicit = [b for b in explicit if b in known]
        extra = [b for b in extra if b in known]
        parts = set(explicit) | set(extra)
        if select.get("all"):
            build["workspace"] = True
            for p in members:
                parts |= {b for b, n in _binaries(p) if not any(m.search(n) for m in minus)}
        if select.get("compile"):
            # The test binaries of a narrowed rdeps(X) are built, not run: a
            # compile break in a user crate stays red (review Z-I1 I2).
            build["packages"] = _rdeps(metadata, select["compile"])
        if select.get("rdeps"):
            build["packages"] = sorted(set(build["packages"]) | set(_rdeps(metadata, select["rdeps"])))
            for p in members:
                if p["name"] in build["packages"]:
                    parts |= {b for b, n in _binaries(p)
                              if not any(m.search(n) for m in minus)}
        if select.get("floor"):
            build["lib_bins"] = sorted(p["name"] for p in members)
            for p in members:
                parts |= {b for b, _n in _binaries(p, ("lib", "bin"))}
    whole = set(build["packages"])
    build["tests"] = [list(_split(b)) for b in explicit if _split(b)[0] not in whole]
    build["extra"] = [list(_split(b)) for b in extra]
    return (sorted(parts) if resolved else []), build


# --- enrichment ----------------------------------------------------------------

def _cmds(row):
    """V0 `[{argv, cwd}]` from the legacy `cmd`/`cwd` or `cmds`/`cwds`."""
    if "cmd" in row:
        return [{"argv": list(row["cmd"]), "cwd": row.get("cwd") or ""}]
    cwds = row.get("cwds") or []
    return [{"argv": list(argv), "cwd": (cwds[i] if i < len(cwds) else "") or ""}
            for i, argv in enumerate(row.get("cmds") or [])]


def _needs(name, row, mode):
    needs = dict.fromkeys(NEEDS_KEYS, 0)
    needs["cpu"] = 1
    if name == "export-audit":
        needs.update(EXPORT_AUDIT_NEEDS.get(mode, EXPORT_AUDIT_NEEDS["integration"]))
    else:
        needs.update(NEEDS.get(name, {}))
    if row.get("cargo") and name not in REUSE and name not in OWN_TARGET:
        needs["cargo"] = 1
    if name in REUSE:
        needs["cargo"] = 0
    return {k: int(v) for k, v in needs.items()}


def _stage(name):
    gp = _gp()
    name = PARENT.get(name, name)
    spec = gp.STATION_TRIGGERS.get(name)
    letters = ["S", "I", "R", "C"]
    if spec is not None and not spec[0]:
        letters = [STAGE_OF_MODE[m] for m in spec[1] if m in STAGE_OF_MODE]
    if name in gp.CI_EXCLUDED:
        letters = [x for x in letters if x != "C"]
    return letters


def _inputs(name, row):
    gp = _gp()
    spec = gp.STATION_TRIGGERS.get(PARENT.get(name, name))
    out = []
    if spec is not None:
        out += [label for label in spec[0] if label != "diff"]
    out += list(EXTRA_INPUTS.get(name, ()))
    if name in NEXTEST:
        # Paths outside the crates that the selection reads (rule 8, templates,
        # examples) are not in any binary's source closure: declare them.
        for reason in row.get("reason") or []:
            label, sep, path = reason.partition(": ")
            if sep and path and not path.startswith("crates/"):
                out.append("glob:" + path)
    if not out:
        out = list(WHOLE_TREE)
    if name in MECLAW_BIN:
        out.append("artefact:meclaw")
    tools = TOOLS.get(name)
    if tools is None:
        # the program and the Python form of a cmd: lane's one reader each
        # (re-check tv-bn M7p M1, M7q H1: past an `env NAME=value` prefix)
        from . import lane
        cmds = [c["argv"] for c in _cmds(row) if c["argv"]]
        tools = TOOLS_RUST if any(lane.program(a) == "cargo" for a in cmds) else ()
        if any(lane.python_rest(a) is not None for a in cmds):
            tools = tuple(tools) + ("tool:python",)
    seen, ordered = set(), []
    for item in out + list(tools):
        if item not in seen:
            seen.add(item)
            ordered.append(item)
    return ordered


def _timeout(name, calibration):
    cal = None
    for key in ("%s@%s" % (name, REPO), name):
        if calibration and key in calibration:
            cal = calibration[key]
            break
    if cal is None:
        return DEFAULT_TIMEOUT_MIN.get(name, TIMEOUT_DEFAULT)
    if isinstance(cal, dict):
        p95 = cal.get("p95_s") if cal.get("p95_s") is not None else cal.get("p90_s")
    else:
        p95 = cal
    if p95 is None:
        return DEFAULT_TIMEOUT_MIN.get(name, TIMEOUT_DEFAULT)
    return hang_cap_min(p95)


def _seconds(st, calibration):
    for key in ("%s@%s" % (st["name"], REPO), st["name"]):
        cal = (calibration or {}).get(key)
        if isinstance(cal, dict):
            for field in ("p50_s", "p90_s"):
                if cal.get(field) is not None:
                    return float(cal[field])
    return float(st["timeout_min"]) * 60.0


def _station(row, mode, calibration, metadata):
    name = row["name"]
    st = {k: v for k, v in row.items() if k not in ("cmds", "env")}
    st["cmds"] = _cmds(row)
    st["env"] = "lane" if row.get("env") else "none"
    st["reason"] = list(row.get("reason") or [])
    st["stage"] = _stage(name)
    st["needs"] = _needs(name, row, mode)
    if name == ALONE:
        st["alone"] = True
    st["inputs"] = _inputs(name, row)
    st["timeout_min"] = _timeout(name, calibration)
    if name in PRIOR_S:
        st["prior_s"] = PRIOR_S[name]
    st["run"] = row.get("run", True) is not False
    st["keep_ws"] = name in NEXTEST
    st["after"] = list(AFTER.get(name, ()))
    st["when"] = {"outcome": "success"}
    build = None
    if name in NEXTEST:
        select = row.get("select")
        if select is not None:
            parts, build = select_parts(select, metadata)
            st["parts"] = parts
            nextest_filter = select.get("filter") or row.get("scope", "")
        else:
            st["parts"] = []
            nextest_filter = row.get("scope", "")
        st["nextest"] = {"profile": NEXTEST[name], "filter": nextest_filter}
    return st, build


def _merge_build(builds):
    out = {"meclaw_bin": False, "workspace": False, "packages": [], "tests": [],
           "lib_bins": [], "extra": []}
    for b in builds:
        out["workspace"] = out["workspace"] or bool(b.get("workspace"))
        out["packages"] = sorted(set(out["packages"]) | set(b.get("packages") or []))
        if b.get("lib_bins") is True or out["lib_bins"] is True:
            out["lib_bins"] = True
        else:
            out["lib_bins"] = sorted(set(out["lib_bins"]) | set(b.get("lib_bins") or []))
        for key in ("tests", "extra"):
            for pair in b.get(key) or []:
                if list(pair) not in out[key]:
                    out[key].append(list(pair))
    for key in ("tests", "extra"):
        out[key].sort()
    return out


def _build_station(consumers, builds, mode, calibration, metadata=None):
    build = _merge_build(builds)
    build["meclaw_bin"] = any(c["name"] in MECLAW_BIN for c in consumers)
    if metadata is not None:
        # the test targets of every crate with named binaries: the executor
        # merges their calls only where no name clashes (`_joint_groups`, #1166)
        named = {c for c, _b in build["tests"] + build["extra"]}
        build["test_targets"] = {
            p["name"]: sorted(t["name"] for t in p.get("targets") or []
                              if "test" in (t.get("kind") or []))
            for p in _members(metadata) if p["name"] in named}
    from .executor import build_argvs
    argvs = [["cargo"] + a for a in build_argvs(build, set())]
    scope = []
    if build["meclaw_bin"]:
        scope.append("bin meclaw")
    if build["workspace"]:
        scope.append("workspace")
    scope += ["%s::%s" % tuple(p) for p in build["tests"] + build["extra"]]
    scope += build["packages"]
    if build["lib_bins"]:
        scope.append("lib+bins")
    st = {
        "name": "build", "scope": ", ".join(scope) or "none", "cargo": True,
        "cmds": [{"argv": a, "cwd": ""} for a in argvs], "env": "none",
        "reason": ["station: %s" % c["name"] for c in consumers],
        "stage": ["S", "I", "R", "C"], "needs": _needs("build", {"cargo": True}, mode),
        "inputs": ["workspace", "glob:crates/**", "tool:rustc", "tool:nextest"],
        "timeout_min": _timeout("build", calibration), "run": True, "keep_ws": True,
        "after": [], "when": {"outcome": "success"}, "build": build,
    }
    if build["meclaw_bin"]:
        st["inputs"].append("artefact:meclaw")
    return st


def _early_build(main, consumer, pairs, mode, calibration):
    """Phase 1 of the build: the targets of `consumer` only (GH #1162 Weg d)."""
    build = dict(main["build"], meclaw_bin=False, restrict=[list(p) for p in pairs])
    from .executor import build_argvs
    argvs = [["cargo"] + a for a in build_argvs(build, set())]
    return {
        "name": EARLY_BUILD, "scope": ", ".join("%s::%s" % tuple(p) for p in pairs),
        "cargo": True, "cmds": [{"argv": a, "cwd": ""} for a in argvs], "env": "none",
        "reason": ["station: %s" % consumer], "stage": list(main["stage"]),
        "needs": dict(main["needs"]),
        "inputs": [i for i in main["inputs"] if i != "artefact:meclaw"],
        "timeout_min": _timeout(EARLY_BUILD, calibration), "run": True, "keep_ws": True,
        "after": [], "when": {"outcome": "success"}, "build": build,
    }


def _toposort(stations):
    """Stable topological order over `after`; ValueError on a cycle."""
    index = {s["name"]: i for i, s in enumerate(stations)}
    pending = {s["name"]: {a for a in s["after"] if a in index} for s in stations}
    out = []
    while pending:
        ready = sorted((n for n, deps in pending.items() if not deps), key=index.get)
        if not ready:
            raise ValueError("cycle in the plan: %s" % sorted(pending))
        n = ready[0]
        out.append(n)
        del pending[n]
        for deps in pending.values():
            deps.discard(n)
    by = {s["name"]: s for s in stations}
    return [by[n] for n in out]


def critical_path(stations, secs_by_station):
    """`(seconds, [names])` of the longest chain over the `after` edges.

    Edges to stations outside `stations` are ignored; a cycle is a ValueError."""
    by = {s["name"]: s for s in stations}
    memo, state = {}, {}

    def walk(name):
        if state.get(name) == "done":
            return memo[name]
        if state.get(name) == "open":
            raise ValueError("cycle in the plan at %s" % name)
        state[name] = "open"
        best = (0.0, [])
        for dep in by[name].get("after") or []:
            if dep in by:
                cand = walk(dep)
                if cand[0] > best[0]:
                    best = cand
        memo[name] = (best[0] + float(secs_by_station.get(name, 0.0)), best[1] + [name])
        state[name] = "done"
        return memo[name]

    best = (0.0, [])
    for s in stations:
        cand = walk(s["name"])
        if cand[0] > best[0]:
            best = cand
    return float(best[0]), best[1]


def _narrow_on():
    """The narrowing switch (`MECLAW_GATE_NARROW`): off, an integration plans
    the resolver floor as before #1159 -- no wave decision, no full suite."""
    from . import narrow as narrow_mod
    return narrow_mod.enabled()


def build_plan(repo_root, mode, base, files=None, only=None, calibration=None, *,
               resolver=None, metadata=None, narrow=False, stage=None, strand_run=None,
               wave=None, db=None, full=None, now=None):
    """The enriched plan of one pass (see the module docstring).

    `now` is the clock the narrowing judges a coverage map's age by (default:
    the wall clock); the kit's self-tests hand their fixture time (GH #1199).

    `mode` "merge" (or `stage="merge"`) is the merge gate: a strand pass on
    the merge commit that narrows only after `narrow.merge_decision`
    (`strand_run` = the strand receipt's store run). An integration with
    `narrow` plans its tests from the wave diff after
    `narrow.integration_decision` (`wave`), else the full suite (KR-TB-1).
    `db` is the store the decisions read (default: the store, read-only).
    `full` (`full_pass.for_plan`) plans a full pass instead of any narrowing.

    -> {mode, rev, base, repo, classes, crates, stations, plan_reason,
        critical_path_s, critical_path[, only, stage, notes, full_pass]}"""
    repo_root = str(repo_root)
    if mode == "merge":
        mode, stage = "strand", "merge"
    if files is None:
        base_resolved, _full = resolve_base(repo_root, mode, base)
        files = changed_files(repo_root, mode, base)
        base = base_resolved
    # the release guard behind `split_alone` reads the real tree's units, so it
    # runs behind the real resolver only (a test's fake resolver plans a fake tree)
    real_resolver = resolver is None or resolver is default_resolver
    resolver = resolver or default_resolver
    raw = resolver(repo_root, mode, list(files))
    notes = []
    scope = None   # re-check H2 M1: what the tests of a merge/integration are
    graph_info = {}   # welle-validierung-2 3a: a graph the narrowing built (receipt)
    if full:  # a full pass after odd effects, or `--full` (welle-tb #1165)
        from . import full_pass
        notes = full_pass.apply(raw.get("stations") or [], mode, repo_root, full)
        if mode == "integration" or stage == "merge":   # the SCOPE line names it (H2 M1)
            scope = ("integration" if mode == "integration" else "merge", None, None,
                     wave if mode == "integration" else None, "full pass (#1165)")
    elif narrow and stage == "merge" and mode == "strand":  # welle-tb S1 part B (#1159)
        from . import narrow as narrow_mod
        db = db if db is not None else narrow_mod._open_store()
        term, since, notes = narrow_mod.merge_decision(repo_root, base, strand_run, db)
        scope = ("merge", term, since, None, "; ".join(notes) or None)
        if term:
            wide = sorted(set(files) | set(changed_files(repo_root, "strand", since)))
            notes = narrow_mod.apply(raw.get("stations") or [], wide, repo_root,
                                     metadata=metadata, stage="merge", narrowed=term,
                                     graph_info=graph_info, now=now)
    elif narrow and mode == "strand":  # OR-DT.W6.5 / #1138: the gate's run switch
        from . import narrow as narrow_mod
        notes = narrow_mod.apply(raw.get("stations") or [], list(files), repo_root,
                                 metadata=metadata, graph_info=graph_info, now=now)
    elif narrow and mode == "integration" and _narrow_on():  # welle-tb S1 part C, KR-TB-1
        from . import narrow as narrow_mod
        db = db if db is not None else narrow_mod._open_store()
        term, since, why = narrow_mod.integration_decision(repo_root, wave, db)
        scope = ("integration", term, since, wave, "; ".join(why) or None)
        rows = raw.get("stations") or []
        if term is None:
            notes = narrow_mod.full_suite(rows, repo_root, why)
        else:
            wfiles = changed_files(repo_root, "integration", since)
            wrow = narrow_mod._tests_row(resolver(repo_root, mode, list(wfiles))
                                         .get("stations") or [])
            row = narrow_mod._tests_row(rows)
            if wrow is None:
                # the wave diff plans no test: the export diff's row stays (more, never less)
                notes = ["narrowing: %s -- the wave diff plans no test, the resolver row "
                         "stays" % term]
            else:
                if row is None:   # a wave start older than the export base
                    rows.append(wrow)
                else:
                    old = (row.get("select") or {}).get("filter")
                    for k in ("select", "scope", "cmd", "cmds", "reason"):
                        if k in wrow:
                            row[k] = wrow[k]
                    if old and "cmd" not in wrow and "cmds" not in wrow:
                        narrow_mod._swap_filter(row, old, wrow["select"].get("filter"))
                notes = narrow_mod.apply(rows, list(wfiles), repo_root, metadata=metadata,
                                         stage="integration", narrowed=term,
                                         graph_info=graph_info, now=now)
            from . import browser_plan   # S1b ruling 10.10. point 2: locks the wave reaches
            notes += browser_plan.apply(rows, list(wfiles), repo_root, metadata=metadata, now=now)
            # tb-s3-b: the cut station takes the wave's units only -- a unit the
            # narrowed `tests` reaches beyond them joins it again; a lock it cut
            # is the ruling's (locks=False)
            notes += narrow_mod.plan_browser_orphans(rows, repo_root, locks=False)
            narrow_mod.refuse_unrun(rows, repo_root, locks=False)   # GH #1182
    elif mode == "integration":
        scope = ("integration", None, None, wave,
                 "narrowing off (narrow=False or MECLAW_GATE_NARROW=0)")
    if mode == "release":
        notes += _release_full_suite(raw.get("stations") or [], repo_root)
    narrowing = _narrowing(raw.get("stations") or [], *scope, notes=notes) if scope else None
    notes = list(notes or []) + split_alone(raw.get("stations") or [])
    if mode == "release" and real_resolver:
        release_guard(raw.get("stations") or [], repo_root)
    stations, builds, consumers, build_of = [], [], [], {}
    for row in raw.get("stations") or []:
        st, build = _station(row, mode, calibration, metadata)
        stations.append(st)
        if build is not None:
            builds.append(build)
            build_of[st["name"]] = build
        if st["name"] in BUILD_CONSUMERS:
            consumers.append(st)
    if consumers:
        b = _build_station(consumers, builds, mode, calibration, metadata)
        first = min(i for i, s in enumerate(stations) if s["name"] in BUILD_CONSUMERS)
        stations.insert(first, b)
        early_pairs = (build_of.get(EARLY_CONSUMER) or {}).get("extra") or []
        for c in consumers:
            if c["name"] == EARLY_CONSUMER and early_pairs:
                c["after"].insert(0, EARLY_BUILD)
            else:
                c["after"].insert(0, "build")
        if early_pairs and any(c["name"] == EARLY_CONSUMER for c in consumers):
            stations.insert(first, _early_build(b, EARLY_CONSUMER, early_pairs, mode,
                                                calibration))
            b["after"].insert(0, EARLY_BUILD)
            # Phase 2 runs whatever phase 1 says: a compile break in phase 1
            # breaks phase 2 too and stays RED there, never a consumer without
            # a build (the executor skips only after RED/ABORTED).
            b["when"] = {"outcome": "any"}
    names = [s["name"] for s in stations]
    builds_named = {s["name"] for s in stations if s.get("build") is not None}
    if "precheck" in names:
        # `build` runs beside `precheck` (GH #1162: 34 s serial before every
        # build of the integration pass 7b1e0e5); a red precheck still stops it
        # -- the executor ends a running build SKIP `precheck red` -- and every
        # consumer of the build keeps its edge, so nothing judges over a red one.
        # The consumer of phase 1 has no edge either (Weg d): it starts on the
        # early build, and a red precheck ends it as it ends the build.
        for s in stations:
            if s["name"] == "precheck" or s["name"] in builds_named \
                    or EARLY_BUILD in s["after"]:
                continue
            if s["needs"]["cargo"] or "build" in s["after"]:
                s["after"].insert(0, "precheck")
    for s in stations:
        s["after"] = [a for a in dict.fromkeys(s["after"]) if a in names and a != s["name"]]
    if "export-audit" in names:
        audit = next(s for s in stations if s["name"] == "export-audit")
        audit["after"] = [n for n in names if n in AUDIT_READS or n in AUDIT_AFTER_ALSO
                          or n in audit["after"]]
        audit["when"] = {"outcome": "any"}
    stations = _toposort(stations)
    if only:
        only = list(only)
        known = set(_gp().STATION_ORDER) | {"build", EARLY_BUILD, ALONE}
        unknown = [n for n in only if n not in known]
        if unknown:
            raise ValueError("unknown station(s) in only: %s" % ", ".join(unknown))
        wanted = set(only)
        # what `split_alone` moved out of a named station goes along with it
        wanted |= {a for a, parent in PARENT.items() if parent in wanted}
        # A named station takes the build it waits for along (phase 1 or 2).
        wanted |= {a for s in stations if s["name"] in wanted
                   for a in s["after"] if a in builds_named}
        for s in stations:
            if s["name"] not in wanted:
                s["run"] = False
        notes = notes + _only_cut_browser(stations, wanted)
    run = [s for s in stations if s["run"]]
    secs = {s["name"]: _seconds(s, calibration) for s in run}
    cp_s, cp = critical_path(run, secs)
    rev = _git(repo_root, "rev-parse", "HEAD", check=False).stdout.strip()
    doc = {
        "mode": mode, "rev": rev, "base": base, "repo": REPO,
        "classes": raw.get("classes", []), "crates": raw.get("crates", []),
        "stations": stations,
        "plan_reason": {s["name"]: list(s["reason"]) for s in stations},
        "critical_path_s": float(cp_s), "critical_path": cp,
    }
    if only:
        doc["only"] = list(only)
    if stage == "merge":
        doc["stage"] = "merge"
    if notes:
        doc["notes"] = notes
    if narrowing is not None:
        doc["narrowing"] = narrowing
    if full:
        doc["full_pass"] = dict(full)
    if graph_info:
        doc["graph_build"] = dict(graph_info)
    return doc


def _filter_terms(argv, gp):
    """(summands, tail) of a `browser:display` command: the terms of its union and
    the engine split behind it (`gate_plan.browser_cmds`, `narrow._with_units`);
    None for a command that is no `test-tier.sh filter`."""
    if argv[:2] != ["scripts/test-tier.sh", "filter"] or len(argv) < 3:
        return None
    from . import narrow as narrow_mod
    filt, tail = argv[2], ""
    for sep in (" - ", " & "):
        if filt.endswith(sep + gp.WEBKIT_TESTS):
            filt, tail = filt[:-len(sep + gp.WEBKIT_TESTS)], sep + gp.WEBKIT_TESTS
            break
    return narrow_mod._top_split(narrow_mod._unwrap(filt), " + "), tail


def split_alone(rows):
    """Move the locks of `gate_plan.ALONE_LOCKS` out of the `browser:display` row of
    `rows` (in place) into a row `ALONE` of their own, right behind it (GH #1191).
    Runs after every narrowing: a lock the narrowed row no longer runs stays out of
    both. A row left without a test leaves the plan (a filter without a test is
    RED, `test-tier.sh` exit 6). Returns the plan notes."""
    import types
    gp = _gp()
    i = next((k for k, r in enumerate(rows) if r.get("name") == "browser:display"), None)
    if i is None:
        return []
    row = rows[i]
    cmds = [row["cmd"]] if "cmd" in row else list(row.get("cmds") or [])
    cwds = [row.get("cwd")] if "cmd" in row else list(row.get("cwds") or [None] * len(cmds))
    split = [_filter_terms(argv, gp) for argv in cmds]
    alone = [t for t in gp.ALONE_LOCKS if any(sp and t in sp[0] for sp in split)]
    if not alone:
        return []
    keep, keep_cwds = [], []
    for argv, cwd, sp in zip(cmds, cwds + [None] * len(cmds), split):
        if sp is None:
            keep.append(argv)
            keep_cwds.append(cwd)
            continue
        left = [t for t in sp[0] if t not in alone]
        if not left:
            continue
        union = " + ".join(left)
        keep.append(argv[:2] + ["(%s)%s" % (union, sp[1]) if sp[1] else union] + argv[3:])
        keep_cwds.append(cwd)
    cmd = ["scripts/test-tier.sh", "filter", " + ".join(alone), "--run-ignored", "all"]
    new = {"name": ALONE, "scope": "alone: %s" % ", ".join(alone),
           "cargo": bool(row.get("cargo")), "cmd": cmd,
           "select": gp._browser_select(types.SimpleNamespace(cmds=[cmd])),
           # its own reason, no trigger path: the row's triggers (a browser unit,
           # a template) are the shared parts'; the lock's sources are its part
           "reason": ["split out of browser:display (GH #1191)"]}
    for k in ("env", "run"):
        if k in row:
            new[k] = row[k]
    if not keep:
        rows[i] = new
        return ["%s: %s alone on the lane, `browser:display` keeps no test and leaves "
                "the plan (GH #1191)" % (ALONE, ", ".join(alone))]
    for k in ("cmd", "cwd", "cmds", "cwds"):
        row.pop(k, None)
    if len(keep) == 1:
        row["cmd"] = keep[0]
        if keep_cwds[0]:
            row["cwd"] = keep_cwds[0]
        row.pop("parallel", None)
    else:
        row["cmds"] = keep
        if any(keep_cwds):
            row["cwds"] = keep_cwds
    row["select"] = gp._browser_select(types.SimpleNamespace(cmds=keep))
    rows.insert(i + 1, new)
    return ["%s: %s alone on the lane, out of `browser:display` (GH #1191)"
            % (ALONE, ", ".join(alone))]


def release_guard(rows, repo_root):
    """The release partition behind every narrowing and the split (fund 27,
    re-check tb-s1e-alone): `gate_plan.plan()` checks the plan before
    `split_alone`, so it never sees `browser:alone`; this reads the rows the
    gate runs -- `tests` + `browser:display` + `browser:alone` + the scenario
    station split `all()`, or the release is refused (`GatePlanError`,
    fail-closed). Returns [] when the rows clear it."""
    import types
    gp = _gp()
    st = {r["name"]: types.SimpleNamespace(
              cmds=[r["cmd"]] if "cmd" in r else list(r.get("cmds") or []))
          for r in rows if r.get("name")}
    gaps, twice = gp.release_partition(st, repo_root)
    if gaps or twice:
        raise gp.GatePlanError(
            "release plan does not split all() behind split_alone (KR-TB-1 d, fund 27): "
            "in no station %s; in two stations %s" % (gaps[:20], twice[:20]))
    return []


def _only_cut_browser(stations, wanted):
    """OR-TB.S10.B.2 (tb-s3 L-2): `tests` subtracts every browser unit (rule
    11) and `browser:display` runs them (orphans included,
    `narrow.plan_browser_orphans`). An `--only` that keeps `tests` and cuts
    `browser:display` runs them in no station -- `--only` is an explicit
    partial run, so the plan keeps the cut and names the units (fail-closed
    visible): one NOTE line, none when `tests` subtracts nothing."""
    names = {s["name"] for s in stations}
    # `split_alone` may have moved every browser lock into ALONE (tb-s1e): the
    # browser units then run there, so either row counts as the browser side.
    browser = names & {"browser:display", ALONE}
    if "tests" not in wanted or not browser or browser <= wanted:
        return []
    from . import narrow as narrow_mod
    row = next(s for s in stations if s["name"] == "tests")
    _head, minus = narrow_mod._split_minus((row.get("nextest") or {}).get("filter"))
    units = []
    for term in minus:
        ids = re.findall(r"binary_id\(=([^)]+)\)", term)
        units += ids or [term]
    if not units:
        return []
    return ["--only cuts browser:display: browser units %s run in no station -- `tests` "
            "subtracts them (OR-TB.S10.B.2)" % ", ".join(units)]


def _release_full_suite(rows, repo_root):
    """The release's `tests` row stays the full suite (KR-TB-1 d, GH #1162
    OR-TB.S4.39): `gate_plan.test_select` plans all() for any release diff, and
    whatever narrowed the row after it is undone here -- by rule, the narrowing
    switch (`MECLAW_GATE_NARROW`) on or off. A row the plan cannot widen (no
    selection) is left as the resolver gave it."""
    from . import narrow as narrow_mod
    row = narrow_mod._tests_row(rows)
    if row is None or row["select"].get("all"):
        return []
    old = row["select"].get("filter")
    row["select"] = narrow_mod.full_select(row["select"], repo_root,
                                           ["release: full suite (KR-TB-1 d)"])
    narrow_mod._swap_filter(row, old, row["select"]["filter"])
    return ["narrowing: release: full suite (KR-TB-1 d) -- the tests row was %s" % old]


def _narrowing(rows, stage, term, since, wave, why, notes=()):
    """`{tests, stage, since, wave, map, why}` of a merge or integration
    (re-check H2 M1): `tests` is what the tests row ended as -- `full-suite`
    (all()), `narrowed` (the stage's narrowing ran on a map, `narrow.on`),
    `floor` (the resolver's row, also when the stage allowed narrowing but no
    map fitted -- re-check m-N1), `none` (no tests station)."""
    row = next((r for r in rows if r.get("name") == "tests"), None)
    sel = (row or {}).get("select") or {}
    if row is None:
        tests = "none"
    elif sel.get("all") or str(sel.get("filter") or "").startswith("all()"):
        tests = "full-suite"
    elif term and (sel.get("narrow") or {}).get("on"):
        tests = "narrowed"
    else:
        # re-check m-N1: the stage allowed it, but no map fitted -- the floor ran
        tests = "floor"
    return {"tests": tests, "stage": stage, "since": since if term else None,
            "wave": wave or None, "map": (sel.get("narrow") or {}).get("map") or None,
            "why": None if tests == "narrowed" else (
                why or "; ".join(w for w in sel.get("why") or [] if w.startswith("narrow"))
                # a merge without a fitting map keeps its row: the say is in the notes
                or "; ".join(n for n in notes or () if str(n).startswith("narrowing"))
                or None)}


# --- command line ----------------------------------------------------------------

def _load_json(path):
    with open(path, encoding="utf-8") as fh:
        return json.load(fh)


def main(argv=None, out=None):
    out = out or sys.stdout
    ap = argparse.ArgumentParser(prog="python3 -m scripts.pipeline.plan",
                                 description="Print the enriched station plan of a pass.")
    ap.add_argument("--mode", required=True)
    ap.add_argument("--base", default=None)
    ap.add_argument("--only", default="", help="comma-separated stations to run")
    ap.add_argument("--repo", default=".", help="the tree to plan (default: cwd)")
    ap.add_argument("--files", nargs="*", default=None,
                    help="plan these paths instead of the diff")
    ap.add_argument("--metadata", default=None, help="`cargo metadata` JSON file")
    ap.add_argument("--calibration", default=None, help="calibration JSON file")
    args = ap.parse_args(argv)
    only = [n for n in args.only.split(",") if n] or None
    from .graph import GraphBuildError  # noqa: PLC0415 -- the narrowing's own module
    try:
        doc = build_plan(os.path.abspath(args.repo), args.mode, args.base, files=args.files,
                         only=only,
                         calibration=_load_json(args.calibration) if args.calibration else None,
                         metadata=_load_json(args.metadata) if args.metadata else None)
    except (ValueError, OSError, GraphBuildError) as exc:
        # GraphBuildError: the graph of the head could not be built -- no plan,
        # one line and exit 2 like `gate: no plan`, never a traceback (c23 M-3)
        print("plan: %s" % exc, file=sys.stderr)
        return 2
    json.dump(doc, out, indent=1)
    out.write("\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
