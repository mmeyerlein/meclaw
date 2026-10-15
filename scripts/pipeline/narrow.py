"""The narrowed test selection of a strand gate (GH #1138).

`pipeline impact` (W6) knew how to replace a crate-level `rdeps(X)` by the
test parts the coverage map says execute the changed files -- and nothing
called it: the gate planned the resolver floor alone, and there was no switch
to turn the narrowing on or off (finding of the after-measurement). This
module is the one place the gate asks `impact`:

    on  (default; unset or anything but 0)
        strand: floor + graph + coverage parts instead of rdeps(X), from the
        newest coverage map in `coverage_dir()` and the latest graph build
    off (MECLAW_GATE_NARROW=0)
        the resolver's selection, unchanged

ON BY DEFAULT since #1126/#1138: map f004e6c36 covers 227/227
binaries and `map_fits` holds against 6f52a1ee2. A review had turned
it off -- the W6 map (b6d6e062b) predated the W5 harness fold, 1175 of its
1355 binary ids were gone at HEAD. `map_fits` is the guard against that case
now: a map that does not fit the tree plans floor + graph, never fewer tests.

FAIL-CLOSED. The narrowing takes tests away, so every doubt keeps them:

    no graph                          -> the resolver floor           (NOTE)
    no map, a broken or partial map   -> floor + graph, rdeps kept    (NOTE)
    a map older than 7 days           -> floor + graph, rdeps kept    (NOTE)
    a changed source the map does not know (new file, unknown stem)
                                      -> rdeps(X) of that crate kept  (NOTE)
    a crate test file no test names (rule 8 runtime `unread`)
                                      -> rdeps(X) of that crate kept  (NOTE)
    a diff without a Rust class under the kit/docs/plans roots
    (`non_rust`, no rdeps; #1160)
                                      -> floor + graph of the diff,
                                         no map drift terms           (NOTE)

RUNTIME READS (GH #1138, OR-DT.W6.5). The map sees executed Rust lines, never
an opened file. Rule 8 runtime of the resolver puts the tests that read a
changed path at runtime -- a walked directory, a generator's sources, a crate
fixture -- on the floor, for the diff AND for the files changed since the map
rev (`map_fits`). That is the W6 replay case R18 (`librarian_seed_corpus` after
a drift of `docs/cell-types.en.md`); the evidence is
`test_pipeline_impact.RuntimeReadsTest.test_narrowed_plan_holds_every_runtime_reader`.

THE MAP HAS TO FIT THE TREE (`map_fits`), else floor + graph:

    no cargo metadata                 -> the map's binaries cannot be checked
    a map binary HEAD does not have   -> the map predates a fold or a rename
    the map's rev is not in the repo  -> no tree diff to check
    R or D under tests/** since the map rev
                                      -> the map's test parts moved
    A or M under tests/** since the map rev
                                      -> that test binary is always planned
    a file changed since the map rev that the diff does not hold
                                      -> by class (`since_plan`, M-4 below)

FILES CHANGED SINCE THE MAP REV (M-4, welle-tb S1b, GH #1165). The map knows
them as they were at its rev, so their cover may have moved -- never all
of them through the floor (one `.config/nextest.toml` made every narrowed plan
all()), never "only their own cover" either:

    a workspace file on NO_SHIFT      -> nothing; it shifts no cover    (NOTE)
                                         (an explicit list, a reason per path)
    any other workspace file          -> rule 1, all()  (unknown shift)
    a Rust source, a crate file       -> its floor + rdeps(X) of its crate,
                                         which the map never replaces   (NOTE)
                                         (a new caller there is a relation
                                         the map lacks)
    any other file                    -> its floor like a diff path (rules
                                         3, 4, 5, 8); the map sees no
                                         non-Rust file, it replaces none

The notes of `impact` travel out with the gate's NOTE lines.

Evidence for turning it on: map f004e6c36 (227/227 binaries, map_fits
against 6f52a1ee2 = True), and the owner's call of 2026-10-09.

MERGE AND INTEGRATION (welle-tb S1 parts B + C, GH #1159). Release plans
everything, always. A merge or an integration narrows only after a decision
that names its reason term -- the term stands in the plan (`why`, the NOTE
lines, every test term's reasons) and in the selection's narrowing state:

    merge_decision        `stage:merge narrowed parent=<sha> strand-run=<id>`
        HEAD has exactly two parents, the first is the planned base; no
        revert; the strand run (`MECLAW_GATE_STRAND_RUN`, the receipt's store
        run) is a GREEN strand gate of a clean tree in the journal, and the
        strand parent differs from that gated rev under `plans/` only (the
        source closure, not the SHA); the parents merge without a conflict and
        the merge commit changes no Rust path beyond that union (the merge
        point's version, changelog and corpus files are non-Rust and planned by
        the floor). Planned over `<merge base>..HEAD` plus `<base>..HEAD`.
        Any doubt -- also no fitting map -- keeps the merge unnarrowed: the
        resolver floor, as before.
    integration_decision  `stage:integration narrowed since=<sha>`
        The wave (`MECLAW_GATE_WAVE`) has a start in the store's journal
        (`pipeline wave start`, event `wave_started`) and that start is an
        ancestor of HEAD. The tests row is planned from the wave diff
        `<start>..HEAD`. Any doubt -- no wave, no start, no ancestor, no
        graph, no fitting map, a file with an unknown reader -- plans the full
        suite (KR-TB-1); a gone name without a folded form stays rdeps of its
        crate (part 0).

The narrowing state of a narrowed merge or integration is `on` whenever a
fitting map was in use, also when no rdeps was left to replace: the cache key
reads `on` and `map` (`cache._narrow_state`), so a narrowed and an unnarrowed
merge never share a key (V12).
"""

import glob
import json
import os
import pathlib
import re

ENV = "MECLAW_GATE_NARROW"
STRAND_RUN_ENV = "MECLAW_GATE_STRAND_RUN"
WAVE_ENV = "MECLAW_GATE_WAVE"
_RUST = ("rust_src", "rust_test", "crate_file", "workspace")
_REVERT = re.compile(r"(?m)^Revert\b|This reverts commit")
_BIN = re.compile(r"binary_id\(=([^)]+)\)")
_TESTS = re.compile(r"(^|/)tests/")
FUTURE_SKEW_DAYS = 1 / 24.0   # an hour of clock skew between lane and host
# #1160 part A (V8): the path classes of a diff without Rust source. Such a
# diff has no rdeps(X) the map could replace, so the drift terms of `map_fits`
# (cover of every source, every test changed since the map rev) only ever ADD
# binaries -- py-only planned 79 binaries narrowed against 3 unnarrowed. A diff
# made of these classes alone plans floor + graph of the diff; one path of any
# other class (templates, crates, config, data) keeps today's plan.
#
# The class is read off the resolver, not off a file extension (welle-val2
# v2-flake-b01, kit finding a): the old list knew `.py`/`.sh` under scripts/,
# so `scripts/pipeline/selftest_durations.json` in a scripts-only diff took the
# map and its drift -- 239 of 247 binaries, 10800 tests. A list of non-Rust
# extensions grows with every new file kind and fails open in the expensive
# direction; what can shift a cover the map knows is exactly what the resolver
# calls Rust (`_RUST`: a crate source or test, any other crate file, a
# workspace file). A Rust test that READS a path (`include_str!`, a walked
# directory, a spawned script) is no reason to take the map: the map sees
# executed `.rs` lines only and names no such reader, the floor plans it by
# rule 8 and the graph either way. NON_RUST_ROOTS bounds the class to the roots
# #1160 accepted without the map (the kit, the docs, the plans, a top-level
# page); templates, examples and workshop keep today's plan -- they run inside
# Rust tests, and the integration's full suite on doubt (KR-TB-1) reads
# `no_map`.
NON_RUST_ROOTS = re.compile(r"^(?:scripts/|docs/|plans/|[^/]+\.md$)")


