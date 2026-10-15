"""`pipeline graph` -- the impact graph of the tree (W6, GH #1120; V0 section 7).

WHY THIS EXISTS
===============
`gate_plan.py` decides on crate level: a change in `crates/meclaw-cells/src`
plans `rdeps(meclaw-cells)`, which is every test binary of the workspace. The
graph is the file-level layer above that floor: which crate depends on which
(`depends`), which file holds which test binary (`contains`), which source a
Rust file imports (`use`), which file a test names as a literal (`reads`, rule
8 inverted), which template or example a test references (`references`), which
scaffold or set pins which template version (`pins`), which ADR a test anchors
(`pinned_by`), which scenario case needs which station (`needs`), which class
triggers which station (`triggered_by`) and -- from the coverage map -- which
test part executes which file (`covers`).

It is built deterministically from the tree, without a model and without
cargo: the manifests are read with `tomllib`, `cargo metadata` output is
accepted when a caller has it (`--metadata FILE`). Every source carries its
blob SHA (`mark`, the graph-space name); a rebuild re-extracts only the sources
whose mark moved (`<graph dir>/extract-cache.json`).

The vocabulary is graph-space's (`templates/graph-space`): `use` resolves like
graph-space section Resolution -- `crate::`, `super::`, `self::` and the
workspace crates by name; everything else is `external`. Macro-generated paths
stay invisible, which is why `impact` never stands on this parser alone.

FILE
====
    {"schema": "meclaw.pipeline.graph/1", "built_at", "rev", "code",
     "sources": [{addr, path, kind, mark}],
     "nodes": [{addr, kind, name, source}],
     "edges": [{from, to, kind, class, state, data?}]}

Private: `~/.local/state/meclaw/pipeline/graph/<rev>.json` (or
`$MECLAW_PIPELINE_GRAPH_DIR`). `pipeline graph build` journals `graph_built`.
"""

import argparse
import collections
import contextlib
import hashlib
import json
import os
import pathlib
import re
import subprocess
import sys
import tempfile
import time
import tomllib

_SCRIPTS = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
if _SCRIPTS not in sys.path:
    sys.path.insert(0, _SCRIPTS)

import gate_plan  # noqa: E402
from pipeline import locks  # noqa: E402

SCHEMA = "meclaw.pipeline.graph/1"
EDGE_KINDS = ("depends", "contains", "use", "reads", "references", "pins", "pinned_by",
              "needs", "triggered_by", "covers", "plans", "ran", "cached_from")
EDGE_CLASSES = ("extracted", "inferred")
EDGE_STATES = ("resolved", "unresolved", "broken", "external")
CACHE_FILE = "extract-cache.json"
EXTRACTOR_VERSION = 1

MEMORY_CASES = "workshop/evals/scenarios/cases"
BUILDER_CASES = "workshop/evals/builder-scenarios/cases"
BUILDER_SCAFFOLDS = "workshop/evals/builder-scenarios/scaffolds"
ADR_REGISTRY = ".github/gates/adr-anchors.tsv"
PIN_SURFACES = locks.SURFACES
SKIP_DIRS = frozenset({".git", "target", "node_modules", "__pycache__", "graphify-out"})
_STD = frozenset({"std", "core", "alloc", "crate", "self", "super", "proc_macro", "test"})


# --- small helpers -----------------------------------------------------------

def code_hash():
    """The code the edges come from: `gate_plan.py` and this file. A graph
    whose `code` differs was built by other resolver code -- `narrow` says so
    (re-check tb-s2 Zusatz-Check m-3, OR-TB.S2.11)."""
    data = b""
    for path in (gate_plan.__file__, __file__):
        with open(path, "rb") as fh:
            data += fh.read()
    return blob_sha(data)[:16]


def blob_sha(data):
    """Git's blob SHA-1 of `data` (bytes) -- the `mark` of a source."""
    h = hashlib.sha1()
    h.update(b"blob %d\0" % len(data))
    h.update(data)
    return h.hexdigest()


def graph_dir():
    env = os.environ.get("MECLAW_PIPELINE_GRAPH_DIR")
    if env:
        return pathlib.Path(env)
    return pathlib.Path.home() / ".local/state/meclaw/pipeline/graph"


def tree_files(root, untracked=True):
    """Repo-relative paths of the tree: git's tracked + untracked-not-ignored, else a walk.
    `untracked=False`: the tracked files alone -- the files of the head rev when no
    tracked file is changed (`fresh` builds the graph of the rev from them)."""
    root = str(root)
    if os.path.exists(os.path.join(root, ".git")):
        try:
            out = subprocess.run(
                ["git", "-C", root, "ls-files", "-z", "--cached"]
                + (["--others", "--exclude-standard"] if untracked else []),
                capture_output=True, check=True, env={**os.environ, "GIT_OPTIONAL_LOCKS": "0"})
            files = [p for p in out.stdout.decode("utf-8", "replace").split("\0") if p]
            return sorted(p for p in set(files) if os.path.isfile(os.path.join(root, p)))
        except (OSError, subprocess.CalledProcessError):
            pass
    out = []
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = sorted(d for d in dirnames if d not in SKIP_DIRS)
        for fn in filenames:
            out.append(os.path.relpath(os.path.join(dirpath, fn), root).replace(os.sep, "/"))
    return sorted(out)


def tree_rev(root):
    try:
        out = subprocess.run(["git", "-C", str(root), "rev-parse", "HEAD"],
                             capture_output=True, text=True, check=True,
                             env={**os.environ, "GIT_OPTIONAL_LOCKS": "0"})
        return out.stdout.strip()
    except (OSError, subprocess.CalledProcessError):
        return "worktree"


def _read(root, rel):
    try:
        with open(os.path.join(root, rel), "rb") as fh:
            return fh.read()
    except OSError:
        return b""


def bin_addr(binary_id):
    return "bin:%s" % binary_id


_PART_TERM_RE = re.compile(r"\(binary_id\(=[^:]+::([^)]+)\) & test\(/\^(.+)::/\)\)")


def _unit_user_addrs(root, crate_dir, unit, crate):
    """`{bin addr}` of rule 3's set for a `_test_sources` unit
    (`gate_plan._unit_users`): `binary_id(=c::b)` -> `bin:<crate>::b`, the
    part term of a folded module -> `bin:<crate>::<harness>/<stem>` (the unit
    address `impact` folds back). A term of another shape keeps the unit."""
    out = set()
    for t in gate_plan._unit_users(root, crate_dir, unit):
        bid, m = binary_id_of_term(t), _PART_TERM_RE.fullmatch(t)
        if bid is not None:
            out.add(bin_addr("%s::%s" % (crate, bid.split("::", 1)[1])))
        elif m:
            out.add(bin_addr("%s::%s/%s" % (crate, m.group(1), m.group(2))))
        else:
            out.add(bin_addr("%s::%s" % (crate, unit)))
    return out or {bin_addr("%s::%s" % (crate, unit))}


