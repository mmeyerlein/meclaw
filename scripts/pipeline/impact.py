"""`pipeline impact` -- which stations and tests a diff needs, and why (W6, GH #1120).

WHY THIS EXISTS
===============
The resolver (`gate_plan.py`, rules 1-10) is the floor and stays the floor:
`impact` never takes a station or a test away from it, except in ONE place --
in `strand`/`pre`, and in a merge or an integration the caller found
narrowable, it may replace a crate-level `rdeps(X)` by the test parts that
the coverage map says execute the changed files, and only when every changed
source of crate X is in a fresh map. Over-selection is allowed,
under-selection is not (development-rules section 7); the replay probe
(`--replay`) is the evidence.

THE STAGE RULE
==============
    every stage but release
                       (0: graph names from before the harness fold, #1153:
                          a test binary the graph names but the tree no longer
                          has maps over the TREE (exactly one harness `main.rs`
                          declaring a plain top-level `mod <stem>;`) to its
                          folded term
                          `(binary_id(=<c>::<harness>) & test(/^<stem>::/))`,
                          only when that term hits >= 1 test of the harness;
                          else rdeps(<c>) with `graph:gone ...` -- never
                          silently dropped, and the map never replaces it;
                          a UNIT address `<c>::<harness>/<stem>` the tree
                          holds is the reader unit, planned as the floor
                          plans it (`gate_plan._unit_users`), one the tree
                          lost looks for `<stem>` in every harness, GH #1203)
    pre, strand        (a: floor terms on file level -- rules 3, 4, 5, 8; rule 8
                          includes the runtime reads, GH #1138)
                       + (b: graph -- tests that `reads`/`references`/`use` a
                          changed file)
                       + (c: coverage -- parts that cover a changed file)
                       + rdeps(X) for every crate X with a changed source the
                         map does not know (new file, no `covers` edge, stale
                         map, no map)
                       + crate roots (A, GH #1159): a changed `lib.rs`/`mod.rs`
                         the map has no line of plans the map parts of
                         every module of its crate (+ its own unit term when it
                         holds tests) -- only when the change since the map rev
                         up to the WORKING TREE is whitespace, non-doc comments
                         or bodies of non-const fns without crate-wide items
                         (`crate_root`); anything else keeps rdeps(X); a
                         binary root (`main.rs`, `src/bin/`) always keeps it:
                         the map does not measure spawned binaries
                       + rdeps(X) a floor rule other than rule 2 put there
                         (rule 8 runtime `unread`: the map cannot see a file
                         read, so it never replaces that rdeps; rule 10
                         `crate-file`: a crate file outside src/ and tests/,
                         e.g. an `include_str!` schema -- the map lists `.rs`
                         lines only)
                       + all() when the floor says all() (workspace class)
                       + the files changed since the map rev the diff does not
                         hold (`narrow.since_plan`, M-4, welle-tb S1b): a Rust
                         source or crate file plans its floor and keeps
                         rdeps(X) the map never replaces (it knows the old
                         file); a workspace file on `narrow.NO_SHIFT` plans
                         nothing, nor does a Cargo manifest or Cargo.lock
                         whose change is the merge point's version bump alone
                         (`narrow._version_only`, V2-A3); every other file
                         plans its floor like a diff path (any other
                         workspace file: all())
    merge              narrowed (`narrowed` = `stage:merge narrowed parent=<sha>
                         strand-run=<id>`, decided by `narrow.merge_decision`:
                         one strand parent, its strand gate GREEN on the same
                         source tree, no revert, no conflict resolution, a
                         fitting map): as strand, over merge base..merge.
                       Else (a) + (b), crate level as the floor plans it; the
                         map only produces a NOTE ("would have spared n
                         binaries")
    integration        narrowed (`narrowed` = `stage:integration narrowed
                         since=<sha>`, `narrow.integration_decision`: the
                         wave's start from the store, an ancestor of HEAD): as
                         strand over the wave diff <start>..tip, EXCEPT
                         all() -- the full suite -- when no fitting map is in
                         use or a changed file has an unknown reader (rule 8
                         runtime `unread`), KR-TB-1. A gone name without a
                         folded form stays rdeps(X) (0).
                       Else as merge unnarrowed (the caller plans the full
                         suite when the wave start is unknown)
    release            everything

Every term carries its reasons: `rule:<n> ...` (the floor), `graph:<kind> ...`,
`cover:file:<path>`, `cover:crate-root file:<path>`, `new:file:<path>`,
`stage:release`, and in a narrowed merge or integration its `stage:...` term.

A pure function: graph document + coverage map + diff + stage -> selection.
"""

import argparse
import collections
import datetime
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile

_SCRIPTS = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
if _SCRIPTS not in sys.path:
    sys.path.insert(0, _SCRIPTS)