def enabled(env=None):
    """On unless the switch is exactly `0` (#1126: on by default)."""
    return (env if env is not None else os.environ).get(ENV, "").strip() != "0"


def coverage_dir():
    env = os.environ.get("MECLAW_PIPELINE_COVERAGE_DIR")
    if env:
        return pathlib.Path(env)
    return pathlib.Path.home() / ".local/state/meclaw/pipeline/coverage"


def newest_map(directory=None):
    """`(map, None)` of the newest `coverage-<rev>.json` (by `built_at`), or `(None, why)`."""
    from . import coverage as coverage_mod
    directory = pathlib.Path(directory or coverage_dir())
    best, why = None, "no coverage map in %s" % directory
    for path in glob.glob(str(directory / "coverage-*.json")):
        cov, err = coverage_mod.load_map(path)
        if cov is None:
            why = err
            continue
        if best is None or str(cov.get("built_at") or "") > str(best.get("built_at") or ""):
            best = cov
    return (best, None) if best is not None else (None, why)


def _git(repo, *args):
    import subprocess
    return subprocess.run(["git", "-C", repo] + list(args), capture_output=True, text=True)


def _git_z(repo, *args):
    """`(rc, fields)` of a `git ... -z` call: the NUL-separated fields, names
    verbatim. Without `-z` git quotes a name with non-ASCII, `"` or `\\`
    (`"crates/beta/src/\\303\\244.rs"`): it had no class, sat in no crate and
    narrowed a change it never planned (re-check I-h1, wave validation 2)."""
    import subprocess
    res = subprocess.run(["git", "-C", repo] + list(args), capture_output=True)
    return res.returncode, res.stdout.decode("utf-8", "surrogateescape").split("\0")[:-1]


def _test_target_of(rel, repo, metadata):
    """`(binary_id, None)` of the test target `rel` belongs to, or `(None, crate)`.

    The paths of the metadata are those of the tree it was made in: the
    cache comes from a lane (all seven files of 10.10. name
    /srv/target/tree), so `rel` is matched against `workspace_root`, the
    repository only without one (Fix-Runde 1b: against the repository a
    test changed since the map planned nothing)."""
    from . import plan as plan_mod
    root = metadata.get("workspace_root") or repo
    crate = None
    for pkg in plan_mod._members(metadata):
        pdir = os.path.relpath(os.path.dirname(pkg.get("manifest_path") or ""), root)
        if rel.startswith(pdir.rstrip("/") + "/"):
            crate = pkg["name"]
        for t in pkg.get("targets") or []:
            if "test" not in (t.get("kind") or []):
                continue
            src = os.path.relpath(t.get("src_path") or "", root)
            tdir = os.path.dirname(src) if os.path.basename(src) == "main.rs" else None
            if rel == src or (tdir and rel.startswith(tdir + "/")):
                return plan_mod._binary_id(pkg, t), None
    return None, crate


def map_fits(cov, repo, metadata):
    """`(ok, notes, files, terms)`: does the map fit the tree at HEAD?

    `files` are the files changed since the map's rev (planned by class,
    `since_plan`), `terms` the test binaries added or changed since (planned)."""
    from . import impact as impact_mod
    from . import plan as plan_mod
    rev = str(cov.get("rev") or "")
    short = rev[:9]
    if metadata is None:
        return False, ["narrowing: no cargo metadata -- the binaries of map %s cannot be "
                       "checked -- floor + graph, rdeps kept" % short], [], []
    known = {b for p in plan_mod._members(metadata) for b, _n in plan_mod._binaries(p)}
    gone = sorted({p.get("binary") for p in cov.get("parts") or []} - known)
    if gone:
        return False, ["narrowing: coverage map %s names %d binaries HEAD does not have (%s) "
                       "-- floor + graph, rdeps kept" % (short, len(gone), ", ".join(gone[:5]))], [], []
    rc, fields = _git_z(repo, "diff", "--name-status", "-z", "-M", rev, "HEAD", "--")
    if rc != 0 or _git(repo, "cat-file", "-e", "%s^{commit}" % rev).returncode != 0:
        return False, ["narrowing: coverage map rev %s is not in this repository -- "
                       "floor + graph, rdeps kept" % short], [], []
    files, terms, moved = [], set(), []
    rows, i = [], 0
    while i < len(fields):   # `-z`: status, path, and a second path for R/C
        status = fields[i]
        n = 2 if status[:1] in ("R", "C") else 1
        rows.append((status, fields[i + 1:i + 1 + n]))
        i += 1 + n
    for status, paths in rows:
        if not paths:
            continue
        if _TESTS.search(paths[0]) and status[:1] in ("R", "D"):
            moved.append("%s %s" % (status[:1], paths[0]))
            continue
        rel = paths[-1]
        if _TESTS.search(rel):
            bid, crate = _test_target_of(rel, repo, metadata)
            if bid:
                terms.add("binary_id(=%s)" % bid)
            elif crate:
                terms.add("rdeps(%s)" % crate)   # a shared helper: its crate stays whole
            elif impact_mod.gate_plan._crate_of(rel):
                # fail-closed (Fix-Runde 1b): a crate's test the metadata
                # cannot place planned nothing -- the map does not fit instead.
                # tests/ outside crates/ (scripts/tests, katas) is no member.
                return False, ["narrowing: %s changed since coverage map %s is in no target "
                               "of the metadata -- floor + graph, rdeps kept"
                               % (rel, short)], [], []
        else:
            files.append(rel)
    if moved:
        return False, ["narrowing: tests moved since coverage map %s (%s) -- floor + graph, "
                       "rdeps kept" % (short, ", ".join(moved[:5]))], [], []
    return True, [], files, sorted(terms)


def non_rust(files, select):
    """True when every changed path is of a known non-Rust class and the floor
    has no rdeps(X) -- then the map has nothing to replace (fail-closed: an
    unknown path or an rdeps keeps today's plan). Known = under NON_RUST_ROOTS
    and of none of the resolver's Rust classes (`_RUST`)."""
    from . import impact as impact_mod
    gp = impact_mod.gate_plan
    return bool(files) and not (select or {}).get("rdeps") \
        and all(NON_RUST_ROOTS.match(f) and not gp._classes_of_path(f) & set(_RUST)
                for f in files)