def binary_id_of_term(term):
    """`binary_id(=c::s)` -> `c::s`; anything else -> None."""
    m = re.fullmatch(r"binary_id\(=(.+)\)", term)
    return m.group(1) if m else None


# --- extraction (per source, cached by mark) --------------------------------

_USE_RE = re.compile(r"(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?use\s+([^;]+);")
_MOD_RE = re.compile(r"(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+([A-Za-z_][A-Za-z0-9_]*)\s*;")
_TEST_FN_RE = re.compile(
    r"#\[(?:[A-Za-z_][\w:]*::)?test(?:\([^\]]*\))?\]\s*(?:#\[[^\]]*\]\s*)*"
    r"(?:pub\s+)?(?:async\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)")


def _expand_use(tree):
    """`a::{b, c::{d, self}}` -> [[a,b], [a,c,d], [a,c]] (one level of grammar, nested)."""
    tree = re.sub(r"\s+", "", tree)
    tree = re.sub(r"\bas\w+$", "", tree)

    def split_top(s):
        parts, depth, cur = [], 0, ""
        for ch in s:
            if ch == "{":
                depth += 1
            elif ch == "}":
                depth -= 1
            if ch == "," and depth == 0:
                parts.append(cur)
                cur = ""
            else:
                cur += ch
        if cur:
            parts.append(cur)
        return parts

    def walk(prefix, s):
        if not s:
            return []
        brace = s.find("{")
        if brace == -1:
            segs = [x for x in s.split("::") if x]
            segs = [re.sub(r"as[A-Za-z_]\w*$", "", x) if "as" in x and x not in ("as",) else x
                    for x in segs]
            if segs and segs[-1] == "self":
                segs = segs[:-1]
            if segs and segs[-1] == "*":
                segs = segs[:-1]
            return [prefix + segs]
        head = [x for x in s[:brace].split("::") if x]
        inner = s[brace + 1:s.rfind("}")]
        out = []
        for part in split_top(inner):
            out.extend(walk(prefix + head, part))
        return out

    return [p for p in walk([], tree) if p]


def extract_rust(text):
    """`{uses: [[seg...]], mods: [name], tests: [fn]}` of one Rust source."""
    code = re.sub(r"//[^\n]*", "", text)
    uses = []
    for m in _USE_RE.finditer(code):
        uses.extend(_expand_use(m.group(1).replace(" as ", " as")))
    return {"uses": uses, "mods": _MOD_RE.findall(code), "tests": _TEST_FN_RE.findall(code)}


def extract_manifest(text):
    """`{name, lib, deps: [[name, kind]]}` from a crate's Cargo.toml."""
    try:
        doc = tomllib.loads(text)
    except tomllib.TOMLDecodeError:
        return {"name": None, "deps": []}
    deps = []
    for table, kind in (("dependencies", "normal"), ("dev-dependencies", "dev"),
                        ("build-dependencies", "build")):
        for name, spec in sorted((doc.get(table) or {}).items()):
            real = spec.get("package", name) if isinstance(spec, dict) else name
            deps.append([real, kind])
    return {"name": (doc.get("package") or {}).get("name"), "deps": deps}


def extract_case(text):
    try:
        doc = json.loads(text)
    except ValueError:
        return {}
    if not isinstance(doc, dict):
        return {}
    tpl = doc.get("templates")
    return {"id": str(doc.get("id") or ""),
            "scaffold": doc.get("scaffold") if isinstance(doc.get("scaffold"), str) else None,
            "templates": [t for t in tpl if isinstance(t, str)] if isinstance(tpl, list) else []}


EXTRACTORS = {"rust": extract_rust, "manifest": extract_manifest, "case": extract_case}


def _source_kind(rel):
    if rel.startswith("crates/") and rel.endswith(".rs"):
        return "rust"
    if rel.startswith("crates/") and rel.endswith("/Cargo.toml") and rel.count("/") == 2:
        return "manifest"
    if (rel.startswith(MEMORY_CASES + "/") or rel.startswith(BUILDER_CASES + "/")) \
            and rel.endswith(".json"):
        return "case"
    return None


class Extractor:
    """Per-source extraction with a mark-keyed cache (the incremental rebuild)."""

    def __init__(self, cache=None):
        self.cache = cache if cache is not None else {}
        self.extracted = 0

    def get(self, kind, mark, data):
        key = "%d:%s:%s" % (EXTRACTOR_VERSION, kind, mark)
        hit = self.cache.get(key)
        if hit is not None:
            return hit
        self.extracted += 1
        out = EXTRACTORS[kind](data.decode("utf-8", "replace"))
        self.cache[key] = out
        return out


# --- metadata ---------------------------------------------------------------

def metadata_from_cargo(meta):
    """`cargo metadata --format-version 1` JSON -> `{crate: {"deps": [[name, kind]], "dir"}}`."""
    members = set(meta.get("workspace_members") or [])
    out = {}
    for pkg in meta.get("packages", []):
        if members and pkg.get("id") not in members:
            continue
        deps = [[d["name"], d.get("kind") or "normal"] for d in pkg.get("dependencies", [])]
        out[pkg["name"]] = {"deps": deps,
                            "dir": os.path.dirname(pkg.get("manifest_path", ""))}
    return out


# --- the build --------------------------------------------------------------

class Graph:
    def __init__(self):
        self.nodes = {}
        self.edges = {}
        self.sources = []

    def node(self, addr, kind, name=None, source=None):
        if addr not in self.nodes:
            self.nodes[addr] = {"addr": addr, "kind": kind, "name": name or addr.split(":", 1)[1],
                                "source": source}
        return addr

    def edge(self, src, dst, kind, cls="extracted", state="resolved", data=None):
        assert kind in EDGE_KINDS, kind
        key = (src, dst, kind)
        e = {"from": src, "to": dst, "kind": kind, "class": cls, "state": state}
        if data:
            e["data"] = data
        self.edges[key] = e

    def to_json(self, rev, built_at):
        return {"schema": SCHEMA, "built_at": built_at, "rev": rev, "code": code_hash(),
                "sources": sorted(self.sources, key=lambda s: s["addr"]),
                "nodes": sorted(self.nodes.values(), key=lambda n: n["addr"]),
                "edges": sorted(self.edges.values(),
                                key=lambda e: (e["from"], e["to"], e["kind"]))}