import gate_plan  # noqa: E402
from pipeline import coverage as coverage_mod  # noqa: E402
from pipeline import crate_root  # noqa: E402
from pipeline import graph as graph_mod  # noqa: E402
from pipeline import spawn_closure  # noqa: E402

STAGES = ("pre", "strand", "merge", "integration", "release")
MODE_OF = {"pre": "strand", "strand": "strand", "merge": "integration",
           "integration": "integration", "release": "release"}
STALE_DAYS = 7  # = coverage.STALE_DAYS
USE_DEPTH = 2
REPLAY_DIRS = ("crates", "templates", "examples", "docs", "scripts", "workshop", ".github",
               ".config", ".cargo", "Cargo.toml", "Cargo.lock", "README.md", "CHANGELOG.md",
               "CONTRIBUTING.md", "ROADMAP.md", "deny.toml", "rust-toolchain.toml", "sets")


# --- terms ------------------------------------------------------------------

def part_term(binary, module=None):
    if module:
        return "(binary_id(=%s) & test(/^%s::/))" % (binary, module)
    return "binary_id(=%s)" % binary


def _binary_of_term(term):
    if term.startswith("(binary_id(="):
        return term[len("(binary_id(="):term.index(")")]
    return graph_mod.binary_id_of_term(term)


def own_unit_term(crate, path, repo=None):
    """The unit-test term of `path`'s own module in the lib binary of
    `crate` (review Z-I gap I-A): `src/a/b.rs` and `src/a/b/mod.rs` ->
    `(binary_id(=crate) & test(/^a::b::/))`, `src/lib.rs` -> the whole
    lib binary. None when the file is no lib module (`src/main.rs`,
    `src/bin/**`, no `src/lib.rs` in `repo`, not under `src/`) -- the
    caller keeps `rdeps(crate)`. Limit: a module that `src/main.rs` of a
    crate with both roots declares counts as lib (none does today)."""
    prefix = "crates/%s/src/" % crate
    if not path.startswith(prefix) or not path.endswith(".rs"):
        return None
    rel = path[len(prefix):-len(".rs")]
    if rel == "main" or rel == "bin" or rel.startswith("bin/"):
        return None
    if repo is not None and not os.path.isfile(os.path.join(repo, prefix, "lib.rs")):
        return None
    if rel == "lib":
        return part_term(crate)
    segs = rel.split("/")
    if segs[-1] == "mod":
        segs = segs[:-1]
    if not segs or not all(re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", x) for x in segs):
        return None
    return part_term(crate, "::".join(segs))


def coverage_index(coverage):
    """`{file: [(binary, module)]}` -- the parts with >= 1 executed line in the file."""
    out = collections.defaultdict(list)
    for part in (coverage or {}).get("parts", []):
        for rel, lines in part.get("files", {}).items():
            if lines > 0:
                out[rel].append((part["binary"], part.get("module")))
    return out


# --- the graph layer (b) ----------------------------------------------------

def _test_owner_bins(ix, file_addr, seen=None):
    """The test binaries a file under `crates/*/tests/` belongs to."""
    seen = seen or set()
    if file_addr in seen:
        return set()
    seen.add(file_addr)
    out = {e["to"] for e in ix.out.get(file_addr, ()) if e["kind"] == "contains"
           and e["to"].startswith("bin:")}
    for e in ix.inc.get(file_addr, ()):
        if e["kind"] == "contains" and e["from"].startswith("file:crates/") \
                and "/tests/" in e["from"]:
            out |= _test_owner_bins(ix, e["from"], seen)
    return out


def graph_tests(ix, rel):
    """`{bin addr: [reason]}` -- the tests the graph ties to the changed file `rel`."""
    out = collections.defaultdict(set)
    addr = "file:%s" % rel
    for e in ix.inc.get(addr, ()):
        if e["kind"] == "reads" and e["from"].startswith("bin:"):
            out[e["from"]].add("graph:reads file:%s" % rel)
        elif e["kind"] == "reads" and e["from"].startswith("file:"):
            # rule 8 runtime, script shape (GH #1138): rel is a generator's source
            for s in ix.inc.get(e["from"], ()):
                if s["kind"] == "reads" and s["from"].startswith("bin:"):
                    out[s["from"]].add("graph:reads file:%s via %s" % (rel, e["from"][5:]))
    # rule 8 runtime, dir shape: a test that walks an ancestor directory --
    # under crates/ at any depth (`crates`, `crates/<c>`, `crates/<c>/tests`):
    # the graph hangs a crate-relative bare `tests` on no node, so every
    # `dir:` node here is a walk of that directory (tb-s1c, #1177). Below
    # crates/ a `.rs` or a file outside tests/ is read only by a walk written
    # repo-relative (`form: repo`, gh523/gh230 read every `.rs` below
    # `crates`), as in `_runtime_readers`; an edge without `form` (a graph
    # built before) counts as crate-relative.
    crate = gate_plan._crate_of(rel)
    fixture = not crate or (rel.split("/")[2:3] == ["tests"] and not rel.endswith(".rs"))
    for d in gate_plan.runtime_ancestors(rel):
        if not gate_plan.runtime_dir_applies(d, rel):
            continue
        for e in ix.inc.get("dir:%s" % d, ()):
            if e["kind"] == "reads" and e["from"].startswith("bin:") and (
                    fixture or (e.get("data") or {}).get("form") == "repo"):
                out[e["from"]].add("graph:reads dir:%s" % d)
    parts = rel.split("/")
    if parts[0] in ("templates", "examples") and len(parts) >= 3:
        owner = "%s:%s" % ("template" if parts[0] == "templates" else "example", parts[1])
        for e in ix.inc.get(owner, ()):
            if e["kind"] == "references" and e["from"].startswith("bin:"):
                out[e["from"]].add("graph:references %s" % owner)
    frontier, seen = [addr], {addr}
    for _ in range(USE_DEPTH):
        nxt = []
        for a in frontier:
            for e in ix.inc.get(a, ()):
                if e["kind"] != "use" or e["from"] in seen:
                    continue
                seen.add(e["from"])
                nxt.append(e["from"])
                if "/tests/" in e["from"]:
                    for b in _test_owner_bins(ix, e["from"]):
                        out[b].add("graph:use file:%s via %s" % (rel, e["from"]))
        frontier = nxt
    if "/tests/" in rel and rel.startswith("crates/"):
        for b in _test_owner_bins(ix, addr):
            out[b].add("graph:contains file:%s" % rel)
    return out


# --- binaries ---------------------------------------------------------------

def _crate_of_binary(bid):
    return bid.split("::", 1)[0]


def rdeps(ix, crate):
    """`{crate}` -- `crate` and every workspace crate that depends on it, transitively."""
    out, frontier = {crate}, [crate]
    while frontier:
        nxt = []
        for c in frontier:
            for e in ix.inc.get("crate:%s" % c, ()):
                if e["kind"] == "depends":
                    d = e["from"][len("crate:"):]
                    if d not in out:
                        out.add(d)
                        nxt.append(d)
        frontier = nxt
    return out


def render_filter(terms, stage, repo):
    """The nextest filter of `terms` -- the one rendering, so the narrowing
    runs exactly what it plans (B-2): None without terms; outside release and the
    bare floor `gate_plan.tests_minus_for` subtracted -- the scenario class and
    every browser unit (rule 11, GH #1161 re-check M-3)."""
    terms = sorted(terms)
    if not terms:
        return None
    if stage == "release" or terms == [gate_plan.T0_FLOOR]:
        return " + ".join(terms)
    expr = " + ".join(terms)
    scen = gate_plan.tests_minus_for(repo)
    return "%s - (%s)" % (expr, scen) if scen else expr


def expand(ix, terms):
    """`(binaries, parts)` a list of filterset terms selects."""
    universe = [n["addr"][4:] for n in ix.nodes.values() if n["kind"] == "bin"
                and n.get("source")]
    bins, parts = set(), set()
    for t in terms:
        if t in ("all()", gate_plan.T0_FLOOR):
            sel = set(universe) if t == "all()" else {b for b in universe if "::" not in b}
            bins |= sel
            parts |= sel
        elif t.startswith("rdeps("):
            crates = rdeps(ix, t[len("rdeps("):-1])
            sel = {b for b in universe if _crate_of_binary(b) in crates}
            bins |= sel
            parts |= sel
        else:
            b = _binary_of_term(t)
            if b:
                bins.add(b)
                parts.add(t)
    return bins, parts, set(universe)


# --- binaries the graph names from before the harness fold -------------------

def _plain_top_level_mod(text, stem):
    """Does `text` declare `[pub[(..)]] mod <stem>;` at brace depth 0 with no
    attribute on it (the harness_fold shape)? A `#[cfg]`-gated, `#[path]`-moved
    or nested declaration is doubt: False (re-check H1, m3)."""
    try:
        toks = crate_root.tokens(text)
    except crate_root.Doubt:
        return False
    depth = 0
    for k, t in enumerate(toks):
        if t == "{":
            depth += 1
        elif t == "}":
            depth -= 1
        elif depth == 0 and t == "mod" and toks[k + 1:k + 3] == [stem, ";"]:
            j = k - 1
            if j >= 0 and toks[j] == ")":
                while j >= 0 and toks[j] != "(":
                    j -= 1
                j -= 1
            if j >= 0 and toks[j] == "pub":
                j -= 1
            if j >= 0 and toks[j] == "]":
                return False
            return True
    return False


def harness_folds(repo, crate, harness, stem):
    """Does `crates/<crate>/tests/<harness>/main.rs` fold `<stem>.rs` -- a plain
    `mod <stem>;` at its top level (`gate_plan._folded_unit` and
    `_plain_top_level_mod`)? The one tree check `folded_form` and
    `checked_graph_terms` share (R-1, GH #1199): a file that stands but is
    orphaned, `#[cfg]`-gated or `#[path]`-renamed is no fold."""
    if not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", stem or ""):
        return False
    tests = os.path.join(gate_plan.REPO_ROOT if repo is None else repo, "crates", crate, "tests")
    main = os.path.join(tests, harness, "main.rs")
    if not (os.path.isfile(main) and os.path.isfile(os.path.join(tests, harness, stem + ".rs"))):
        return False
    with open(main, encoding="utf-8", errors="replace") as fh:
        text = fh.read()
    return (gate_plan._folded_unit(text, harness, stem) == "%s/%s" % (harness, stem)
            and _plain_top_level_mod(text, stem))


def unit_held(repo, crate, unit):
    """Does the tree hold the folded unit `<harness>/<stem>` the graph names?

    R-1 (welle-validierung-2, GH #1199): the file standing was the whole check
    (`gate_plan._test_binary_exists`); a harness that no longer folds the module
    then planned `(binary_id(=<c>::<h>) & test(/^<stem>::/))` with 0 tests. The
    unit `<h>/main` is the harness itself. Any other unit counts only when its
    harness folds it (`harness_folds`); its tests are then `<stem>::*`, and a
    folded module without a test of its own is a helper for which
    `gate_plan._unit_users` plans the whole harness -- never a 0-test term.
    """
    harness, _, mod = unit.partition("/")
    if not gate_plan._test_binary_exists(repo, crate, unit):
        return False
    return mod == "main" or harness_folds(repo, crate, harness, mod)


def folded_form(repo, bid):
    """`(term, harness, why)` of the pre-fold binary `crate::stem` in the tree.

    welle-tb S1 part 0 (GH #1159): the graph of 28c948e15 named binaries the
    harness fold (#1153) had folded away -- up to 398 "gone from the tree"
    NOTEs per measured case, each a test the graph tied to the diff and the
    plan then dropped. harness_fold (rules: `tests/HARNESS.toml`) turns
    `tests/<stem>.rs` into `tests/<harness>/<stem>.rs` plus `mod <stem>;` in the
    harness `main.rs`; the old binary's tests are exactly `<stem>::*` of
    `<crate>::<harness>` (W5-2c). The TREE is the proof, not the rule table:
    the term counts only when exactly one harness holds the module AND it has
    >= 1 test attribute (it hits a test of the harness). `term` is None
    otherwise -- the caller plans rdeps of the crate (fail-closed).
    """
    crate, _, stem = bid.partition("::")
    if not stem or not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", stem):
        return None, None, "no folded form"
    tests = os.path.join(gate_plan.REPO_ROOT if repo is None else repo, "crates", crate, "tests")
    hits = []
    try:
        entries = sorted(os.listdir(tests))
    except OSError:
        entries = []
    for h in entries:
        if harness_folds(repo, crate, h, stem):
            hits.append(h)
    if len(hits) != 1:
        return None, None, ("no folded form" if not hits
                            else "folded into %d harnesses (%s)" % (len(hits), ", ".join(hits)))
    harness = hits[0]
    with open(os.path.join(tests, harness, stem + ".rs"), encoding="utf-8",
              errors="replace") as fh:
        text = fh.read()
        count = crate_root.test_attribute_count(text)
        if (gate_plan._test_counts(text)[0] if count is None else count) == 0:
            return None, harness, "folded term %s::%s %s:: hits no test" % (crate, harness, stem)
    return gate_plan._binary_id(crate, "%s/%s" % (harness, stem)), harness, None


# --- the function -----------------------------------------------------------

def checked_graph_terms(ix, added, floor, repo, notes):
    """`{term: reasons}` of the graph's additions that nextest can run (review I3).

    The graph may be older than the tree: a binary whose source is gone
    would be a `binary_id` nextest rejects (the whole `tests` station red),
    and a binary whose every test is ignored runs none (rule 9, #997). A gone
    binary maps to its folded harness term (`folded_form`), or -- no folded
    form, or one that hits no test -- to rdeps of its crate: never silently
    away (welle-tb S1 part 0). A term the resolver floor names anyway stays.

    GH #1203, #1202: the graph hangs `reads`/`references` on the UNIT address
    of a folded module, `bin:<c>::<harness>/<stem>` (`graph._unit_user_addrs`
    and the `reads` loop over `gate_plan.test_sources`) -- no node, no binary
    of its own. Taken for a gone binary (no node, and `folded_form` knows no
    stem with a `/`), it planned rdeps(<c>) for a unit the tree holds: one
    assert in meclaw-core::misc planned 10 411 tests (M-ROT-1-W, `tests`
    784 s, expected ~17 s), one line of the development rules page the suite
    (M-STR-doku, 719 s, expected ~41 s). A unit the tree holds plans what the
    floor plans for a reader unit (`gate_plan._unit_users`: the unit, a
    folded helper's harness, its sharers); one the tree lost goes through
    `folded_form` with its module name (moved to another harness), else
    rdeps(<c>) as before.
    """
    root = gate_plan.REPO_ROOT if repo is None else repo
    out = collections.defaultdict(set)
    for b, rs in added.items():
        bid = b[len("bin:"):]
        crate, _, stem = bid.partition("::")
        unit = "/" in stem
        if unit and unit_held(repo, crate, stem):
            for t in gate_plan._unit_users(repo, crate, stem):
                out[t] |= set(rs)
            continue
        term = part_term(bid)
        src = (ix.nodes.get(b) or {}).get("source")
        if unit or (term not in floor and not (src and os.path.isfile(os.path.join(root, src)))):
            folded, harness, why = folded_form(
                repo, "%s::%s" % (crate, stem.split("/", 1)[1]) if unit else bid)
            if folded is not None:
                notes.append("graph: %s is gone from the tree -- folded into %s::%s"
                             % (bid, _crate_of_binary(bid), harness))
                out[folded] |= set(rs) | {"graph:gone %s folded into %s::%s"
                                          % (bid, _crate_of_binary(bid), harness)}
            else:
                notes.append("graph: %s is gone from the tree, %s -- rdeps(%s)"
                             % (bid, why, _crate_of_binary(bid)))
                out["rdeps(%s)" % _crate_of_binary(bid)] |= {"graph:gone %s -- %s" % (bid, why)}
            continue
        out[term] |= set(rs)
    cands = [t for t in out if t not in floor and "::" in (_binary_of_term(t) or "")]
    runs = gate_plan._drop_binaries_without_tests(repo, cands)
    for t in cands:
        if t not in runs:
            notes.append("graph: %s runs no test (rule 9) -- not selected" % _binary_of_term(t))
            del out[t]
    return dict(out)


def crate_root_parts(cidx, crate, path, repo, rev, notes):
    """`{term: reasons}` for a changed crate root `path` the map has no line of,
    or None -- the caller keeps rdeps(crate) (welle-tb S1 part A, GH #1159).

    Only the positive list of `crate_root` (whitespace, non-doc comments,
    non-const fn bodies between the map's `rev` and the working tree -- what
    the plan reads, dirty tree included) plans the map parts of every module
    of the crate. A root with tests of its own adds its own unit term. No
    part at all is doubt as well. A binary root (`main.rs`, `src/bin/`) is
    doubt: the map does not measure spawned binaries, so "no line" there is
    not "never runs" (re-check H1, M1).
    """
    if not crate_root.is_crate_root(path):
        return None
    if path.rsplit("/", 1)[-1] == "main.rs" or "/src/bin/" in path:
        notes.append("crate root %s is a binary root the map does not measure -- "
                     "rdeps(%s) kept" % (path, crate))
        return None
    if not crate_root.root_change_is_harmless(repo, rev, path, tree=True):
        notes.append("crate root %s: change outside the positive list (comments, "
                     "whitespace, non-const fn bodies) -- rdeps(%s) kept" % (path, crate))
        return None
    prefix = "crates/%s/src/" % crate
    out = collections.defaultdict(set)
    for f in sorted(cidx):
        if f.startswith(prefix):
            for b, m in cidx[f]:
                out[part_term(b, m)].add("cover:crate-root file:%s" % path)
    head = crate_root.tree_text(repo, path) or ""
    own_tests = crate_root.test_attribute_count(head)
    if own_tests != 0 or "test" in crate_root.tokens(head):
        own = own_unit_term(crate, path, repo)
        if own is None:
            notes.append("crate root %s holds tests and is no lib module -- rdeps(%s) kept"
                         % (path, crate))
            return None
        out[own].add("cover:own-module file:%s" % path)
    if not out:
        notes.append("crate root %s: the map knows no module of %s -- rdeps kept" % (path, crate))
        return None
    return dict(out)


def impact(paths, stage, doc, repo=None, coverage=None, now=None, stations=True, rev=None,
           tree_rev=None, narrowed=None, no_map=False):
    """Selection for `paths` at `stage` over the graph `doc` (and the coverage map).

    The map narrows nothing unless it is COMPLETE (`coverage.is_complete`),
    fresh, and of the matching rev: `rev` (the base the diff is planned
    against), by default the graph's own rev (review C1). Any other map is
    as good as none: `rdeps(X)` stays.

    `narrowed` is the reason term of a merge or integration the caller found
    narrowable (`narrow.merge_decision` / `narrow.integration_decision`):
    that stage then plans like strand, and every term carries the reason.
    Release ignores it.

    `no_map` (re-check H2 M3): the caller planned a python/docs-only diff
    without the map on purpose (`narrow.non_rust`) -- a narrowed integration
    then plans floor + graph instead of the full suite of "no fitting map";
    an unknown reader still plans the full suite.
    """
    if stage not in STAGES:
        raise ValueError("stage must be one of %s" % (STAGES,))
    files = gate_plan._norm(paths)
    ix = graph_mod.Index(doc)
    reason = collections.defaultdict(set)
    notes = []
    if tree_rev is not None and doc.get("rev") != tree_rev:
        notes.append("graph rev %s is not HEAD %s" % (doc.get("rev"), tree_rev))
    mode = MODE_OF[stage]

    station_names = []
    if stations:
        classes = gate_plan.classify(files)
        for st in gate_plan.plan(files, mode, repo):
            if st.name in station_names:
                continue
            station_names.append(st.name)
            hits = [e["to"][len("class:"):] for e in ix.out.get("station:%s" % st.name, ())
                    if e["kind"] == "triggered_by" and e["to"][len("class:"):] in classes]
            for c in hits:
                reason[st.name].add("class:%s" % c)
            if not hits:
                reason[st.name].add("resolver:%s" % mode)

    cov_state = {"used": False, "rev": None, "stale": None, "complete": None,
                 "rev_match": None, "narrowed": None}
    if stage not in ("merge", "integration"):
        narrowed = None
    cov_state["narrowed"] = narrowed
    if stage == "release":
        tests = {"all()": {"stage:release"}}
    elif not files:
        tests = {gate_plan.T0_FLOOR: {"rule:7 empty diff"}}
    else:
        floor = gate_plan.test_terms(files, repo)
        tests = collections.defaultdict(set)
        for t, rs in floor.items():
            tests[t] |= set(rs)
        added = collections.defaultdict(set)
        for rel in files:
            for b, rs in graph_tests(ix, rel).items():
                added[b] |= rs
        for t, rs in checked_graph_terms(ix, added, floor, repo, notes).items():
            tests[t] |= rs
        cidx = None
        if coverage:
            want = rev if rev is not None else doc.get("rev")
            cov_state["rev"] = coverage.get("rev")
            stale = coverage_mod.age_days(coverage, now) > STALE_DAYS
            complete = coverage_mod.is_complete(coverage)
            match = coverage.get("rev") == want
            cov_state.update(stale=stale, complete=complete, rev_match=match)
            if not complete:
                notes.append("coverage map %s is not complete -- rdeps kept"
                             % coverage.get("rev"))
            elif not match:
                notes.append("coverage map rev %s is not %s -- rdeps kept"
                             % (coverage.get("rev"), want))
            if not stale and complete and match:
                cidx = coverage_index(coverage)
        strand_like = stage in ("pre", "strand") or narrowed is not None
        if narrowed is not None and stage == "integration" and "all()" not in tests:
            # KR-TB-1 (c): no fitting map, or a file whose readers nobody
            # knows (rule 8 runtime `unread`) -- the full suite.
            unread = sorted(r for rs in tests.values() for r in rs
                            if r.startswith("rule:8 runtime unread "))
            why = None
            if cidx is None and not no_map:
                why = "no fitting coverage map"
            elif unread:
                why = "unknown reader (%s)" % "; ".join(unread)
            if why:
                notes.append("integration: %s -- full suite" % why)
                tests = {"all()": {narrowed, "stage:integration full suite: %s" % why}}
        if strand_like and cidx is not None and "all()" not in tests:
            for t in [t for t in list(tests) if t.startswith("rdeps(")]:
                crate = t[len("rdeps("):-1]
                # GH #1138: the map replaces only what it can see -- Rust
                # sources (rule 2). An rdeps a runtime read put there (rule 8
                # runtime unread: a crate test file no test names) stays, and
                # so does one of a crate file (rule 10, welle-tb S1b).
                blind = sorted(r for r in tests[t] if not r.startswith("rule:2 "))
                if blind:
                    notes.append("%s not seen by the coverage map -- rdeps(%s) kept"
                                 % ("; ".join(blind), crate))
                    continue
                changed = [p for p in files if gate_plan._crate_of(p) == crate
                           and "rust_src" in gate_plan._classes_of_path(p)]
                roots = {}
                for p in changed:
                    if p not in cidx and crate_root.is_crate_root(p):
                        got = crate_root_parts(cidx, crate, p, repo, coverage.get("rev"), notes)
                        if got is not None:
                            roots[p] = got
                unknown = [p for p in changed if p not in roots
                           and (p not in cidx or own_unit_term(crate, p, repo) is None)]
                if unknown:
                    for p in unknown:
                        if p in cidx:   # covered, but no lib module (main.rs, src/bin/)
                            tests[t].add("cover:no-lib-module file:%s" % p)
                            notes.append("%s is no lib module -- rdeps(%s) kept" % (p, crate))
                            continue
                        why = "new:file:%s" % p if ("file:%s" % p) not in ix.nodes \
                            else "cover:unknown file:%s" % p
                        tests[t].add(why)
                    continue
                kept = tests.pop(t)
                cov_state["used"] = True
                for p in changed:
                    if p in roots:
                        for term, rs in roots[p].items():
                            tests[term] |= rs
                        continue
                    for b, m in cidx[p]:
                        tests[part_term(b, m)].add("cover:file:%s" % p)
                    # review Z-I gap I-A: the map only knows lines that ran; a
                    # new `#[test]` in p runs in the unit binary, module of p.
                    tests[own_unit_term(crate, p, repo)].add("cover:own-module file:%s" % p)
                notes.append("rdeps(%s) replaced by coverage (%s)" % (crate, "; ".join(sorted(kept))))
        elif stage in ("merge", "integration") and narrowed is None and cidx is not None:
            sel = [t for t in tests if not t.startswith("rdeps(") and t != "all()"]
            for t in [t for t in tests if t.startswith("rdeps(")]:
                crate = t[len("rdeps("):-1]
                changed = [p for p in files if gate_plan._crate_of(p) == crate
                           and "rust_src" in gate_plan._classes_of_path(p)]
                if changed and all(p in cidx and own_unit_term(crate, p, repo) for p in changed):
                    sel += [part_term(b, m) for p in changed for b, m in cidx[p]]
                    sel += [own_unit_term(crate, p, repo) for p in changed]
                else:
                    sel.append(t)
            full, _, _ = expand(ix, list(tests))
            narrow, _, _ = expand(ix, sel)
            if len(narrow) < len(full):
                notes.append("coverage would have spared %d binaries" % (len(full) - len(narrow)))
        if strand_like and cidx is not None and "all()" not in tests:
            # S1b (#1165): the map sees a spawned binary's code only where it
            # proves that bin -- every other spawner of a changed closure runs
            for t, rs in spawn_closure.fail_closed_terms(
                    files, coverage, repo, lambda c: rdeps(ix, c), kept=list(tests)).items():
                tests[t] |= rs
        # new files: their tests are in (rule 3) -- say so
        for rel in files:
            if ("file:%s" % rel) not in ix.nodes and "rust_test" in gate_plan._classes_of_path(rel):
                for t in tests:
                    if _binary_of_term(t) and rel.split("/")[3].removesuffix(".rs") in t:
                        tests[t].add("new:file:%s" % rel)
    if narrowed is not None:
        for t in tests:
            tests[t].add(narrowed)
    for t, rs in tests.items():
        reason[t] |= set(rs)
    terms = sorted(tests)
    if stage != "release":
        # rule 11 of the resolver (GH #1161, re-check M-3): a browser unit runs in
        # `browser:display`, never in `tests` -- dropped here, subtracted below
        browser = set(gate_plan.browser_terms(repo))
        terms = [t for t in terms if t not in browser]
    bins, parts, universe = expand(ix, terms)
    filt = render_filter(terms, stage, repo)
    return {"stage": stage, "stations": station_names, "tests": terms, "filter": filt,
            "reason": {k: sorted(v) for k, v in sorted(reason.items())},
            "binaries": {"selected": len(bins), "parts": len(parts), "total": len(universe)},
            "coverage": cov_state, "notes": notes}


# --- the replay probe -------------------------------------------------------

def _archive(repo, rev, dest):
    """Unpack the tree of `rev` (the dirs the graph reads) into `dest` -- read-only on the repo."""
    have = subprocess.run(["git", "-C", repo, "ls-tree", "--name-only", rev],
                          capture_output=True, text=True, check=True).stdout.split()
    want = [p for p in REPLAY_DIRS if p in have]
    tar = subprocess.Popen(["git", "-C", repo, "archive", rev] + want, stdout=subprocess.PIPE)
    subprocess.run(["tar", "-x", "-C", dest], stdin=tar.stdout, check=True)
    tar.wait()


def red_binaries(case):
    out = set()
    for r in case.get("red_binaries") or case.get("red") or []:
        parts = r.split("::")
        out.add("::".join(parts[:2]) if len(parts) >= 2 else parts[0])
    return out


def replay(cases, repo, coverage=None, stage="strand", workdir=None):
    """Plan each case's diff at `stage` on its own tree; a red binary outside the plan is a miss."""
    rows = []
    cache = {}
    tmp = workdir or tempfile.mkdtemp(prefix="impact-replay-")
    try:
        for case in cases:
            dest = os.path.join(tmp, case["id"])
            os.makedirs(dest, exist_ok=True)
            if not os.listdir(dest):
                _archive(repo, case["rev"], dest)
            gate_plan._SOURCE_CACHE.pop(dest, None)
            g, _ = graph_mod.build(dest, cache=cache, stations=False,
                                   files=graph_mod.tree_files(dest))
            doc = g.to_json(case["rev"], "replay")
            # The probe measures the GIVEN map on every case's tree: its rev is
            # taken as the base on purpose. A map older than the case can only
            # miss more, so a pass is the stronger evidence. Completeness is
            # still required.
            res = impact(case["paths"], stage, doc, repo=dest, coverage=coverage,
                         stations=False, rev=(coverage or {}).get("rev"))
            bins, _, universe = expand(graph_mod.Index(doc), res["tests"])
            red = red_binaries(case)
            missed = sorted(red - bins)
            why = {}
            for b in sorted(red & bins):
                for t in res["tests"]:
                    if (_binary_of_term(t) == b) or t == "all()" or (
                            t.startswith("rdeps(") and _crate_of_binary(b) in
                            rdeps(graph_mod.Index(doc), t[6:-1])):
                        why[b] = (t, res["reason"].get(t, []))
                        break
            rows.append({"id": case["id"], "selected": len(bins), "total": len(universe),
                         "red": sorted(red), "missed": missed, "why": why})
    finally:
        if workdir is None:
            shutil.rmtree(tmp, ignore_errors=True)
    return rows


# --- CLI --------------------------------------------------------------------

def _cmd_impact(args):
    try:
        doc = graph_mod.load(args.graph)
    except FileNotFoundError as exc:
        print("impact: no graph (%s) -- run `scripts/pipeline.py graph build` first"
              % exc.filename, file=sys.stderr)
        return 2
    except (ValueError, KeyError, TypeError) as exc:
        # json.JSONDecodeError is a ValueError; a pointer without "path" a KeyError
        print("impact: broken graph (%s: %s) -- run `scripts/pipeline.py graph build` again"
              % (type(exc).__name__, str(exc).splitlines()[0] if str(exc) else ""),
              file=sys.stderr)
        return 2
    cov = None
    if args.coverage:
        cov, why = coverage_mod.load_map(args.coverage)
        if why:
            print("impact: %s -- build it again (`scripts/pipeline.py coverage build`)" % why,
                  file=sys.stderr)
            return 2
    if args.replay:
        with open(args.replay, encoding="utf-8") as fh:
            cases = json.load(fh)
        rows = replay(cases, args.repo, coverage=cov, stage=args.stage, workdir=args.workdir)
        missed = sum(1 for r in rows if r["missed"])
        for r in rows:
            print("%s\t%d/%d\t%s\t%s" % (r["id"], r["selected"], r["total"],
                                         "MISSED " + ",".join(r["missed"]) if r["missed"] else "ok",
                                         json.dumps(r["why"])))
        print("replay: %d cases, %d missed" % (len(rows), missed))
        return 1 if missed else 0
    if args.diff == "-":
        paths = sys.stdin.read().splitlines()
    else:
        with open(args.diff, encoding="utf-8") as fh:
            paths = fh.read().splitlines()
    if cov and args.db:
        coverage_mod.check_stale(cov, db_path=args.db)
    res = impact(paths, args.stage, doc, repo=args.repo, coverage=cov, rev=args.base,
                 tree_rev=graph_mod.tree_rev(args.repo))
    print(json.dumps(res, indent=1))
    return 0


def register(sub):
    """`pipeline impact --diff <file|-> [--stage]` / `impact --replay <cases.json>` (V0 sections 7, 9)."""
    p = sub.add_parser("impact", help="stations and tests a diff needs, with the reason for each")
    p.add_argument("--diff", default="-", help="file with one changed path per line ('-' = stdin)")
    p.add_argument("--stage", choices=STAGES, default="strand")
    p.add_argument("--repo", default=os.path.dirname(_SCRIPTS))
    p.add_argument("--graph", default=None, help="graph file (default: the latest build)")
    p.add_argument("--coverage", default=None, help="coverage map (coverage-<rev>.json)")
    p.add_argument("--base", default=None,
                   help="the rev the diff starts from; a map of another rev is not used"
                        " (default: the graph's rev)")
    p.add_argument("--replay", default=None, help="cases JSON: plan each red diff, count misses")
    p.add_argument("--workdir", default=None, help="replay: keep the unpacked trees here")
    p.add_argument("--db", default=None, help="store for `coverage_stale` (default: none)")
    p.set_defaults(func=_cmd_impact)