# M-4 (OR-TB.S1b.17): a workspace file changed since the map rev that provably
# shifts no cover plans nothing. An explicit list, one reason per path, no
# pattern -- every other workspace file keeps rule 1 (all()).
#   .config/nextest.toml  profiles, timeouts, test groups, retries, junit: the
#                         filterset of the tests row decides WHICH tests run
#                         (the file's own header says so) and the binaries
#                         decide what they execute; the tests station's cache
#                         key reads the file (`cache.INPUTS`), so a changed
#                         file never reuses a result. Only while it holds no
#                         key of NO_SHIFT_GUARD.
#   deny.toml             read by `cargo deny` alone (station deny); no build,
#                         no test reads it.
NO_SHIFT = {
    ".config/nextest.toml": "runner settings only -- no filter, wrapper or setup script",
    "deny.toml": "read by cargo deny alone",
}
# The nextest keys that DO select, wrap or prepare a test: a default filter,
# extra test-binary arguments, a wrapper, a setup script. M-5: read with
# `tomllib` at any depth (quoted, dotted, inline -- one key to TOML), in the
# map's state AND at HEAD -- one of them in either state and the file is no
# NO_SHIFT entry (a key removed since the map lets in tests the map never
# measured): rule 1, all(). Unreadable or no TOML: none either.
NO_SHIFT_GUARD = {
    ".config/nextest.toml": frozenset({"default-filter", "run-extra-args", "run-wrapper",
                                       "list-wrapper", "setup", "scripts", "script"}),
}
_SINCE_RDEPS = ("rust_src", "crate_file")

# V2-A3 (K9, welle-val2): the merge point's version bump
# (`versions.set_workspace_version`) rewrites `[workspace.package] version`,
# the workspace members' versions in Cargo.lock and the inter-crate pins
# (`meclaw-* = { path = ..., version = ... }`) in `crates/*/Cargo.toml`. That
# changes no code: a manifest whose diff since the map is only those lines
# keeps the map (no floor, no rdeps). Compared as parsed TOML with exactly
# those values removed -- a dependency, feature, path or registry version
# that moved is a difference and keeps rule 1 / rule 10. Unreadable, no TOML
# or absent on one side: no version-only diff (fail-closed).
_VERSION_ONLY = re.compile(r"^(?:Cargo\.toml|Cargo\.lock|crates/[^/]+/Cargo\.toml)$")
_DEP_TABLES = ("dependencies", "dev-dependencies", "build-dependencies")


def _strip_versions(doc, lock):
    """`doc` without the values the merge point's bump writes (a copy)."""
    import copy
    doc = copy.deepcopy(doc)
    if lock:
        for pkg in doc.get("package") or []:
            if isinstance(pkg, dict) and "source" not in pkg:   # a workspace member
                pkg.pop("version", None)
        return doc
    for table in (doc.get("package"), (doc.get("workspace") or {}).get("package")):
        if isinstance(table, dict) and isinstance(table.get("version"), str):
            table.pop("version")

    def pins(node):
        if isinstance(node, dict):
            for key, value in node.items():
                if key in _DEP_TABLES and isinstance(value, dict):
                    for name, spec in value.items():
                        if name.startswith("meclaw-") and isinstance(spec, dict) \
                                and "path" in spec:
                            spec.pop("version", None)
                pins(value)
        elif isinstance(node, list):
            for value in node:
                pins(value)
    pins(doc)
    return doc


def _version_only(rel, repo, map_rev):
    """True when `rel` (a Cargo manifest or the lock file) differs between the
    map's rev and HEAD in the bump's version values alone."""
    if not map_rev or not _VERSION_ONLY.match(rel):
        return False
    lock = rel == "Cargo.lock"
    docs = []
    for rev in (map_rev, "HEAD"):
        if _git(repo, "cat-file", "-e", "%s:%s" % (rev, rel)).returncode != 0:
            return False
        doc = _toml_at(repo, rev, rel)
        if not doc:
            return False
        docs.append(_strip_versions(doc, lock))
    return docs[0] == docs[1]


def _guard_hit(doc, keys):
    """Does the parsed TOML `doc` hold one of `keys` at any depth?"""
    if isinstance(doc, dict):
        return any(k in keys or _guard_hit(v, keys) for k, v in doc.items())
    if isinstance(doc, list):
        return any(_guard_hit(v, keys) for v in doc)
    return False


def _toml_at(repo, rev, rel):
    """The parsed `rel` at `rev`: {} when the file is absent there, None when
    it cannot be read or is no TOML."""
    import tomllib
    if _git(repo, "cat-file", "-e", "%s:%s" % (rev, rel)).returncode != 0:
        return {} if _git(repo, "cat-file", "-e", "%s^{commit}" % rev).returncode == 0 else None
    res = _git(repo, "show", "%s:%s" % (rev, rel))
    if res.returncode != 0:
        return None
    try:
        return tomllib.loads(res.stdout)
    except (tomllib.TOMLDecodeError, ValueError):
        return None


def _no_shift(rel, repo, map_rev):
    """The NO_SHIFT reason of `rel`, or None (not listed, a guard key in the
    map's state or at HEAD, unreadable)."""
    why = NO_SHIFT.get(rel)
    keys = NO_SHIFT_GUARD.get(rel)
    if why is None or keys is None:
        return why
    if not map_rev:
        return None
    for rev in (map_rev, "HEAD"):
        doc = _toml_at(repo, rev, rel)
        if doc is None or _guard_hit(doc, keys):
            return None
    return why


def since_plan(since, files, repo, map_rev):
    """`(floor, terms, notes)` of the files changed since the map rev (M-4).

    A file the diff holds is the diff's. A Cargo manifest or lock file whose
    change is the merge point's version bump alone (`_version_only`, V2-A3)
    plans nothing. Every other one by class: on NO_SHIFT nothing; any other workspace file goes to the floor (rule 1,
    all()); a Rust source or crate file goes to the floor and keeps
    rdeps(<crate>) in `terms` -- the map knows the old file, so it replaces
    none of that crate; any other file goes to the floor like a diff path.
    """
    from . import impact as impact_mod
    gp = impact_mod.gate_plan
    short = str(map_rev or "")[:9]
    floor, terms, notes = [], set(), []
    diff = set(files)
    for rel in sorted(set(since) - diff):
        if _version_only(rel, repo, map_rev):
            notes.append("narrowing: %s changed since coverage map %s -- version lines only "
                         "(the merge point's bump), the map stays valid" % (rel, short))
            continue
        classes = gp._classes_of_path(rel)
        if "workspace" in classes:
            why = _no_shift(rel, repo, map_rev)
            if why:
                notes.append("narrowing: %s changed since coverage map %s -- shifts no cover "
                             "(%s), no floor" % (rel, short, why))
                continue
        floor.append(rel)
        crate = gp._crate_of(rel)
        if crate and "workspace" not in classes and classes & set(_SINCE_RDEPS):
            terms.add("rdeps(%s)" % crate)
            notes.append("narrowing: %s changed since coverage map %s -- the map knows the "
                         "old file, rdeps(%s) kept" % (rel, short, crate))
    return floor, sorted(terms), notes