def _module_map(files, crate):
    """`{modpath tuple: file}` of `crates/<crate>/src/**.rs` (lib/main root = ())."""
    pre = "crates/%s/src/" % crate
    out = {}
    for rel in files:
        if not rel.startswith(pre) or not rel.endswith(".rs"):
            continue
        parts = rel[len(pre):-3].split("/")
        if parts[0] == "bin":
            continue
        if parts == ["lib"] or (parts == ["main"] and () not in out):
            out[()] = rel
            continue
        if parts[-1] == "mod":
            parts = parts[:-1]
        out[tuple(parts)] = rel
    return out


def _modpath(rel, crate):
    pre = "crates/%s/src/" % crate
    if not rel.startswith(pre):
        return None
    parts = rel[len(pre):-3].split("/")
    if parts in (["lib"], ["main"]):
        return ()
    if parts[-1] == "mod":
        parts = parts[:-1]
    return tuple(parts)


def _resolve(modmap, segs):
    for n in range(len(segs), -1, -1):
        hit = modmap.get(tuple(segs[:n]))
        if hit:
            return hit
    return None


def _mod_child(files_set, rel, name):
    d, base = os.path.split(rel)
    stem = base[:-3]
    roots = (d,) if stem in ("mod", "lib", "main") else (os.path.join(d, stem), d)
    for r in roots:
        for cand in ("%s/%s.rs" % (r, name), "%s/%s/mod.rs" % (r, name)):
            if cand in files_set:
                return cand
    return None


def _literal_index(files):
    """`{literal: [file]}` for every path outside crates/ (rule 8 spellings)."""
    idx = collections.defaultdict(list)
    for rel in files:
        if rel.startswith("crates/"):
            continue
        for lit in gate_plan._path_literals(rel):
            idx[lit].append(rel)
    return idx


_RUN_RE = re.compile(r"[A-Za-z0-9_./\-@+]+")


def _head_re(heads):
    alt = "|".join(sorted((re.escape(h) for h in heads), key=len, reverse=True))
    return re.compile("(?=(%s))" % alt) if alt else None


def _literals_in(text, idx, head_re, odd):
    """The literals of `idx` that occur in `text` -- the same hits as `lit in text`.

    A literal starts with its first path segment (a "head"); every position a
    head occurs at (overlapping) is a candidate start, and the candidate ends
    are the prefixes of the path-character run from there.
    """
    found = set()
    if head_re is not None:
        for m in head_re.finditer(text):
            i = m.start()
            run = _RUN_RE.match(text, i)
            if not run:
                continue
            end = run.end()
            for j in range(i + len(m.group(1)), end + 1):
                lit = text[i:j]
                if lit in idx:
                    found.add(lit)
    for lit in odd:
        if lit in text:
            found.add(lit)
    return found


def _template_refs(text, names, ref_res, rel_res, memo, key):
    """`{name}` of templates `text` references -- `gate_plan._tests_referencing_template`
    per test source, with the catalogue-wide verdict computed once per source."""
    hit = memo.get(key)
    if hit is not None:
        return set(hit)
    wide = gate_plan._is_catalogue_wide(text)
    if wide:
        out = set(names)
    else:
        shipped = gate_plan._names_the_shipped_root(text)
        out = {n for n in names
               if ref_res[n].search(text) or (shipped and rel_res[n].search(text))}
    memo[key] = sorted(out)
    return out