def drift_note(cov, since, files, since_terms, rdeps_left, res, now=None, diff_terms=()):
    """The one loud NOTE when the map's drift alone keeps rdeps, else None.

    Kit finding b (welle-val2 v2-flake-b01): map 45a463300 predated a Rust
    merge on the base by nine hours; `since_plan` kept rdeps(meclaw-cells) for
    files the diff did not hold, and a diff without Rust ran 239 of 247
    binaries, said only in one NOTE per drifted file. A fresh map is a full
    coverage run on the coverage lane -- the gate does not start it; it says,
    first, which map, how old, how many files and which rdeps the drift
    alone keeps, and how many binaries that makes.

    `diff_terms` are the rdeps terms the diff's own files keep (a file not in the
    map, a crate file the map cannot see): a fresh map keeps them too, so they are
    no drift's doing and the NOTE leaves them out (re-check R2 M2)."""
    from . import coverage as coverage_mod
    own = set(diff_terms)
    held = sorted(t[len("rdeps("):-1] for t in since_terms
                  if t[len("rdeps("):-1] in set(rdeps_left) and t not in own)
    if cov is None or not held:
        return None
    try:
        age = "%.1f h old" % (coverage_mod.age_days(cov, now) * 24.0)
    except (KeyError, TypeError, ValueError, AttributeError):
        age = "of unknown age"
    drifted = len(set(since) - set(files))
    return ("narrowing: coverage map %s is %s and predates %d file(s) changed on the base "
            "since (not in this diff) -- rdeps(%s) kept for that drift, %d of %d "
            "binaries; a fresh map (`scripts/pipeline.py coverage build`) narrows them"
            % (str(cov.get("rev"))[:9], age, drifted, ", ".join(held),
               res["binaries"]["selected"], res["binaries"]["total"]))


def state(on, map_rev, narrowed=None):
    """The narrowing state a selection carries (the cache key reads `on` and `map`)."""
    out = {"on": bool(on), "map": str(map_rev or "") if on else ""}
    if narrowed:
        out["narrowed"] = narrowed
    return out


def full_select(select, repo, why):
    """The full suite in the resolver's selection form (integration in doubt, KR-TB-1).

    It subtracts the scenario class AND every browser unit (rule 11,
    `gate_plan.tests_minus_for`): in `tests` a browser unit runs unbooked and
    without MECLAW_REQUIRE_BROWSERS, SKIP green on a lane without the engine
    (re-check tb-s3 n2 M-2). `full_suite` and `apply` plan them into
    `browser:display` (`plan_browser_orphans`)."""
    from . import impact as impact_mod
    minus = impact_mod.gate_plan.tests_minus_for(repo)
    out = dict(select or {}, binaries=[], rdeps=[], all=True, floor=False,
               filter="all() - (%s)" % minus if minus else "all()", narrow=state(False, ""),
               why=list((select or {}).get("why") or []) + ["narrow: %s" % w for w in why])
    out.pop("compile", None)
    return out


def narrow(select, files, repo, graph_doc, cov, cov_why=None, now=None, metadata=None,
           stage="strand", narrowed=None):
    """`(select, notes)` -- the tests station's selection after the narrowing.

    `stage` merge or integration narrows only with the `narrowed` reason term
    of its decision; without it the selection stays as it is."""
    from . import coverage as coverage_mod
    from . import impact as impact_mod
    if stage not in ("strand", "merge", "integration") or (stage != "strand" and not narrowed):
        return select, []
    if select is None or select.get("all") or select.get("floor") or not files:
        return select, []
    if graph_doc is None:
        why = "narrowing: no graph (%s)" % (cov_why or "not built")
        if stage == "integration":
            return full_select(select, repo, [why, narrowed]), [why + " -- full suite"]
        return select, [why + " -- resolver floor"]
    notes, usable = [], None
    if cov is None:
        notes.append("narrowing: %s -- floor + graph, rdeps kept" % (cov_why or "no coverage map"))
    elif not coverage_mod.is_complete(cov):
        notes.append("narrowing: coverage map %s is not complete -- floor + graph, rdeps kept"
                     % str(cov.get("rev"))[:9])
    else:
        try:
            age = coverage_mod.age_days(cov, now)
        except (KeyError, TypeError, ValueError, AttributeError):
            age = None
        short = str(cov.get("rev"))[:9]
        if age is None:
            notes.append("narrowing: coverage map %s has no valid built_at -- floor + graph, "
                         "rdeps kept" % short)
        elif age < -FUTURE_SKEW_DAYS:   # a future map would never go stale
            notes.append("narrowing: coverage map %s was built in the future (%s) -- floor + "
                         "graph, rdeps kept" % (short, cov.get("built_at")))
        elif age > impact_mod.STALE_DAYS:
            notes.append("narrowing: coverage map %s is %.1f days old (> %d) -- floor + graph, "
                         "rdeps kept" % (short, age, impact_mod.STALE_DAYS))
        else:
            usable = cov
    # Every stage (re-check H2 M3, m-N7): a python/docs-only diff plans
    # floor + graph without the map -- its drift terms would only add.
    skip_map = non_rust(files, select)
    if skip_map:
        notes.append("narrowing: no Rust source in the diff (kit/docs/plans only) -- floor + graph "
                     "of the diff, no map drift terms")
        usable = None
    since, extra = [], []
    if usable is not None:
        ok, fit_notes, since, extra = map_fits(usable, repo, metadata)
        notes.extend(fit_notes)
        if not ok:
            usable, since, extra = None, [], []
    if usable is None and stage == "merge" and not skip_map:
        notes.append("narrowing: %s -- the map does not fit, the merge stays unnarrowed"
                     % narrowed)
        return select, notes
    # M-4: the files changed since the map go in by class, never wholesale
    since, since_terms, since_notes = since_plan(since, files, repo, (usable or {}).get("rev"))
    notes.extend(since_notes)
    extra = sorted(set(extra) | set(since_terms))
    want = sorted(set(files) | set(since))
    res = impact_mod.impact(want, stage, graph_doc, repo=repo, coverage=usable, now=now,
                            stations=False, rev=(usable or {}).get("rev"), narrowed=narrowed,
                            no_map=skip_map)
    notes.extend("narrowing: impact: %s" % n for n in res.get("notes") or [])
    terms = sorted(set(res["tests"]) | set(extra))
    if "all()" in terms:
        terms = ["all()"]
    for t in terms:
        if t.startswith("rdeps(") and usable is not None:
            unknown = sorted(r.split("file:", 1)[1] for r in res["reason"].get(t, ())
                             if r.startswith(("new:file:", "cover:unknown file:")))
            if unknown:
                notes.append("narrowing: %s not in the coverage map -- %s kept"
                             % (", ".join(unknown), t))
    if narrowed:
        notes.append("narrowing: %s" % narrowed)
    if res["coverage"]["used"]:
        notes.append("narrowing: on, map %s -- %d of %d binaries"
                     % (str(usable.get("rev"))[:9], res["binaries"]["selected"],
                        res["binaries"]["total"]))
    # B-2: the filter is rendered from `terms` -- the drift terms of
    # `map_fits` and the rdeps of `since_plan` RUN, not only build
    filt = impact_mod.render_filter(terms, stage, repo) or select.get("filter")
    # GH #1182: what `render_filter` subtracts by policy is exempt here; the
    # stations that run it are judged once the plan is whole (`unrun_subtractions`)
    missing = unplanned_run(terms, filt, exempt=summands(
        impact_mod.render_filter(["all()"], stage, repo))[1])
    if missing:
        notes.append("narrowing: plan != run (%s not in the filter) -- resolver floor"
                     % ", ".join(missing))
        return select, notes
    out = dict(select)
    rdeps_left = sorted(t[len("rdeps("):-1] for t in terms if t.startswith("rdeps("))
    # an rdeps a reason ties to a file of the diff stays with any map
    diff_terms = sorted(t for t in terms if t.startswith("rdeps(")
                        and any(p in r for r in res["reason"].get(t, ()) for p in files))
    drift = drift_note(usable, since, files, since_terms, rdeps_left, res, now, diff_terms)
    if drift:
        notes.insert(0, drift)
    # Execution coverage is no compile dependency. The test
    # binaries of a replaced rdeps(X) are still BUILT (no run) -- `compile`.
    replaced = sorted(set(select.get("rdeps") or []) - set(rdeps_left))
    if replaced:
        out["compile"] = sorted(set(select.get("compile") or []) | set(replaced))
    out.update(binaries=sorted({b for t in terms for b in _BIN.findall(t)}),
               rdeps=rdeps_left,
               all="all()" in terms, filter=filt,
               # review Z-I gap C-A: the cache key carries the narrowing state
               # V12: a narrowed merge/integration is `on` with its map even
               # when no rdeps was left to replace
               narrow=state(res["coverage"]["used"] or (narrowed and usable is not None
                                                        and "all()" not in terms),
                            (usable or {}).get("rev"), narrowed),
               why=list(select.get("why") or []) + ["narrow: %s" % n for n in notes])
    if out["all"]:
        out.pop("compile", None)
    return out, notes


def unrun_subtractions(rows, repo, reaches=None, locks=True):
    """The terms the `tests` row subtracts that one of its summands reaches
    and no other row runs -- the guard over the whole plan (GH #1182). Called
    once every station is planned (`refuse_unrun`, behind
    `plan_browser_orphans`): what another row runs is read from its
    `test-tier.sh filter` commands themselves (`_ran`), so a row without a
    command, or with another command, runs nothing.

    Exempt: the scenario class (policy); a browser unit whose every test is
    `#[ignore]`d -- it runs in `tests` neither way (re-check tb-s1e h1 M-1 a:
    six such units in meclaw-cells, each one refused every plan reaching the
    crate); a count of (0, 0) is no such proof but doubt (h1 F-1: the lexer
    read (0, 0) for five units with a runnable test), it stays named; with
    `locks=False` the BROWSER_LOCKS units -- after the lock cut of a narrowed
    integration a missing lock is the owner's ruling of 10.10.
    (`pipeline/browser_plan.py`, as in `browser_orphans`). `reaches(summand,
    term)` as in `unplanned_run`; by default a browser unit by `_reaches` (the
    orphan step's reading), any other term yes (fail-closed)."""
    from . import impact as impact_mod
    gp = impact_mod.gate_plan
    row = _tests_row(rows)
    if row is None:
        return []
    filt = (row["select"] or {}).get("filter")
    minus = summands(filt)[1]
    units = {t: v for t, v in gp.browser_units(repo).items() if t in minus} if minus else {}
    exempt = set(summands(gp.scenario_for(repo))[0])
    for t, (crate, unit, text) in units.items():
        tests, runnable = gp._test_counts(text)
        if (tests and not runnable) or (not locks and gp._browser_lock_term(crate, unit)):
            exempt.add(t)
    if reaches is None and units:
        users = gp._crate_users(repo)

        def reaches(s, t):
            return t not in units or _reaches(s, units[t][0], units[t][1], users)
    return unplanned_run([], filt, elsewhere=_ran(rows, row, units), exempt=exempt,
                         reaches=reaches)


class UnrunSubtraction(ValueError):
    """`tests` subtracts a term one of its summands reaches and no station
    runs (GH #1182). The plan is refused (`gate: no plan`, exit 2): the
    alternative was a silent hole, the unit running nowhere."""


def refuse_unrun(rows, repo, locks=True):
    """The guard over the whole plan (GH #1182), behind every
    `plan_browser_orphans` -- `apply`, `full_suite` and the cut integration
    in `pipeline/plan.py` -- with the same `locks`. Raises UnrunSubtraction
    naming what no station runs. Re-check tb-s1e h1 M-1: `narrow()` exempts
    the policy subtractions (OR-TB.S1e.1), so without this call the case of
    tb-s3 aa9454842 stayed silent."""
    left = unrun_subtractions(rows, repo, locks=locks)
    if left:
        raise UnrunSubtraction("tests subtracts %s, which one of its summands reaches and no "
                               "station runs (GH #1182)" % ", ".join(left))


def _ran(rows, row, units):
    """The terms the rows other than `row` run whole (`_runs` of each
    `test-tier.sh filter` command, the cuts of all of them together), plus
    the browser units of `units` one of those terms runs (`_runs_unit`)."""
    cuts = {}
    for r in rows:
        for argv in [] if r is row else (r.get("cmds") or [r.get("cmd") or []]):
            if argv[:2] == ["scripts/test-tier.sh", "filter"] and len(argv) > 2:
                for term, got in _runs(argv[2]).items():
                    cuts.setdefault(term, set()).update(got)
    whole = {t for t, got in cuts.items()
             if () in got or any(len(c) == 1 and c[0][0] == "-" and (("&", c[0][1]),) in got
                                 for c in got)}
    return whole | {t for t, (crate, unit, _x) in units.items()
                    if any(_runs_unit(r, crate, unit) for r in whole)}


def _runs(filt, cut=()):
    """`{term: {cut}}` -- the terms a filter runs and under which cut: a
    summand runs whole (`()`), a parenthesised union is opened up, `(U) - X`
    runs U without X (cut `("-", X)`), `(U) & X` U within X (`("&", X)`).
    Only the two cuts of the same X together run U whole (`_ran`). L-a of
    re-check tb-s1e h1: the left side of a ` & ` counted as run whole, so one
    `(U) & <webkit>` command alone passed for the whole union (fail-open)."""
    out = {}
    plus, minus = summands(filt)
    if minus:
        cut = cut + (("-", " + ".join(sorted(minus))),)
    for t in plus:
        parts = [p.strip() for p in _split_top(t, " & ")]
        head, here = parts[0], cut + tuple(("&", p) for p in parts[1:])
        inner = head[1:-1] if head.startswith("(") and _opens_all(head) else None
        # a folded unit `(binary_id(=c::h) & test(/^stem::/))` is one term, no union
        if inner is not None and (len(_split_top(inner, " + ")) > 1 or summands(inner)[1]
                                  or len(_split_top(inner, " & ")) == 1):
            for term, got in _runs(inner, here).items():
                out.setdefault(term, set()).update(got)
        else:
            out.setdefault(head, set()).add(here)
    return out