def build(root, metadata=None, coverage=None, cache=None, stations=True, files=None):
    """Build the graph of `root`. Returns `(Graph, stats)`; `cache` is updated in place."""
    t0 = time.monotonic()
    root = str(root)
    files = tree_files(root) if files is None else sorted(files)
    files_set = set(files)
    g = Graph()
    ex = Extractor(cache)

    # sources with marks; per-source extraction
    rust, manifests, cases = {}, {}, {}
    for rel in files:
        kind = _source_kind(rel)
        if kind is None:
            continue
        data = _read(root, rel)
        mark = blob_sha(data)
        g.sources.append({"addr": "file:%s" % rel, "path": rel, "kind": kind, "mark": mark})
        result = ex.get(kind, mark, data)
        {"rust": rust, "manifest": manifests, "case": cases}[kind][rel] = result

    # crates and `depends`
    crate_dirs = {}
    for rel, man in manifests.items():
        name = man.get("name") or rel.split("/")[1]
        crate_dirs[name] = rel.split("/")[1]
    if metadata is not None:
        deps_of = {n: v["deps"] for n, v in metadata_from_cargo(metadata).items()}
    else:
        deps_of = {}
        for rel, man in manifests.items():
            deps_of[man.get("name") or rel.split("/")[1]] = man.get("deps", [])
    workspace = set(deps_of) | set(crate_dirs)
    for name in sorted(workspace):
        g.node("crate:%s" % name, "crate", name, "crates/%s/Cargo.toml" % crate_dirs.get(name, name))
    for name, deps in sorted(deps_of.items()):
        for dep, kind in deps:
            if dep in workspace:
                g.edge("crate:%s" % name, "crate:%s" % dep, "depends",
                       data={"dev": True} if kind == "dev" else (
                           {"build": True} if kind == "build" else None))
    by_dir = {d: n for n, d in crate_dirs.items()}

    # files of crates, binaries, tests
    lib_tests = collections.defaultdict(bool)
    for rel in files:
        crate_dir = gate_plan._crate_of(rel)
        if not crate_dir or crate_dir not in by_dir:
            continue
        crate = by_dir[crate_dir]
        rest = rel.split("/", 2)[2]
        if rest.startswith(("src/", "tests/")) or rest == "build.rs":
            g.node("file:%s" % rel, "file", rel, rel)
            g.edge("crate:%s" % crate, "file:%s" % rel, "contains")
        info = rust.get(rel)
        if not info:
            continue
        parts = rest.split("/")
        if parts[0] == "tests" and (len(parts) == 2 or (len(parts) == 3 and parts[2] == "main.rs")):
            stem = parts[1][:-3] if len(parts) == 2 else parts[1]
            if len(parts) == 3 or not os.path.isdir(os.path.join(root, "crates", crate_dir,
                                                                    "tests", stem)) \
                    or len(parts) == 2:
                bid = "%s::%s" % (crate, stem)
                g.node(bin_addr(bid), "bin", bid, rel)
                g.edge("file:%s" % rel, bin_addr(bid), "contains")
                for fn in info["tests"]:
                    taddr = "test:%s::%s" % (bid, fn)
                    g.node(taddr, "test", "%s::%s" % (bid, fn), rel)
                    g.edge(bin_addr(bid), taddr, "contains")
        elif parts[0] == "src" and info["tests"]:
            lib_tests[crate] = True
            mp = _modpath(rel, crate_dir) or ()
            for fn in info["tests"]:
                taddr = "test:%s::%s" % (crate, "::".join(list(mp) + [fn]))
                g.node(taddr, "test", taddr[5:], rel)
                g.edge(bin_addr(crate), taddr, "contains")
    for crate, has in sorted(lib_tests.items()):
        lib = "crates/%s/src/lib.rs" % crate_dirs[crate]
        if has and lib in files_set:
            g.node(bin_addr(crate), "bin", crate, lib)
            g.edge("file:%s" % lib, bin_addr(crate), "contains")
        elif has:
            g.nodes.pop(bin_addr(crate), None)
            for key in [k for k in g.edges if k[0] == bin_addr(crate)]:
                g.edges.pop(key)

    # `use` and `mod`
    modmaps = {d: _module_map(files, d) for d in by_dir}
    for rel, info in sorted(rust.items()):
        crate_dir = gate_plan._crate_of(rel)
        if crate_dir not in by_dir:
            continue
        src = "file:%s" % rel
        g.node(src, "file", rel, rel)
        for name in info["mods"]:
            child = _mod_child(files_set, rel, name)
            if child:
                g.edge(src, "file:%s" % child, "contains")
        here = _modpath(rel, crate_dir)
        for segs in info["uses"]:
            head = segs[0]
            target, state = None, "resolved"
            if head == "crate":
                target = _resolve(modmaps[crate_dir], segs[1:])
            elif head in ("self", "super") and here is not None:
                base = list(here)
                rest = segs
                while rest and rest[0] in ("self", "super"):
                    if rest[0] == "super" and base:
                        base.pop()
                    rest = rest[1:]
                target = _resolve(modmaps[crate_dir], base + rest)
            elif head.replace("_", "-") in by_dir or head.replace("_", "-") in workspace:
                dep = head.replace("_", "-")
                target = _resolve(modmaps.get(crate_dirs.get(dep, dep), {}), segs[1:])
            elif head not in _STD and head[:1].islower():
                ext = "crate:%s" % head
                g.node(ext, "crate", head, None)
                g.edge(src, ext, "use", state="external")
                continue
            else:
                continue
            if target:
                if target != rel:
                    g.edge(src, "file:%s" % target, "use", state=state)
            else:
                g.edge(src, "file:%s" % "::".join(segs), "use", state="unresolved")

    # rule 8 inverted (`reads`), rule 4/5 (`references`)
    sources_of_tests = gate_plan.test_sources(root)
    idx = _literal_index(files)
    heads = {lit.split("/", 1)[0] for lit in idx if _RUN_RE.fullmatch(lit)}
    odd = [lit for lit in idx if not _RUN_RE.fullmatch(lit)]
    head_re = _head_re(heads)
    templates = sorted({p.split("/")[1] for p in files
                        if p.startswith("templates/") and p.count("/") >= 2})
    examples = sorted({p.split("/")[1] for p in files
                       if p.startswith("examples/") and p.count("/") >= 2})
    ref_res = {n: gate_plan._template_reference_re(n) for n in templates}
    rel_res = {n: gate_plan._root_relative_reference_re(n) for n in templates}
    names_key = blob_sha(json.dumps(templates).encode())[:12]
    memo = ex.cache.setdefault("template-refs", {})
    for name in templates:
        g.node("template:%s" % name, "template", name, "templates/%s" % name)
    for name in examples:
        g.node("example:%s" % name, "example", name, "examples/%s" % name)
    for crate_dir, stem, text in sources_of_tests:
        crate = by_dir.get(crate_dir, crate_dir)
        bid = bin_addr("%s::%s" % (crate, stem))
        raw = _literals_in(text, idx, head_re, odd)
        # a path a prose comment names reads nothing (#1160) -- unless the unit
        # walks an ancestor directory of it (the resolver's doubt, B-1)
        hits = _literals_in(gate_plan.reader_text(text), idx, head_re, odd) if raw else set()
        for lit in sorted(raw):
            for rel in idx[lit]:
                if lit in hits or gate_plan.walks_ancestor(root, crate_dir, stem, rel):
                    g.node("file:%s" % rel, "file", rel, rel)
                    g.edge(bid, "file:%s" % rel, "reads")
        key = "%s:%s" % (blob_sha(text.encode("utf-8", "surrogateescape")), names_key)
        tpls = sorted(_template_refs(text, templates, ref_res, rel_res, memo, key))
        exs = [name for name in examples if "examples/%s" % name in text]
        # rules 4/5 plan the rule-3 set of the reader unit (a folded helper
        # -> its harness, a shared file -> every binary compiling it in): the
        # edges hang on that set, as on the floor (tb-s1c m-2, #1177)
        users = sorted(_unit_user_addrs(root, crate_dir, stem, crate)) if tpls or exs else ()
        for src in users:
            for name in tpls:
                g.edge(src, "template:%s" % name, "references")
            for name in exs:
                g.edge(src, "example:%s" % name, "references")
    # rule 8 runtime (GH #1138): what a test reads without naming it. A walked
    # directory is a `dir:<d>` node (one edge per test, not per file below
    # it); a generator of SCRIPT_READS `reads` its sources (file -> file), and
    # the tests that name the generator read them through it. `impact`
    # resolves both with the resolver's own filter (`runtime_dir_applies`).
    # `form`: a walk written repo-relative (`crates`, `crates/<c>/src`) reads
    # every crate file below, `.rs` included; a crate-relative one only the
    # non-`.rs` files of tests/ -- `impact` filters on it (tb-s1c, #1177). A
    # crate file a test names whole repo-relative is a `file:` node as well.
    index = gate_plan.runtime_index(root)
    dirs = {d for rel in files for d in gate_plan.runtime_ancestors(rel)}
    fixtures = {rel for rel in files if rel.startswith("crates/") and (
        (rel.split("/")[2:3] == ["tests"] and not rel.endswith(".rs")) or rel in index)}
    for (crate_dir, unit), ds in sorted(gate_plan.runtime_roots(root, dirs | fixtures).items()):
        if gate_plan.is_helper(root, crate_dir, unit):
            # a helper without main.rs (tb-s1c-c F4) has no binary: a `bin:`
            # node for it is a binary_id nextest refuses; the floor (rule 8,
            # `_helper_users`) plans its includers
            continue
        src = bin_addr("%s::%s" % (by_dir.get(crate_dir, crate_dir), unit))
        for d in sorted(ds):
            kind = "file" if d in fixtures else "dir"
            form = "repo" if (crate_dir, unit) in index.get(d, ()) else "crate"
            g.node("%s:%s" % (kind, d), kind, d, d)
            g.edge(src, "%s:%s" % (kind, d), "reads", data={"runtime": "dir", "form": form})
    for script, globs in gate_plan.SCRIPT_READS:
        if script not in files:
            continue
        g.node("file:%s" % script, "file", script, script)
        for rel in files:
            if any(gate_plan._matches(rel, pat) for pat in globs):
                g.node("file:%s" % rel, "file", rel, rel)
                g.edge("file:%s" % script, "file:%s" % rel, "reads", data={"runtime": "script"})
    for rel in files:
        parts = rel.split("/")
        if parts[0] == "templates" and len(parts) >= 3:
            g.node("file:%s" % rel, "file", rel, rel)
            g.edge("template:%s" % parts[1], "file:%s" % rel, "contains")
        elif parts[0] == "examples" and len(parts) >= 3:
            g.node("file:%s" % rel, "file", rel, rel)
            g.edge("example:%s" % parts[1], "file:%s" % rel, "contains")

    # pins (scaffolds, examples, sets) -> template@version
    versions = locks.template_versions(root)
    pin_files = collections.defaultdict(set)
    if versions:
        rx = locks._pin_re(versions)
        rels = [r for top in PIN_SURFACES for r in locks._walk_json(root, top)]
        rels += locks._sets(root)
        for rel in rels:
            if rel not in files_set:
                continue
            for line in locks._lines(root, rel):
                if "@" not in line:
                    continue
                for m in rx.finditer(line):
                    name, ver = m.group("name"), m.group("ver")
                    paddr = g.node("pin:%s#%s" % (rel, name), "pin", "%s#%s" % (rel, name), rel)
                    g.node("file:%s" % rel, "file", rel, rel)
                    g.edge("file:%s" % rel, paddr, "contains")
                    tpl = "template:%s" % name
                    g.node(tpl, "template", name, "templates/%s" % name)
                    g.edge(paddr, tpl, "pins", state="resolved" if versions[name] == ver
                           else "broken", data={"version": ver})
                    pin_files[rel].add(name)

    # scenario cases
    for rel, case in sorted(cases.items()):
        if not case:
            continue
        suite = "memory" if rel.startswith(MEMORY_CASES) else "builder"
        cid = case.get("id") or os.path.basename(rel)[:-5]
        caddr = g.node("case:%s/%s" % (suite, cid), "case", "%s/%s" % (suite, cid), rel)
        g.node("file:%s" % rel, "file", rel, rel)
        g.edge("file:%s" % rel, caddr, "contains")
        st = g.node("station:scenarios:%s" % suite, "station", "scenarios:%s" % suite)
        g.edge(caddr, st, "needs")
        if suite == "memory":
            g.edge(caddr, "template:memory-hive", "references", cls="inferred")
        for name in case.get("templates", []):
            g.edge(caddr, "template:%s" % name, "references")
        if case.get("scaffold"):
            pre = "%s/%s/" % (BUILDER_SCAFFOLDS, case["scaffold"])
            for prel, names in sorted(pin_files.items()):
                if prel.startswith(pre):
                    for name in sorted(names):
                        g.edge(caddr, "template:%s" % name, "references",
                               data={"via": "scaffold:%s" % case["scaffold"]})

    # ADR anchors: doc pinned_by test
    test_index = collections.defaultdict(list)
    for addr, n in g.nodes.items():
        if n["kind"] == "test":
            test_index[addr.rsplit("::", 1)[-1]].append(addr)
    reg = _read(root, ADR_REGISTRY).decode("utf-8", "replace")
    for line in reg.splitlines():
        if line.startswith("#") or not line.strip():
            continue
        cols = line.split("\t")
        if len(cols) < 4 or cols[0] == "adr" or not cols[3].startswith("test:"):
            continue
        doc = g.node("doc:plans/adr/%s" % cols[0], "doc", "plans/adr/%s" % cols[0], ADR_REGISTRY)
        fn = cols[3][5:].strip()
        hits = test_index.get(fn, [])
        if not hits:
            g.edge(doc, "test:%s" % fn, "pinned_by", state="unresolved")
        for t in hits:
            g.edge(doc, t, "pinned_by")

    # stations triggered by classes (probed through the resolver itself)
    if stations:
        for cls, sample, mode, names in station_triggers(root, files):
            caddr = g.node("class:%s" % cls, "class", cls)
            for name in names:
                st = g.node("station:%s" % name, "station", name)
                key = (st, caddr, "triggered_by")
                prev = g.edges.get(key, {}).get("data", {"modes": [], "sample": sample})
                modes = sorted(set(prev["modes"]) | {mode})
                g.edge(st, caddr, "triggered_by", cls="inferred",
                       data={"modes": modes, "sample": prev["sample"]})

    # coverage
    if coverage:
        add_coverage(g, coverage)

    secs = round(time.monotonic() - t0, 3)
    stats = {"nodes": len(g.nodes), "edges": len(g.edges), "secs": secs,
             "extracted": ex.extracted, "sources": len(g.sources)}
    return g, stats