def _runs_unit(term, crate, unit):
    """Does a term another row runs run the browser unit? Its own term, its
    binary by id, or a `binary(/<re>/)` that matches its binary's name --
    `browser_cmds` names a lock that way (`gate_plan._browser_select` reads
    it alike). Any other shape: no -- the guard names the unit (fail-closed)."""
    binary = unit.split("/")[0]
    if term == _gate_plan()._binary_id(crate, unit):
        return True
    m = re.fullmatch(r"binary\(/(.*)/\)", term)
    if m:
        try:
            return re.search(m.group(1), binary) is not None
        except re.error:
            return False
    m = re.fullmatch(r"binary_id\(=([^:)]+)::([^)]+)\)", term)
    return bool(m) and m.group(1) == crate and m.group(2) == binary


def summands(filt):
    """`(plus, minus)` of a nextest filter `<a> + <b> - (<c> + <d>) - <e>`:
    its top-level summands and the terms its top-level subtractions take out.

    GH #1182: it stopped at the first ` - ` and cut the subtractions off, so
    the guard took every one for planned -- a browser unit subtracted from
    `tests` that no station ran passed it silently (tb-s3 aa9454842)."""
    parts = _split_top(filt or "", " - ")
    minus = set()
    for part in parts[1:]:
        part = part.strip()
        while part.startswith("(") and _opens_all(part):
            part = part[1:-1]
        minus |= set(_split_top(part, " + "))
    return set(_split_top(parts[0], " + ")) if parts else set(), minus


def _split_top(text, sep):
    """`text` split at every `sep` outside parentheses."""
    out, depth, cur, i = [], 0, "", 0
    while i < len(text):
        ch = text[i]
        depth += (ch == "(") - (ch == ")")
        if depth == 0 and text.startswith(sep, i):
            out.append(cur)
            cur, i = "", i + len(sep)
            continue
        cur += ch
        i += 1
    return out + [cur] if cur else out


def _opens_all(text):
    """Does the parenthesis that opens `text` close at its last character?"""
    depth = 0
    for i, ch in enumerate(text):
        depth += (ch == "(") - (ch == ")")
        if depth == 0:
            return i == len(text) - 1
    return False


def unplanned_run(terms, filt, elsewhere=(), exempt=(), reaches=None):
    """What the plan holds and nobody runs (B-2 guard: plan == run).

    A planned term is missing when one of its summands is no summand of the
    filter. A term the filter subtracts is missing as well (GH #1182) when a
    summand of the filter reaches it -- `reaches(summand, term)`, None =
    unknown = yes (fail-closed) -- unless it is subtracted by policy
    (`exempt`: the scenario class) or another station runs it (`elsewhere`:
    the terms the other rows' commands carry, read from the commands
    themselves -- a row without a `test-tier.sh filter` command runs none,
    re-check tb-s3 nb L-1)."""
    plus, minus = summands(filt)
    out = {t for t in terms if not summands(t)[0] <= plus}
    out |= {m for m in minus - set(exempt) - set(elsewhere)
            if reaches is None or any(reaches(p, m) for p in plus)}
    return sorted(out)


# --- browser units: what `tests` subtracts runs in `browser:display` ----------
# Rule 11 (GH #1161, re-check M-3) subtracts EVERY browser unit from `tests`
# (`gate_plan.tests_minus_for`), and `browser:display` takes only the units the
# diff its row was planned from reaches (`gate_plan.browser_moved`). The
# narrowing selects beyond that diff: rdeps of a crate changed since the map
# (M-4, `since_plan`), the map's drift terms, the wider merge diff, the wave
# diff of an integration. Such a unit ran in no station (tb-s3-b) -- the
# invariant below plans it into `browser:display`, booked.

def _top_split(text, sep):
    """`text` split at every `sep` outside parentheses, in order."""
    out, depth, cur, i = [], 0, "", 0
    while i < len(text):
        ch = text[i]
        depth += (ch == "(") - (ch == ")")
        if depth == 0 and text.startswith(sep, i):
            out.append(cur)
            cur, i = "", i + len(sep)
            continue
        cur += ch
        i += 1
    return out + [cur] if cur else out


def _unwrap(text):
    """`text` without parentheses that enclose all of it."""
    while text.startswith("(") and _closes_at_end(text):
        text = text[1:-1]
    return text


def _closes_at_end(text):
    """Does the parenthesis that opens `text` close at its last character?"""
    depth = 0
    for i, ch in enumerate(text):
        depth += (ch == "(") - (ch == ")")
        if depth == 0:
            return i == len(text) - 1
    return False


def _split_minus(filt):
    """`(head, [subtracted terms])` of a filter `<head> - (<a> + <b>)`."""
    parts = _top_split(filt or "", " - ")
    if len(parts) < 2:
        return filt or "", []
    return parts[0], _top_split(_unwrap(" - ".join(parts[1:])), " + ")


def _row_filters(row):
    return [argv[2] for argv in (row.get("cmds") or [row.get("cmd") or []])
            if argv[:2] == ["scripts/test-tier.sh", "filter"] and len(argv) > 2]


def browser_planned(rows, repo):
    """The browser-unit terms the `browser:display` row of `rows` runs: a lock
    by its binary term (`gate_plan._browser_select`), every other unit by its
    own term as a summand of the station's union (`gate_plan.browser_cmds`)."""
    gp = _gate_plan()
    row = next((r for r in rows if r.get("name") == "browser:display"), None)
    if row is None:
        return set()
    import types
    cmds = row.get("cmds") or ([row["cmd"]] if row.get("cmd") else [])
    locks = set(gp._browser_select(types.SimpleNamespace(cmds=cmds))["extra"])
    union = set()
    for filt in _row_filters(row):
        for sep in (" - ", " & "):
            if filt.endswith(sep + gp.WEBKIT_TESTS):
                filt = filt[:-len(sep + gp.WEBKIT_TESTS)]
        union |= set(_top_split(_unwrap(filt), " + "))
    return {t for t, (c, u, _x) in gp.browser_units(repo).items()
            if t in union or (gp._browser_lock_term(c, u) and "%s::%s" % (c, u) in locks)}


def _reaches(term, crate, unit, users):
    """Does the `tests` summand `term` run the browser unit? A lib or bin
    part (`binary_id(=<crate>)`, no `::`) holds no `tests/` unit; every other
    shape as the resolver reads it (`gate_plan._selects_unit`, unknown = yes)."""
    gp = _gate_plan()
    m = re.match(r"^\(?binary_id\(=([^)]+)\)", term)
    if m and "::" not in m.group(1):
        return False
    return gp._selects_unit(term, crate, unit, users)


def _may_run(gp, text):
    """False only when the reader sees tests and every one is `#[ignore]`d."""
    tests, runnable = gp._test_counts(text)
    return runnable > 0 or tests == 0


def browser_orphans(rows, repo, locks=True):
    """The browser units `tests` subtracts although one of its own summands
    runs them and `browser:display` does not -- units that ran nowhere. A
    unit whose tests the reader sees and finds every one `#[ignore]`d runs in
    `tests` neither way and is none. A unit the reader counts (0, 0) is doubt
    and an orphan (re-check 003a717c3 B-neu-1): `gate_plan._test_counts` lost
    a string with a line continuation (read since the token lexer, welle-tb S4;
    its doubt now is a character it does not know) and read 744, gh695, gh703_phone,
    gh767_page_canvas and gh958 -- 11 runnable tests -- as empty, so they ran
    in no station. `locks=False` leaves the BROWSER_LOCKS units out:
    after the lock cut of a narrowed integration a missing lock is the
    owner's ruling of 10.10. (`pipeline/browser_plan.py`), no orphan."""
    gp = _gate_plan()
    row = _tests_row(rows)
    if row is None:
        return []
    head, minus = _split_minus((row["select"] or {}).get("filter"))
    units = gp.browser_units(repo)
    cut = [t for t in minus if t in units]
    if not cut:
        return []
    heads = _top_split(head, " + ")
    users = gp._crate_users(repo)
    planned = browser_planned(rows, repo)
    return sorted(t for t in cut if t not in planned and _may_run(gp, units[t][2])
                  and (locks or not gp._browser_lock_term(units[t][0], units[t][1]))
                  and any(_reaches(s, units[t][0], units[t][1], users) for s in heads))


def plan_browser_orphans(rows, repo, locks=True):
    """Fail-closed (tb-s3-b): an orphan runs in `browser:display`, booked (its
    engines and colonies, `pipeline/plan.py` NEEDS; like `tests` it reuses the
    build's binaries, `plan.REUSE`, and takes no cargo lock of its own) --
    never in `tests`, which books no engine and runs without
    MECLAW_REQUIRE_BROWSERS (executor `_env`): there it would SKIP green on a
    lane without the engine (h2 M-3 class). A planned station takes the
    orphans into its union (both parts); without one the station is planned
    for exactly them, ONE call as a narrowed one (`gate_plan.browser_cmds`,
    keep=nothing). Either way the row's `reason` names them (re-check nb m-4;
    no `<label>: <path>` form, there is no diff path to cover). `tests` keeps
    subtracting them. The NOTE lines it printed."""
    orphans = browser_orphans(rows, repo, locks=locks)
    if not orphans:
        return []
    gp = _gate_plan()
    import types
    row = next((r for r in rows if r.get("name") == "browser:display"), None)
    if row is None:
        cmds = gp.browser_cmds(False, keep=set(), moved=orphans)
        row = {"name": "browser:display", "scope": "%d browser units" % len(orphans),
               "cargo": True, "parallel": {"by": "cmd", "profiles": list(gp.BROWSER_PROFILES)}}
        if "browser:display" in gp.ENV_STATIONS:
            row["env"] = True
        at = next((i + 1 for i, r in enumerate(rows) if r.get("name") == "tests"), len(rows))
        rows.insert(at, row)
    else:
        cmds = [_with_units(argv, orphans, gp)
                for argv in (row.get("cmds") or ([row["cmd"]] if row.get("cmd") else []))]
    row.pop("cmd", None)
    row.pop("cmds", None)
    if len(cmds) == 1:
        row["cmd"] = cmds[0]
    else:
        row["cmds"] = cmds
    row["select"] = gp._browser_select(types.SimpleNamespace(cmds=cmds))
    row["reason"] = list(row.get("reason") or []) + [
        "tests reaches %s beyond the plan -- booked here (tb-s3-b)" % ", ".join(orphans)]
    return ["browser units: %s reached by `tests` beyond the browser:display plan -- planned "
            "into browser:display" % ", ".join(orphans)]


def _with_units(argv, units, gp):
    """A `browser:display` command with `units` joined to its union."""
    if argv[:2] != ["scripts/test-tier.sh", "filter"] or len(argv) < 3:
        return argv
    filt, add = argv[2], " + ".join(units)
    for sep in (" - ", " & "):
        tail = sep + gp.WEBKIT_TESTS
        if filt.endswith(tail):
            filt = "(%s + %s)%s" % (_unwrap(filt[:-len(tail)]), add, tail)
            break
    else:
        filt = "%s + %s" % (filt, add)
    return argv[:2] + [filt] + argv[3:]


def _gate_plan():
    from . import impact as impact_mod
    return impact_mod.gate_plan


def _tests_row(rows):
    return next((r for r in rows if r.get("name") == "tests" and r.get("select") is not None), None)


def _swap_filter(row, old, new):
    """The command runs the same filter the parts were cut by."""
    if old and new and new != old:
        if "cmd" in row:
            row["cmd"] = [new if a == old else a for a in row["cmd"]]
        if "cmds" in row:
            row["cmds"] = [[new if a == old else a for a in argv] for argv in row["cmds"]]
        if row.get("scope") == old:
            row["scope"] = new


def apply(rows, files, repo, env=None, now=None, metadata=None, stage="strand", narrowed=None,
          graph_info=None):
    """Narrow the `tests` row of a resolver plan in place; the NOTE lines it printed.

    The graph is the one of `repo`'s head (`graph.fresh`, welle-validierung-2
    3a): built here when the latest build is of another rev, its time in
    `graph_info` (`{rev, secs, stored, reason}`) for the receipt's
    `graph_build_secs`. A failed build raises `graph.GraphBuildError`."""
    from . import graph as graph_mod
    if not enabled(env):
        return ["narrowing: off (%s=0) -- resolver floor" % ENV]
    row = _tests_row(rows)
    if row is None:
        return []
    built = None
    try:
        (graph_doc, built), gwhy = graph_mod.fresh(repo), None  # the switch is the process env
    except (OSError, ValueError, KeyError) as exc:
        graph_doc, gwhy = None, str(exc)
    cov, cwhy = newest_map()
    old = (row["select"] or {}).get("filter")
    row["select"], notes = narrow(row["select"], files, repo, graph_doc, cov,
                                  cwhy if graph_doc is not None else gwhy, now=now,
                                  metadata=metadata, stage=stage, narrowed=narrowed)
    if graph_doc is not None and graph_doc.get("code") != graph_mod.code_hash():
        # The edges are those of other code (#1160 only dropped prose edges, so
        # there a superset); the floor stays this resolver's. Said, never
        # silent: no step rebuilds the graph by itself (OR-TB.S2.11).
        notes = ["narrowing: graph built by other resolver code (%s, now %s) -- run "
                 "`scripts/pipeline.py graph build`" % (graph_doc.get("code"),
                                                        graph_mod.code_hash())] + notes
    if built is not None:
        if graph_info is not None:
            graph_info.update(built)
        notes = ["narrowing: graph of %s built (auto, %.1f s%s)%s"
                 % (built["rev"][:12], built["secs"],
                    "" if built["stored"] else ", dirty tree (%s): not stored"
                    % " ".join(built.get("dirt") or ["?"]),
                    " -- journal: %s" % built["journal"] if built.get("journal") else "")] \
            + notes
    _swap_filter(row, old, (row["select"] or {}).get("filter"))
    # tb-s3-b: the narrowed `tests` may reach a browser unit beyond the diff the
    # station was planned from -- the station takes it, never nowhere
    notes = notes + plan_browser_orphans(rows, repo)
    refuse_unrun(rows, repo)   # GH #1182: what is left runs nowhere
    return notes