def station_triggers(root, files):
    """`[(class, sample path, mode, [station...])]` -- one probe per class and mode.

    The resolver has no class table to read; asking it with one sample path of
    each class is the only way that cannot drift from what it plans.
    """
    samples = {}
    for rel in files:
        for cls in sorted(gate_plan._classes_of_path(rel)):
            samples.setdefault(cls, rel)
    always = {}
    out = []
    for mode in ("strand", "integration"):
        try:
            base = {st.name for st in gate_plan.plan(["__graph_probe_none__"], mode, root)}
        except gate_plan.GatePlanError:
            base = set()
        always[mode] = base
        for cls, rel in sorted(samples.items()):
            try:
                names = {st.name for st in gate_plan.plan([rel], mode, root)}
            except gate_plan.GatePlanError:
                continue
            out.append((cls, rel, mode, sorted(names - base)))
    return out


def add_coverage(g, coverage):
    """`covers` edges from a coverage map (`coverage.py`, `coverage-<rev>.json`).

    The source of an edge is the test PART: `test:<binary>::<module>` for a
    module part, `bin:<binary>` for a whole binary (V0 section 2 `cache`).
    """
    since = coverage.get("built_at")
    for part in coverage.get("parts", []):
        binary, module = part["binary"], part.get("module")
        if module:
            src = g.node("test:%s::%s" % (binary, module), "test", "%s::%s" % (binary, module))
            g.node(bin_addr(binary), "bin", binary)
            g.edge(bin_addr(binary), src, "contains")
        else:
            src = g.node(bin_addr(binary), "bin", binary)
        for rel, lines in sorted(part.get("files", {}).items()):
            if lines <= 0:
                continue
            g.node("file:%s" % rel, "file", rel, rel)
            g.edge(src, "file:%s" % rel, "covers", data={"lines": lines, "since": since})