def full_suite(rows, repo, why, env=None):
    """The `tests` row of an integration in doubt becomes the full suite (KR-TB-1)."""
    if not enabled(env):
        return ["narrowing: off (%s=0) -- resolver floor" % ENV]
    row = _tests_row(rows)
    if row is None:
        return ["%s -- no tests station" % w for w in why]
    old = (row["select"] or {}).get("filter")
    if not (row["select"] or {}).get("all"):
        row["select"] = full_select(row["select"], repo, why)
        _swap_filter(row, old, row["select"]["filter"])
    # re-check tb-s3 n2 M-2: the browser units the full suite subtracts run in
    # `browser:display`, booked; no lock cut in doubt, so locks stay in
    notes = ["%s -- full suite" % w for w in why] + plan_browser_orphans(rows, repo)
    refuse_unrun(rows, repo)   # GH #1182: what is left runs nowhere
    return notes


# --- merge and integration decisions (welle-tb S1 parts B + C) ----------------

def _open_store():
    """The store, read-only, or None (then nothing can be checked: no narrowing)."""
    from . import store as store_mod
    try:
        return store_mod.open_ro()
    except (OSError, ValueError, Exception):  # noqa: BLE001 -- doubt keeps the old plan
        return None


def _commit(repo, rev):
    res = _git(repo, "rev-parse", "--verify", "--quiet", "%s^{commit}" % rev)
    return res.stdout.strip() if res.returncode == 0 else None


def merge_decision(repo, base, strand_run, db):
    """`(term, since, notes)` of a merge commit at HEAD: the reason term and the
    merge base when the merge may plan like strand, else `(None, None, why)`."""
    from . import impact as impact_mod

    def no(why):
        return None, None, ["narrowing: merge stays unnarrowed -- %s" % why]
    line = _git(repo, "rev-list", "--parents", "-n", "1", "HEAD").stdout.split()
    parents = line[1:]
    if _REVERT.search(_git(repo, "log", "-1", "--format=%B", "HEAD").stdout):
        return no("a revert")
    if len(parents) < 2:
        return no("HEAD is no merge commit")
    if len(parents) > 2:
        return no("%d parents -- more than one strand parent" % len(parents))
    p1, p2 = parents
    if not base or _commit(repo, base) != p1:
        return no("the first parent %s is not the base %s" % (p1[:9], (base or "?")[:9]))
    if not strand_run:
        return no("no strand run (%s)" % STRAND_RUN_ENV)
    if db is None:
        return no("no store to read strand run %s from" % strand_run)
    from . import events as events_mod
    planned = ended = None
    for row in events_mod.read_run(db, strand_run):
        if row.get("event") == "pass_planned":
            planned = row.get("data") or {}
        elif row.get("event") == "pass_ended":
            ended = row.get("data") or {}
    gated = (planned or {}).get("rev") or ""
    if (not planned or planned.get("mode") != "strand" or planned.get("dirty") is not False
            or not ended or ended.get("verdict") != "GREEN" or not _commit(repo, gated)):
        return no("strand run %s is not a GREEN strand gate of a clean tree" % strand_run)
    # re-check H2 m1 / OR-TB.S7.12: what may move after the gate is the
    # receipt-reuse rule of `turnaround.code_equal_revs`, one source -- the
    # reports and strand files of the merge's own wave (`WAVE_ENV`), regular
    # files without a symlink on the path, named by no committed reader.
    from . import turnaround
    same, off = turnaround.code_equal_revs(repo, gated, p2,
                                           os.environ.get(WAVE_ENV, "").strip() or None)
    if not same:
        return no("the strand parent %s differs from the gated tree %s (%s)"
                  % (p2[:9], gated[:9], turnaround.describe(off)))
    res = _git(repo, "merge-tree", "--write-tree", p1, p2)
    if res.returncode != 0:
        return no("the parents conflict")
    union = res.stdout.split()[0] if res.stdout.split() else ""
    rc, names = _git_z(repo, "diff", "--name-only", "-z", union, "HEAD^{tree}", "--")
    hand = [p for p in names
            if p and set(impact_mod.gate_plan._classes_of_path(p)) & set(_RUST)]
    if rc != 0 or hand:
        return no("the merge tree is not the union of its parents (%s)"
                  % (", ".join(hand[:5]) or "no diff"))
    since = _git(repo, "merge-base", p1, p2).stdout.strip()
    if not since:
        return no("no merge base")
    return ("stage:merge narrowed parent=%s strand-run=%s" % (p2[:9], strand_run), since, [])


_WAVE_DATE = re.compile(r"-[0-9]{4}-[0-9]{2}-[0-9]{2}$")


def wave_key(wave):
    """`wave` without its date: `strand.sh` passes `welle-x` (`wave_name`), a
    start may name `welle-x-2026-10-10` -- the same wave (re-check H2 M1)."""
    return _WAVE_DATE.sub("", str(wave or "").strip())


def _same_wave(recorded, wave):
    """Does a start recorded for `recorded` belong to `wave`? A name with its
    date is one wave: it takes its own starts and those without a date
    (re-check m-N2: `welle-fix-2026-10-01` is not `welle-fix-2026-10-20`). A
    name without its date takes every start of that name -- two dated waves
    behind it are two starts, a contradiction (full suite)."""
    recorded, wave = str(recorded or "").strip(), str(wave or "").strip()
    if wave_key(recorded) != wave_key(wave):
        return False
    if _WAVE_DATE.search(wave) and _WAVE_DATE.search(recorded):
        return recorded == wave
    return True


def wave_starts(db, wave):
    """The distinct start revs of `wave` (`wave_started`), oldest first."""
    out = []
    if db is None or not wave_key(wave):
        return out
    for row in db.execute("SELECT data FROM journal WHERE event='wave_started' ORDER BY seq ASC"):
        data = json.loads(row[0] or "{}")
        start = data.get("start")
        if _same_wave(data.get("wave"), wave) and start and start not in out:
            out.append(start)
    return out


def wave_start(db, wave):
    """The start rev of `wave`: its FIRST `wave_started` (KR-TB-1 b, re-check
    H2 M2: a late or second start never moves the base), or None."""
    starts = wave_starts(db, wave)
    return starts[0] if starts else None


def integration_decision(repo, wave, db):
    """`(term, since, notes)` of an integration at HEAD: the wave start when it
    is known and an ancestor of HEAD, else `(None, None, why)` -- full suite."""
    def no(why):
        return None, None, ["narrowing: integration %s" % why]
    if not wave:
        return no("has no wave start: no wave named (%s)" % WAVE_ENV)
    if db is None:
        return no("has no wave start: no store")
    starts = wave_starts(db, wave)
    if not starts:
        return no("has no wave start: wave %s has no start in the store" % wave)
    if len(starts) > 1:
        return no("wave %s has %d different starts (%s) -- contradiction"
                  % (wave, len(starts), ", ".join(s[:9] for s in starts)))
    start = starts[0]
    sha = _commit(repo, start)
    if not sha or _git(repo, "merge-base", "--is-ancestor", sha, "HEAD").returncode != 0:
        return no("wave start %s of %s is not an ancestor of HEAD" % (start[:9], wave))
    return "stage:integration narrowed since=%s" % sha[:9], sha, []