# --- persistence ------------------------------------------------------------

def load_cache(directory):
    try:
        with open(os.path.join(directory, CACHE_FILE), encoding="utf-8") as fh:
            return json.load(fh)
    except (OSError, ValueError):
        return {}


def _write_atomic(path, text):
    """Write `text` to `path` through a tmp file of its own in the same directory (two
    writers in the shared graph dir never share a tmp name), then `os.replace`."""
    fd, tmp = tempfile.mkstemp(prefix=path.name + ".", suffix=".tmp", dir=path.parent)
    try:
        os.fchmod(fd, 0o644)  # mkstemp creates 0600; readers expect the old mode
        with os.fdopen(fd, "w", encoding="utf-8") as fh:
            fh.write(text)
        os.replace(tmp, path)
    except BaseException:
        with contextlib.suppress(OSError):
            os.unlink(tmp)
        raise


# The graphs of the newest revs the directory keeps; every older `<rev>.json`
# and `<rev>.lock` goes when a graph is saved. WHY 8: since 3a every gated clean
# rev stores its own graph (7.5 MB, measured 14.10.; 33 files, 209 MB on the
# machine before any pruning); a wave gates up to six strand heads plus master at
# once, and each must find its graph again between `--plan-only` and the gate.
# Eight revs cover that with one to spare and cap the directory near 60 MB.
GRAPH_KEEP_REVS = 8
_REV_FILE = re.compile(r"^(?P<rev>[0-9a-f]{7,64})\.(?:json|lock)$")


def save(g, rev, directory, cache=None, built_at=None):
    directory = pathlib.Path(directory)
    directory.mkdir(parents=True, exist_ok=True)
    built_at = built_at or time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    doc = g.to_json(rev, built_at)
    path = directory / ("%s.json" % rev)
    try:
        with open(directory / "latest.json", encoding="utf-8") as fh:
            before = json.load(fh).get("rev")
    except (OSError, ValueError, AttributeError):
        before = None
    _write_atomic(path, json.dumps(doc, sort_keys=True))
    _write_atomic(directory / "latest.json", json.dumps({"rev": rev, "path": str(path)}))
    if cache is not None:
        _write_atomic(directory / CACHE_FILE, json.dumps(cache, sort_keys=True))
    _prune_graph_dir(directory, keep={rev, before})
    return path


def _prune_graph_dir(directory, keep=(), revs=GRAPH_KEEP_REVS):
    """Remove the `<rev>.json`/`<rev>.lock` of every rev but the `revs` newest
    (by the newest mtime of its files) and the `keep` ones: the rev just saved
    and the one `latest.json` named before -- a reader may hold it right now. A
    lock another process holds (a build of that rev under way) stays, and so
    does its graph. The removed paths."""
    import fcntl  # noqa: PLC0415
    by_rev = collections.defaultdict(list)
    try:
        names = os.listdir(directory)
    except OSError:
        return []
    for name in names:
        m = _REV_FILE.match(name)
        if m:
            by_rev[m.group("rev")].append(pathlib.Path(directory, name))

    def newest(rev):
        stamps = []
        for path in by_rev[rev]:
            with contextlib.suppress(OSError):
                stamps.append(path.stat().st_mtime_ns)
        return max(stamps, default=0)
    order = sorted(by_rev, key=newest, reverse=True)
    removed = []
    for rev in order[revs:]:
        if rev in keep:
            continue
        lock = pathlib.Path(directory, "%s.lock" % rev)
        held = None
        try:
            if lock.exists():
                held = open(lock, "a", encoding="utf-8")  # noqa: SIM115 -- closed below
                try:
                    fcntl.flock(held, fcntl.LOCK_EX | fcntl.LOCK_NB)
                except OSError:
                    continue        # a build of this rev is under way
            for path in by_rev[rev]:
                with contextlib.suppress(FileNotFoundError):
                    path.unlink()
                    removed.append(path)
        except OSError:
            continue
        finally:
            if held is not None:
                held.close()
    return removed


def load(path=None):
    """The graph at `path`, or the latest build. A `latest.json` whose rev is
    not the rev of the file it points to is refused (ValueError, review I3)."""
    want = None
    if path is None:
        latest = graph_dir() / "latest.json"
        with open(latest, encoding="utf-8") as fh:
            pointer = json.load(fh)
        path, want = pointer["path"], pointer.get("rev")
    with open(path, encoding="utf-8") as fh:
        doc = json.load(fh)
    if doc.get("schema") != SCHEMA:
        raise ValueError("not a %s file: %s" % (SCHEMA, path))
    if want is not None and doc.get("rev") != want:
        raise ValueError("latest.json names rev %s, %s holds rev %s" % (want, path, doc.get("rev")))
    return doc


# --- the graph of the head, built when it is missing (welle-validierung-2 3a) --
#
# Only `pipeline graph build` (by hand) built the graph; part 1 of the wave
# narrowed on a graph 807 commits behind the head, and `impact` only noted
# "graph rev X is not HEAD Y". Every narrowing goes through `narrow.apply`, and
# that asks `fresh` first: a graph of another rev or of other resolver code is
# rebuilt there and then (measured on the tip 14.10.: 45 s cold, 27 s with the
# extraction cache, so a full build and no incremental path). A build that
# fails raises `GraphBuildError` -- the plan is refused (`gate: no plan`), never
# narrowed on the old graph. Two gates of one rev build once: the second waits
# for the lock on `<rev>.lock` and reads the `<rev>.json` the first saved.
#
# One rule decides what a tree's graph is (`graph_dirt`, re-check v2-kit c23
# I-1): a tree whose tracked files are those of its head and whose untracked
# files the graph does not read IS its head rev for the graph -- built from the
# tracked files under the lock, stored, read again. An untracked report, receipt
# or gate archive under plans/ is such a file; a strand worktree always carries
# one, and counting it made every plan of the strand rebuild unstored. A tree
# with a tracked change or an untracked file the graph reads (a new
# `tests/*.rs`, a Cargo.toml) is no rev: built from the working tree, stored
# nowhere, and never answered with the stored graph of its head (which lacks
# exactly that file). `MECLAW_PIPELINE_GRAPH_AUTO=0` keeps the old behaviour
# (the latest build as it is) -- the test isolation sets it.

AUTO_ENV = "MECLAW_PIPELINE_GRAPH_AUTO"
# Untracked paths below these the graph may leave out: the wave records
# (reports, receipts, gate archives, re-check scripts). Any other untracked
# path -- a crate file, a template, a doc page, a root manifest -- is dirt:
# the graph's sources, containment and station probes read those trees.
GRAPH_BLIND_ROOTS = ("plans/",)
# Untracked paths below `GRAPH_BLIND_ROOTS` that the resolver gives classes
# beyond `docs` (corridor fixtures, export fixtures) are read all the same.
_BLIND_CLASSES = frozenset({"docs"})
_PLANS_RUN = re.compile(r"[A-Za-z0-9_.@+\-/]*plans/[A-Za-z0-9_.@+\-/]*")
_PATH_CHARS = re.compile(r"[A-Za-z0-9_.@+\-/]+")


class GraphBuildError(RuntimeError):
    """The graph of the head could not be built -- no narrowing on an old one."""


def auto_enabled(env=None):
    return (os.environ if env is None else env).get(AUTO_ENV, "1").strip() != "0"


def _status_entries(root):
    """`(tracked, untracked)` paths of `git status`; None when git cannot say."""
    try:
        out = subprocess.run(["git", "-C", str(root), "status", "--porcelain", "-z",
                              "--untracked-files=all"],
                             capture_output=True, check=True,
                             env={**os.environ, "GIT_OPTIONAL_LOCKS": "0"})
    except (OSError, subprocess.CalledProcessError):
        return None
    tracked, untracked = [], []
    parts = out.stdout.decode("utf-8", "surrogateescape").split("\0")
    i = 0
    while i < len(parts):
        entry = parts[i]
        i += 1
        if len(entry) < 4:
            continue
        code, path = entry[:2], entry[3:]
        if code == "??":
            untracked.append(path)
            continue
        tracked.append(path)
        if "R" in code or "C" in code:
            i += 1      # -z: the source path of a rename or copy follows
    return tracked, untracked


def _named_by_a_test(root, paths):
    """The `paths` (below plans/) a test source names or walks: a literal of
    the path, of a directory below it, or of a directory above it the
    resolver indexes as walked (`_runtime_index`)."""
    texts = [text for _crate, _unit, text in gate_plan._reader_sources(root)]
    runs = {run for text in texts for run in _PLANS_RUN.findall(text)}
    walked = [k.rstrip("/") for k in gate_plan._runtime_index(root) if k.startswith("plans/")]
    out = []
    for rel in paths:
        # a path of path characters only occurs inside a run of them; any
        # other spelling (a blank, an umlaut) is looked for in the texts whole
        named = any(rel in run for run in runs) if _PATH_CHARS.fullmatch(rel) \
            else any(rel in text for text in texts)
        if named or any(rel.startswith(w + "/") for w in walked):
            out.append(rel)
    return out


def graph_dirt(root):
    """The paths that keep `root` from being its head rev for the graph: every
    tracked change, and every untracked file the graph reads (the rule above).
    `[]` -- build the rev from the tracked files, under the lock, and store it."""
    status = _status_entries(root)
    if status is None:
        return ["<git status failed>"]
    tracked, untracked = status
    dirt = list(tracked)
    blind = []
    for rel in untracked:
        if rel.startswith(GRAPH_BLIND_ROOTS) \
                and gate_plan._classes_of_path(rel) <= _BLIND_CLASSES:
            blind.append(rel)
        else:
            dirt.append(rel)
    if blind:
        dirt += _named_by_a_test(root, blind)
    return sorted(dirt)


def _current(doc, rev):
    return doc is not None and doc.get("rev") == rev and doc.get("code") == code_hash()


def _journal_build(data):
    """Journal `graph_built`; the error text when the store refused, else None
    (the build stands -- the plan names the hole instead of refusing)."""
    from pipeline import events, store  # noqa: PLC0415
    try:
        db = store.open()
        try:
            with store.tx(db):
                events.emit(db, "graph_built", "note", data)
        finally:
            store.close(db)
    except Exception as exc:  # noqa: BLE001 -- named in the plan's note
        return "%s: %s" % (type(exc).__name__, exc)
    return None


def fresh(repo, env=None):
    """`(doc, built)`: the graph of `repo`'s head and, when this call built it,
    `{rev, secs, stored, reason: "auto"}` (else None). Raises GraphBuildError."""
    try:
        latest = load()
    except (OSError, ValueError, KeyError):
        latest = None
    if not auto_enabled(env):
        if latest is None:
            raise OSError("no graph build (%s=0)" % AUTO_ENV)
        return latest, None
    rev = tree_rev(repo)
    if rev == "worktree":
        raise GraphBuildError("%s is no git tree -- no rev to build the graph of" % repo)
    directory = graph_dir()
    dirt = graph_dirt(repo)
    try:
        directory.mkdir(parents=True, exist_ok=True)
        if dirt:   # no rev: the working tree, stored nowhere, never the head's graph
            return _auto_build(repo, rev, directory, store_it=False, dirt=dirt)
        if _current(latest, rev):
            return latest, None
        import fcntl  # noqa: PLC0415
        with open(directory / ("%s.lock" % rev), "w", encoding="utf-8") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX)
            try:
                done = load(str(directory / ("%s.json" % rev)))
            except (OSError, ValueError, KeyError):
                done = None
            if _current(done, rev):     # the other gate of this rev built it
                return done, None
            return _auto_build(repo, rev, directory, store_it=True)
    except GraphBuildError:
        raise
    except Exception as exc:  # noqa: BLE001 -- named, never a silent old graph
        raise GraphBuildError("auto build of the graph of %s failed: %s: %s"
                              % (rev[:12], type(exc).__name__, exc)) from exc


def _auto_build(repo, rev, directory, store_it, dirt=()):
    """Build the graph of `repo`: of its rev from the tracked files (`store_it`),
    else of the working tree as it is (`dirt` names why it is no rev)."""
    cache = load_cache(directory)
    g, st = build(repo, cache=cache,
                  files=tree_files(repo, untracked=False) if store_it else None)
    if store_it:
        save(g, rev, directory, cache)
    else:
        _write_atomic(directory / CACHE_FILE, json.dumps(cache, sort_keys=True))
    doc = g.to_json(rev, time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()))
    built = {"rev": rev, "secs": st["secs"], "stored": store_it, "reason": "auto"}
    if dirt:
        built["dirt"] = list(dirt[:3]) + (["+%d" % (len(dirt) - 3)] if len(dirt) > 3 else [])
    why = _journal_build({"rev": rev, "nodes": st["nodes"], "edges": st["edges"],
                          "secs": st["secs"], "reason": "auto", "stored": store_it})
    if why:
        built["journal"] = why
    return doc, built


# --- questions (graph-space `in_graph` names) -------------------------------

class Index:
    def __init__(self, doc):
        self.doc = doc
        self.nodes = {n["addr"]: n for n in doc["nodes"]}
        self.out = collections.defaultdict(list)
        self.inc = collections.defaultdict(list)
        for e in doc["edges"]:
            self.out[e["from"]].append(e)
            self.inc[e["to"]].append(e)


def dependents(ix, addr, depth=1, kinds=None):
    """`{addr: depth}` of everything that reaches `addr` over `depth` edges (reverse)."""
    seen = {addr: 0}
    frontier = [addr]
    for d in range(1, depth + 1):
        nxt = []
        for a in frontier:
            for e in ix.inc.get(a, ()):
                if kinds and e["kind"] not in kinds:
                    continue
                if e["from"] not in seen:
                    seen[e["from"]] = d
                    nxt.append(e["from"])
        frontier = nxt
    seen.pop(addr)
    return seen


def deps(ix, addr, kinds=None):
    return sorted({e["to"] for e in ix.out.get(addr, ()) if not kinds or e["kind"] in kinds})


def path(ix, a, b, limit=8):
    """Shortest path a -> b over edges in either direction: `[(addr, kind, dir)...]`."""
    prev = {a: None}
    frontier = [a]
    for _ in range(limit):
        nxt = []
        for x in frontier:
            for e, y, way in ([(e, e["to"], "->") for e in ix.out.get(x, ())]
                              + [(e, e["from"], "<-") for e in ix.inc.get(x, ())]):
                if y in prev:
                    continue
                prev[y] = (x, e["kind"], way)
                if y == b:
                    out = [(b, e["kind"], way)]
                    cur = x
                    while prev[cur] is not None:
                        out.append((cur, prev[cur][1], prev[cur][2]))
                        cur = prev[cur][0]
                    out.append((a, None, None))
                    return list(reversed(out))
                nxt.append(y)
        frontier = nxt
    return None


def stats(doc):
    nk = collections.Counter(n["kind"] for n in doc["nodes"])
    ek = collections.Counter(e["kind"] for e in doc["edges"])
    st = collections.Counter(e["state"] for e in doc["edges"])
    return {"rev": doc["rev"], "nodes": len(doc["nodes"]), "edges": len(doc["edges"]),
            "node_kinds": dict(sorted(nk.items())), "edge_kinds": dict(sorted(ek.items())),
            "edge_states": dict(sorted(st.items()))}


def broken(doc):
    """Pins on versions no template provides, and references to files that are not there."""
    nodes = {n["addr"] for n in doc["nodes"]}
    out = []
    for e in doc["edges"]:
        if e["state"] == "broken":
            out.append(e)
        elif e["kind"] in ("references", "pins") and e["to"] not in nodes:
            out.append(dict(e, state="broken"))
    return out


# --- CLI --------------------------------------------------------------------

def _cmd_graph(args):
    if args.op == "build":
        root = os.path.abspath(args.repo)
        directory = pathlib.Path(args.out) if args.out else graph_dir()
        cache = load_cache(directory)
        meta = None
        if args.metadata:
            with open(args.metadata, encoding="utf-8") as fh:
                meta = json.load(fh)
        cov = None
        if args.coverage:
            with open(args.coverage, encoding="utf-8") as fh:
                cov = json.load(fh)
        g, st = build(root, metadata=meta, coverage=cov, cache=cache,
                      stations=not args.no_stations)
        rev = tree_rev(root)
        out = save(g, rev, directory, cache)
        data = {"rev": rev, "nodes": st["nodes"], "edges": st["edges"], "secs": st["secs"]}
        if cov:
            data["coverage_rev"] = cov.get("rev")
        if not args.no_journal:
            from pipeline import events, store
            db = store.open(args.db)
            try:
                with store.tx(db):
                    events.emit(db, "graph_built", "note", data)
            finally:
                store.close(db)
        print(json.dumps(dict(data, path=str(out), extracted=st["extracted"])))
        return 0
    doc = load(args.graph)
    ix = Index(doc)
    if args.op == "dependents":
        res = dependents(ix, args.addr, args.depth)
        print(json.dumps(dict(sorted(res.items())), indent=1))
    elif args.op == "deps":
        print(json.dumps(deps(ix, args.addr), indent=1))
    elif args.op == "path":
        print(json.dumps(path(ix, args.addr, args.to)))
    elif args.op == "stats":
        print(json.dumps(stats(doc), indent=1))
    elif args.op == "broken":
        print(json.dumps(broken(doc), indent=1))
    return 0


def register(sub):
    """`pipeline graph build|dependents|deps|path|stats|broken` (V0 sections 7, 9)."""
    p = sub.add_parser("graph", help="build or ask the impact graph of the tree")
    p.add_argument("op", choices=("build", "dependents", "deps", "path", "stats", "broken"))
    p.add_argument("addr", nargs="?", help="node address `<kind>:<name>`")
    p.add_argument("to", nargs="?", help="second address (`path`)")
    p.add_argument("--depth", type=int, default=1, choices=(1, 2, 3))
    p.add_argument("--repo", default=os.path.dirname(_SCRIPTS))
    p.add_argument("--out", default=None, help="graph directory (default: private state dir)")
    p.add_argument("--graph", default=None, help="graph file (default: the latest build)")
    p.add_argument("--metadata", default=None, help="`cargo metadata` JSON to take crates from")
    p.add_argument("--coverage", default=None, help="coverage map to add `covers` edges from")
    p.add_argument("--no-stations", action="store_true", help="skip the station probe")
    p.add_argument("--no-journal", action="store_true", help="do not journal `graph_built`")
    p.add_argument("--db", default=None, help="store path (default $MECLAW_PIPELINE_DB)")
    p.set_defaults(func=_cmd_graph)
