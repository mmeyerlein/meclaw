"""Station cache of the pipeline (W3 plan item 5, OR-DT-10).

A cache key is the sha256 of the SOURCE closure of one station -- or of one
test binary of a nextest station -- never of a built binary: 471 test files
embed `env!("CARGO_MANIFEST_DIR")`, so the binaries of two trees are never
bit-identical, and a binary hash would never hit between strand, merge and
integration. The key covers:

* the station, the part, and its argv (for the parts of a nextest station
  only the nextest profile -- the filterset is the selection, not the run);
* the sorted (path, blob SHA) pairs of every declared input, read from the
  git index of the tree plus its overlay (dirty, untracked, deleted files);
* the tool versions of the lane fingerprint and its `build_env`: the WHOLE
  environment of the stations minus the closed exclusion list of
  `lane_fingerprint.sh` (BUILD_ENV_NOT: how fast, where, the caller's terminal
  and session -- each name with its reason), each value as a digest; the rustc
  among the tools is the one the stations build with (RUSTC, CARGO_BUILD_RUSTC,
  PATH -- cargo's order). Both from `lane_fingerprint.sh`, the one rule of that
  environment (re-check tv-bn M7r A-M2; orchestrator OR-T12-18 A-M1/A-M2: a name
  nobody listed is in the key, a miss, never a false hit). A fingerprint without
  an answer or without a tool's version keys nothing (M7r A-M1).

The declared inputs of a station are the classes and globs of `INPUTS`; an
`artefact:<bin>` input and every test binary expand to the source closure of
their workspace crates from `cargo metadata` (dev-dependencies included for
test binaries) plus what their sources name: `include_*!`/`#[path]` targets,
`mod` files, path literals (rule 8 of `gate_plan.py`, inverted), the
templates and examples they reference, and the closure of every workspace
binary they start at runtime (`CARGO_BIN_EXE_<bin>` or `target/<p>/<bin>`).

Lane acceptance (the real dep-info of a build against the declared closure):

    python3 -m scripts.pipeline.cache check-dep-info --target <CARGO_TARGET_DIR>
"""

import argparse
import bisect
import datetime
import fnmatch
import glob as globmod
import hashlib
import json
import os
import posixpath
import re
import subprocess
import sys

from . import store

try:  # `scripts.pipeline` (tests, the core) -> `scripts.gate_plan`
    from .. import gate_plan as gp
except (ImportError, ValueError):  # `pipeline` (the lane starter): a top-level package
    sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
    import gate_plan as gp  # noqa: E402


# 2 (re-check tv-bn M7t A-M1/H1): `build_env` is the inverted rule; entries of
# version 1 carried an `env` of the old list (or none) and answer nothing.
# 3 (orchestrator OR-T12-18 A-M1/A-M2): `build_env` is the whole environment minus
# the exclusion list, values as digests, and `rustc` is the building one; entries
# of version 2 answer nothing.
# 4 (orchestrator OR-T12-21): the verdict names of `env:` from `lane.verdict_env()`,
# values from `build_env` (digests); entries of version 3 answer nothing.
# 5 (re-check bundle N kit-fp M1/M2, N-K1/N-K2): `rustc` and `cargo` measured in
# the tree's toolchain, `cargo_config` (the cargo configuration outside the tree)
# in every key; entries of version 4 answer nothing.
# 6 (V2-A5, GH #1199): every cached station outside cargo keys the whole tree
# (`unkeyed_reads`); entries of version 5 keyed less than their checker read and
# answer nothing.
# 7 (V2-A5 for the browser stations, review v2-fix-b I2): `browser:display` and
# `browser:alone` key the import closure of their Node drivers (`mjs:`); entries
# of version 6 left `wait.mjs` and `lib/*.mjs` out and answer nothing.
KEY_VERSION = 7

# Never cached (W3 plan item 5): `export-audit` reads the receipt, `build`
# produces the artefacts the cached parts are derived from.
NEVER = {"precheck", "corpus-committed", "export-audit", "persona-receipt", "build",
         "build:browser"}

CACHEABLE_VERDICTS = ("GREEN",)   # never a FLAKY: a finding is no cache entry (OR-TV-2)
MAX_AGE = datetime.timedelta(days=7)
RELEASE_AGE = datetime.timedelta(hours=24)

# Fingerprint tools that enter every key; `tool:<x>` inputs add their own.
KEY_TOOLS = ("rustc", "cargo", "linker", "nextest", "python", "cargo_config")

# Declared inputs per station (W3 plan, "Cache-Eingaben je Station"). A spec is
#   <class>            a class or trigger label of `gate_plan._trigger_labels`
#                      (fnmatch: `template:*`)
#   <class>:<crate>    that class inside one crate (`rust_src:meclaw-cells`)
#   glob:<pattern>     `gate_plan` glob semantics (`**` crosses `/`)
#   artefact:<bin>     the source closure of a workspace binary
#   artefact:binary:<part>  the source closure of one test binary
#   compile:*          the compile closure of every workspace crate and target
#   mjs:<pattern>      the Node modules matching the glob plus every module they
#                      import by a relative specifier, transitively (a driver's
#                      `import ... from "./wait.mjs"`)
#   tree               the whole tree
#   tool:<name>        a fingerprint value, no path
#   env:<NAME>         a verdict variable the station's verdict reads (one of
#                      `lane_sync.sh VERDICT_ENV`): its value enters the key
#   env:part           (nextest stations) per part, the verdict variables the
#                      part's own code names -- read from that code
# The verdict variables (MECLAW_GATE_MODE, ...) enter ONLY the keys that declare
# them (orchestrator OR-T12-21): in the whole build environment a strand result
# never answered the integration pass of the same tree, a Cargo station's
# CACHED was lost for a variable its verdict never reads.
# Stations missing here are never cached (`slow-tests` reads the JUnit of the
# run, `corridor` and `deny-advisories` read beyond the tree). The argv paths
# of a station are inputs as well (see `_argv_units`). Under-declaration is the
# feared error: `uncovered_triggers` holds every table to the resolver, and
# `unkeyed_reads` every station outside cargo to the whole tree (V2-A5: what
# such a station reads shows only when it runs; the other specs of its row
# stay as the record of its triggers), and every station -- cargo too -- to the
# import closure of the Node modules its key holds (dep-info sees no Node).
INPUTS = {
    "roadmap-anchors": ["tree"],
    "adr-anchors": ["tree"],
    "claims": ["tree"],
    "tree-rules": ["tree"],
    "corpus": ["tree", "corpus_source"],
    "catalogue": ["tree", "template", "catalogue"],
    "shellcheck": ["shell", "gate_infra"],
    # Its unittests read the real tree (gate_plan against the crates, ...).
    "gate-selftest": ["tree"],
    "display-lab": ["tree", "display_lab", "glob:templates/display/**"],
    "llm-conformance": ["tree", "llm_conformance"],
    "guide-selftest": ["tree", "evals_guide", "persona_source"],
    # `cargo fmt --all` checks every target of every crate (examples too), like clippy.
    # `crate_file` (rule 10): a crate file outside src/ and tests/ -- a README
    # no target includes lies outside every compile closure.
    "fmt": ["rust_src", "rust_test", "crate_file", "workspace", "compile:*",
            "glob:rustfmt.toml", "glob:.rustfmt.toml"],
    "clippy": ["rust_src", "rust_test", "crate_file", "workspace", "compile:*",
               "glob:clippy.toml", "glob:.clippy.toml"],
    "unwrap-budget": ["rust_src", "rust_test", "crate_file", "workspace", "unwrap_infra"],
    # `env:part`: a test that reads the mode (710 the_copy_is_the_document asks the
    # living tree in integration) keys its part with it, every other part not.
    "tests": ["workspace", "glob:.config/nextest.toml", "tool:nextest", "env:part"],
    "scenario-tests": ["workspace", "glob:.config/nextest.toml", "tool:nextest", "env:part"],
    "doctests": ["rust_src", "crate_file", "workspace", "compile:*"],
    "deny": ["workspace", "glob:Cargo.lock", "glob:deny.toml", "glob:**/Cargo.toml"],
    "scenarios:memory": ["tree", "artefact:meclaw", "evals_memory",
                         "glob:templates/memory-hive/**",
                         "glob:workshop/fixtures/positive/memory-hive-probe/**"],
    "scenarios:builder": ["tree", "artefact:meclaw", "evals_builder", "glob:templates/**",
                          "glob:examples/meclaw-os/**", "glob:examples/organism/**"],
    "scenarios:display": ["tree", "display_scenarios", "glob:templates/display/**"],
    # The proofs serve `templates/web/**` and source `workshop/tools/wkenv.sh`
    # (OR-TB.S7.11, S1b re-check); the only station that books an engine. Its
    # drivers import `workshop/tools/wait.mjs` and `lib/*.mjs` (review v2-fix-b I2).
    "browser:display": ["artefact:meclaw", "display_browser", "glob:templates/display/**",
                        "glob:templates/web/**", "glob:workshop/tools/wkenv.sh",
                        "glob:workshop/tools/display-*", "glob:workshop/tools/display-*/**",
                        "mjs:workshop/tools/display-**", "tool:node", "tool:playwright"],
    # GH #1191: the locks `browser:display` hands to its station alone (plan.ALONE);
    # an in-process `web` cell, no `meclaw` binary.
    "browser:alone": ["display_browser", "glob:templates/display/**",
                      "glob:workshop/tools/display-*", "glob:workshop/tools/display-*/**",
                      "mjs:workshop/tools/display-**", "tool:node", "tool:playwright"],
    "recall-harness": ["tree", "artefact:meclaw", "evals_memory", "evals_recall", "recall_src",
                       "glob:templates/memory-hive/**"],
    "persona-cases": ["tree", "evals_persona", "persona_source"],
    # V2-A5 (GH #1199): the drift test reads the ci plan of scripts/gate_plan.py,
    # make_export.py and `git ls-files` of crates/ tests/ examples/ scripts/.
    "export-selftest": ["tree", "export_infra"],
    # The export index decides what is exported; only the whole tree is safe.
    "export-tree-tests": ["tree"],
}

_REASON_NO_PATH = frozenset({"always", "mode", "station", "floor"})
_SPEC_HEADS = frozenset({"glob", "file", "dir", "pydir", "crate", "testfix", "tool",
                         "artefact", "compile", "mjs"})

_INCLUDE_RE = re.compile(r'\binclude(?:_str|_bytes)?!\s*\(\s*"([^"\\]+)"')
_PATH_ATTR_RE = re.compile(r'#\[\s*path\s*=\s*"([^"\\]+)"\s*\]')
_MOD_RE = re.compile(r'^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+([A-Za-z_][A-Za-z0-9_]*)\s*;', re.M)
_RUN_RE = re.compile(r'[\w.\-/@+~]+')
_BIN_EXE_RE = re.compile(r'CARGO_BIN_EXE_([A-Za-z0-9_\-]+)')
_BIN_PATH_RE = re.compile(r'target/[\w\-]+/([A-Za-z0-9_\-]+)')
_PY_IMPORT_RE = re.compile(r'^\s*(?:from\s+([\w.]+)\s+import|import\s+([\w., ]+))', re.M)
# A relative module specifier of JavaScript: `import ... from "./x.mjs"` (also
# over several lines), `export ... from`, `import "./x.mjs"`, `import("./x.mjs")`.
# A hit in a comment only widens the closure.
_JS_IMPORT_RE = re.compile(r"""(?:\bfrom|\bimport)\s*\(?\s*["'](\.{1,2}/[^"'\n]+)["']""")
NODE_MODULE_EXT = (".mjs", ".js", ".cjs")


class MetadataError(Exception):
    """`cargo metadata --offline` did not answer: nothing is cached this run."""


# --- the tree ----------------------------------------------------------------

def _git(root, *args, data=None):
    return subprocess.run(["git", "-C", root, *args], input=data, capture_output=True,
                          check=True).stdout


def _zsplit(raw):
    return [p.decode("utf-8", "surrogateescape") for p in raw.split(b"\0") if p]


def tree_index(repo_root):
    """`{path: blob sha}` of the tree: the git index plus the overlay of the
    working tree (dirty and untracked files re-hashed, deleted files gone,
    ignored files absent). Paths are repo-relative, so two worktrees of the
    same content give the same index."""
    root = os.path.abspath(repo_root)
    out = {}
    for rec in _git(root, "ls-files", "-s", "-z").split(b"\0"):
        if not rec:
            continue
        meta, path = rec.split(b"\t", 1)
        out[path.decode("utf-8", "surrogateescape")] = meta.split()[1].decode()
    deleted = set(_zsplit(_git(root, "ls-files", "-z", "--deleted")))
    for path in deleted:
        out.pop(path, None)
    rehash = set(_zsplit(_git(root, "ls-files", "-z", "--modified"))) - deleted
    rehash |= set(_zsplit(_git(root, "ls-files", "-z", "--others", "--exclude-standard")))
    rehash = sorted(p for p in rehash
                    if "\n" not in p and os.path.isfile(os.path.join(root, p)))
    if rehash:
        data = ("\n".join(rehash) + "\n").encode("utf-8", "surrogateescape")
        shas = _git(root, "hash-object", "--stdin-paths", data=data).decode().split()
        out.update(zip(rehash, shas))
    return out


def _rel(path, base):
    rel = os.path.relpath(os.path.normpath(path), os.path.normpath(base))
    if rel == ".." or rel.startswith("../") or os.path.isabs(rel):
        return None
    return "" if rel == "." else rel.replace(os.sep, "/")


# --- units: named path sets that make up a closure ----------------------------

class _Context:
    """One tree, its index and its workspace; memoizes scans and unit digests."""

    def __init__(self, root, metadata, index=None):
        self.root = os.path.abspath(root)
        self.index = tree_index(self.root) if index is None else index
        self.paths = sorted(self.index)
        self.dirs = set()
        for p in self.paths:
            parts = p.split("/")
            for i in range(1, len(parts)):
                self.dirs.add("/".join(parts[:i]))
        self.templates = sorted(d[len("templates/"):] for d in self.dirs
                                if d.startswith("templates/") and d.count("/") == 1)
        self.examples = sorted(d[len("examples/"):] for d in self.dirs
                               if d.startswith("examples/") and d.count("/") == 1)
        self._labels = None
        self._texts = {}
        self._scans = {}
        self._digests = {}
        self._names_in = {}
        self._crate_scan = {}
        self._modules = {}
        self.pkgs, self.bins = self._workspace(metadata)
        self.crate_by_dir = {p["dir"]: n for n, p in self.pkgs.items()}

    # metadata ---------------------------------------------------------------

    def _workspace(self, metadata):
        ws_root = metadata.get("workspace_root") or self.root
        members = set(metadata.get("workspace_members") or ())
        pkgs, by_dir = {}, {}
        for p in metadata.get("packages") or ():
            if (members and p.get("id") not in members) or (not members and p.get("source")):
                continue
            d = _rel(os.path.dirname(p["manifest_path"]), ws_root)
            if d is None:
                continue
            targets = []
            for t in p.get("targets") or ():
                src = _rel(t.get("src_path", ""), ws_root)
                targets.append({"name": t["name"], "kind": list(t.get("kind") or ()), "src": src})
            pkgs[p["name"]] = {"dir": d, "targets": targets, "raw_deps": p.get("dependencies") or ()}
            by_dir[d] = p["name"]
        for name, p in pkgs.items():
            deps = []
            for dep in p.pop("raw_deps"):
                target = None
                if dep.get("path"):
                    target = by_dir.get(_rel(dep["path"], ws_root))
                elif not dep.get("source") and dep.get("name") in pkgs:
                    target = dep["name"]
                if target and target != name:
                    deps.append((dep.get("kind") or "normal", target))
            p["deps"] = deps
        bins = {}
        for name, p in pkgs.items():
            for t in p["targets"]:
                if "bin" in t["kind"]:
                    bins.setdefault(t["name"], name)
        return pkgs, bins

    # index helpers ----------------------------------------------------------

    def under(self, prefix):
        """Index paths below the directory `prefix` (no trailing slash)."""
        if not prefix:
            return list(self.paths)
        lo = bisect.bisect_left(self.paths, prefix + "/")
        hi = bisect.bisect_left(self.paths, prefix + "0")   # "0" follows "/"
        return self.paths[lo:hi]

    def labels(self, path):
        if self._labels is None:
            self._labels = {}
        got = self._labels.get(path)
        if got is None:
            got = self._labels[path] = gp._trigger_labels(path)
        return got

    def text(self, rel):
        got = self._texts.get(rel)
        if got is None:
            try:
                with open(os.path.join(self.root, rel), encoding="utf-8", errors="replace") as fh:
                    got = fh.read()
            except OSError:
                got = ""
            self._texts[rel] = got
        return got

    # unit membership and resolution -----------------------------------------

    def _crate_dir(self, crate):
        p = self.pkgs.get(crate)
        if p is not None:
            return p["dir"]
        return "crates/" + crate

    def member(self, unit, path):
        """Does `unit` contain `path`? A predicate, so paths absent from the
        tree (deletions in a diff) are answered as well."""
        if unit == "tree":
            return True
        if unit == "global":
            return (path in ("Cargo.toml", "Cargo.lock") or path.startswith(".cargo/")
                    or ("/" not in path and path.startswith("rust-toolchain")))
        head, _sep, rest = unit.partition(":")
        if head == "glob":
            return gp._matches(path, rest)
        if head == "file":
            return path == rest
        if head == "dir":
            return path.startswith(rest + "/")
        if head == "pydir":
            return posixpath.dirname(path) == rest and path.endswith(".py")
        if head == "crate":
            d = self._crate_dir(rest)
            if not path.startswith(d + "/"):
                return False
            return path[len(d) + 1:].split("/", 1)[0] not in ("tests", "benches", "examples")
        if head == "testfix":
            d = self._crate_dir(rest) + "/tests/"
            if not path.startswith(d):
                return False
            tail = path[len(d):]
            return "/" in tail or not tail.endswith(".rs")
        if head == "tool":
            return False
        if head == "mjs":
            return path in self.module_closure(rest)
        labels = self.labels(path)
        if any(fnmatch.fnmatchcase(label, unit) for label in labels):
            return True
        cls, sep, crate = unit.partition(":")
        if sep and cls in labels:
            return gp._crate_of(path) == crate or path.startswith(self._crate_dir(crate) + "/")
        return False

    def unit_paths(self, unit):
        head, _sep, rest = unit.partition(":")
        if unit == "tree":
            return list(self.paths)
        if head == "file":
            return [rest] if rest in self.index else []
        if head in ("dir", "pydir"):
            cands = self.under(rest)
        elif head in ("crate", "testfix"):
            cands = self.under(self._crate_dir(rest))
        elif head == "glob":
            lit = re.split(r"[*?\[]", rest, 1)[0]
            cands = self.under(lit.rsplit("/", 1)[0]) if "/" in lit else self.paths
        elif unit == "global":
            cands = [p for p in self.paths if "/" not in p] + self.under(".cargo")
        elif head == "mjs":
            return sorted(self.module_closure(rest))
        else:
            cands = self.paths
        return [p for p in cands if self.member(unit, p)]

    def digest(self, unit):
        got = self._digests.get(unit)
        if got is None:
            h = hashlib.sha256()
            for p in sorted(self.unit_paths(unit)):
                h.update(("%s\0%s\n" % (p, self.index[p])).encode("utf-8", "surrogateescape"))
            got = self._digests[unit] = h.hexdigest()
        return got

    # Node modules -------------------------------------------------------------

    def module_imports(self, roots):
        """`roots` (tree paths) plus every tree module they import by a relative
        specifier, transitively."""
        seen, todo = set(), list(roots)
        while todo:
            cur = todo.pop()
            if cur in seen or cur not in self.index:
                continue
            seen.add(cur)
            base = posixpath.dirname(cur)
            for rel in _JS_IMPORT_RE.findall(self.text(cur)):
                p = self._resolve(base, rel)
                if p is not None:
                    todo.append(p)
        return seen

    def module_closure(self, pattern):
        """The unit `mjs:<pattern>`: the Node modules matching the glob and
        their import closure."""
        got = self._modules.get(pattern)
        if got is None:
            roots = [p for p in self.paths
                     if p.endswith(NODE_MODULE_EXT) and gp._matches(p, pattern)]
            got = self._modules[pattern] = frozenset(self.module_imports(roots))
        return got

    def unkeyed_imports(self, specs):
        """Tree modules imported by a Node module the path specs of `specs` hold
        that no such spec holds itself (sorted). `artefact:`/`compile:` closures
        are Rust and judged by `check-dep-info`; they cover nothing here."""
        units = [u for u in specs if u != "tree"
                 and u.partition(":")[0] not in ("tool", "env", "artefact", "compile")]

        def keyed(path):
            return any(self.member(u, path) for u in units)
        roots = [p for p in self.paths if p.endswith(NODE_MODULE_EXT) and keyed(p)]
        return sorted(p for p in self.module_imports(roots) if not keyed(p))

    # source scans -----------------------------------------------------------

    def _resolve(self, base_dir, rel):
        p = posixpath.normpath(posixpath.join(base_dir, rel))
        if p.startswith("../") or p == ".." or p.startswith("/"):
            return None
        return p

    def _literal_units(self, text):
        """Repo paths the text names as literals (rule 8 of gate_plan,
        inverted): every '/'-bounded substring of a path-like run that is a
        tracked file, and a tracked directory where the run ends in it."""
        out = set()
        for m in _RUN_RE.finditer(text):
            run = m.group(0)
            if "/" not in run and "." not in run:
                continue
            comps = run.split("/")
            n = len(comps)
            for i in range(n):
                if not comps[i]:
                    continue
                for j in range(i + 1, n + 1):
                    cand = "/".join(comps[i:j])
                    if cand in self.index:
                        out.add("file:" + cand)
                        if cand.endswith(".md"):
                            alias = cand[:-3] + ".en.md"
                            if alias in self.index:
                                out.add("file:" + alias)
                    elif (j == n or (j == n - 1 and not comps[-1])) and cand in self.dirs:
                        out.add("dir:" + cand)
        return out

    def scan(self, rel):
        """`(compile units, runtime units, started binaries)` of one source."""
        got = self._scans.get(rel)
        if got is not None:
            return got
        text = self.text(rel)
        base = posixpath.dirname(rel)
        compile_units = set()
        for rx in (_INCLUDE_RE, _PATH_ATTR_RE):
            for m in rx.finditer(text):
                p = self._resolve(base, m.group(1))
                if p is not None:
                    compile_units.add("dir:" + p if p in self.dirs else "file:" + p)
        runtime = self._literal_units(text)
        bins = set(m.group(1) for m in _BIN_EXE_RE.finditer(text))
        bins |= set(m.group(1) for m in _BIN_PATH_RE.finditer(text))
        got = self._scans[rel] = (compile_units, runtime, {b for b in bins if b in self.bins})
        return got

    def crate_scan(self, crate):
        """Scans of every source of a crate outside tests/, benches/, examples/."""
        got = self._crate_scan.get(crate)
        if got is not None:
            return got
        comp, run, bins = set(), set(), set()
        for p in self.unit_paths("crate:" + crate):
            if p.endswith(".rs"):
                c, r, b = self.scan(p)
                comp |= c
                run |= r
                bins |= b
        got = self._crate_scan[crate] = (comp, run, bins)
        return got

    def _test_files(self, rel):
        """The sources of one test target: its file, the `mod` files it pulls
        in (transitively), or the whole directory of a `tests/<x>/main.rs`."""
        crate_tests = posixpath.dirname(rel)
        units, files, todo = set(), [], [rel]
        if posixpath.basename(rel) == "main.rs" and posixpath.basename(crate_tests) not in ("src",):
            units.add("dir:" + crate_tests)
            todo = [p for p in self.under(crate_tests) if p.endswith(".rs")]
        else:
            units.add("file:" + rel)
        seen = set()
        while todo:
            p = todo.pop()
            if p in seen:
                continue
            seen.add(p)
            files.append(p)
            base = posixpath.dirname(p)
            for m in _MOD_RE.finditer(self.text(p)):
                for cand in (posixpath.join(base, m.group(1) + ".rs"),
                             posixpath.join(base, m.group(1), "mod.rs")):
                    if cand in self.index:
                        units.add("file:" + cand)
                        todo.append(cand)
        return units, files

    def _test_refs(self, text):
        """Templates and examples a test references (gate_plan rules 4/5)."""
        out = set()
        if gp._is_catalogue_wide(text):
            out.add("dir:templates")
        for name in self.examples:
            if "examples/%s" % name in text:
                out.add("dir:examples/" + name)
        shipped = None
        for name in self.templates:
            if name not in text:
                continue
            if gp._template_reference_re(name).search(text):
                out.add("dir:templates/" + name)
                continue
            if shipped is None:
                shipped = gp._names_the_shipped_root(text)
            if shipped and gp._root_relative_reference_re(name).search(text):
                out.add("dir:templates/" + name)
        return out

    # closures ---------------------------------------------------------------

    def _crates_closure(self, roots, compile_only=False):
        """Units of the crates in `roots` plus their normal and build
        dependencies; runtime-started binaries pull their own closure."""
        units = {"global"}
        todo, seen = list(roots), set()
        while todo:
            crate = todo.pop()
            if crate in seen or crate not in self.pkgs:
                continue
            seen.add(crate)
            units.add("crate:" + crate)
            comp, run, bins = self.crate_scan(crate)
            units |= comp
            if not compile_only:
                units |= run
                todo.extend(self.bins[b] for b in bins)
            todo.extend(dep for kind, dep in self.pkgs[crate]["deps"] if kind != "dev")
        return units

    def artefact_units(self, name):
        crate = self.bins.get(name) or (name if name in self.pkgs else None)
        if crate is None:
            return None
        return self._crates_closure([crate])

    def part_units(self, part):
        """Units of one nextest part: `<crate>` (lib unit tests),
        `<crate>::bin/<name>` (bin unit tests) or `<crate>::<target>`."""
        crate, sep, rest = part.partition("::")
        p = self.pkgs.get(crate)
        if p is None:
            return None
        roots = [crate] + [dep for kind, dep in p["deps"] if kind == "dev"]
        extra, bins = set(), set()
        if sep and rest.startswith("bin/"):
            if not any(t["name"] == rest[4:] and "bin" in t["kind"] for t in p["targets"]):
                return None
        elif sep:
            target = next((t for t in p["targets"] if t["name"] == rest
                           and set(t["kind"]) & {"test", "bench", "example"} and t["src"]), None)
            src = target["src"] if target else None
            if src is None:
                for cand in ("%s/tests/%s.rs" % (p["dir"], rest),
                             "%s/tests/%s/main.rs" % (p["dir"], rest)):
                    if cand in self.index:
                        src = cand
                        break
            if src is None:
                return None
            own, files = self._test_files(src)
            extra |= own | {"testfix:" + crate}
            for f in files:
                comp, run, b = self.scan(f)
                extra |= comp | run
                bins |= b
                extra |= self._test_refs(self.text(f))
        units = self._crates_closure(roots + [self.bins[b] for b in bins])
        return units | extra

    def compile_all(self):
        """`compile:*` -- every crate and target, compile inputs only."""
        units = self._crates_closure(sorted(self.pkgs), compile_only=True)
        for crate, p in self.pkgs.items():
            units.add("testfix:" + crate)
            for t in p["targets"]:
                if t["src"] and set(t["kind"]) & {"test", "bench", "example"}:
                    own, files = self._test_files(t["src"])
                    units |= own
                    for f in files:
                        units |= self.scan(f)[0]
        return units

    def names_read(self, units, names):
        """The `names` some file of `units` spells (a part's own code: its test
        files, its crates' sources, their fixtures) -- `env:part`."""
        out = set()
        for unit in units:
            for p in self.unit_paths(unit):
                got = self._names_in.get(p)
                if got is None:
                    try:
                        with open(os.path.join(self.root, p), "rb") as fh:
                            raw = fh.read()
                    except OSError:
                        raw = b""
                    got = self._names_in[p] = frozenset(
                        n for n in names if n.encode("ascii") in raw)
                out |= got
        return sorted(out)

    # station-level ----------------------------------------------------------

    def spec_units(self, spec):
        """Units of one declared spec, `set()` for `tool:`, None if unknown."""
        if spec in ("tree", "global"):
            return {spec}
        head, sep, rest = spec.partition(":")
        if head in ("tool", "env"):
            return set()
        if head == "glob":
            return {spec}
        if head == "artefact":
            if rest.startswith("binary:"):
                return self.part_units(rest[len("binary:"):])
            return self.artefact_units(rest)
        if head == "compile":
            return self.compile_all() if rest == "*" else None
        if head in _SPEC_HEADS:
            return {spec}
        return {spec}   # a class or trigger label, with or without `:<crate>`

    def argv_units(self, station):
        units = set()
        for cmd in _cmds(station):
            for arg in cmd:
                if not isinstance(arg, str):
                    continue
                path = arg[2:] if arg.startswith("./") else arg
                if path not in self.index:
                    continue
                units.add("file:" + path)
                if path.endswith(".py"):
                    units |= self._py_units(path)
        return units

    def _py_units(self, path):
        base = posixpath.dirname(path)
        units = {"pydir:" + base}
        if base:
            units.add("pydir:" + posixpath.dirname(base))
        for m in _PY_IMPORT_RE.finditer(self.text(path)):
            for name in (m.group(1) or m.group(2) or "").split(","):
                top = name.strip().split(" ")[0].split(".")[0]
                if top and posixpath.join(base, top) in self.dirs:
                    units.add("dir:" + posixpath.join(base, top))
        return units


def _cmds(station):
    out = []
    for cmd in station.get("cmds") or ():
        if isinstance(cmd, dict):
            out.append(list(cmd.get("argv") or ()))
        else:
            out.append(list(cmd))
    return out


def _parts_station(station):
    return "parts" in station or bool(station.get("nextest"))


def _diff_glob(spec):
    """`glob:<literal path>` -- what the plan adds to a nextest station per
    trigger of the diff; the closures of its parts hold those paths."""
    return spec.startswith("glob:") and not any(c in spec for c in "*?[")


def _declared(station):
    """The plan's own inputs (the table row when it names none); for nextest
    stations without the per-diff literal globs. None: not cacheable."""
    if not INPUTS.get(station.get("name")):
        return None
    specs = station.get("inputs")
    if specs is None:
        specs = INPUTS[station["name"]]
    if _parts_station(station):
        specs = [s for s in specs if not _diff_glob(s)]
    return list(specs)


def _key_specs(station):
    """What enters the key: the table row of `INPUTS` -- the floor, and the
    sole judge of what is cacheable -- plus the plan's own inputs."""
    specs = _declared(station)
    if specs is None:
        return None
    return list(dict.fromkeys(list(INPUTS[station["name"]]) + specs))


def _verdict_names():
    """`lane_sync.sh VERDICT_ENV` (the one list, `lane.verdict_env`)."""
    from . import lane  # the verdict's names are the transport's list
    return lane.verdict_env()


def _semantic_env(env):
    """The fingerprint's `build_env` in one form, without the verdict variables:
    WHICH names change what is built is `lane_fingerprint.sh`'s `BUILD_ENV`
    (re-check tv-bn M7r A-M2, one source); a verdict variable changes no build and
    enters only the keys of the stations whose verdict reads it (`env:`,
    orchestrator OR-T12-21)."""
    drop = set(_verdict_names())
    return {str(k): str(v) for k, v in sorted((env or {}).items()) if str(k) not in drop}


def _env_specs(specs):
    """The verdict variables `env:<NAME>` declares, and whether `env:part` is
    declared; None when a name is no verdict variable (not cacheable)."""
    names = set(_verdict_names())
    out = [s[len("env:"):] for s in specs if s.startswith("env:") and s != "env:part"]
    if any(n not in names for n in out):
        return None
    return sorted(set(out)), "env:part" in specs


def _verdict(build_env, names):
    """The values (as `build_env` measured them, sha-16, never in clear) of
    `names`; None for a name the stations' environment does not set."""
    env = build_env or {}
    return {n: env.get(n) for n in sorted(names)}


def _tools(fingerprint, specs):
    names = set(KEY_TOOLS) | {s[len("tool:"):] for s in specs if s.startswith("tool:")}
    return {n: fingerprint.get(n) for n in sorted(names)}


# The browser duty of a station that books an engine (GH #1161/#1167, OR-TB.S7.10).
# From GH #1161 on, a station that books `browser_<engine>` runs its proofs with
# MECLAW_REQUIRE_BROWSERS=1 (an engine that does not start is RED, not SKIP), and the
# lane fingerprint carries the `browsers` probe; both come with the same executor.
# A green run from before (build02, 10.10.: WebKit SKIPped for a missing libGL.so.1,
# `browser:display` GREEN) must never answer such a run as CACHED. So the duty --
# the switch and the probe of the booked engines -- enters the key and the stored
# entry, and `lookup` serves no entry that does not carry the same duty (fail-closed).
BROWSER_ENGINES = ("chromium", "webkit")


def browser_duty(station, fingerprint):
    """None for a station that books no engine; else `{"required", "probe"}`:
    `required` "1" when the fingerprint carries the `browsers` probe (that executor
    runs a booking station with MECLAW_REQUIRE_BROWSERS=1), "0" before it; `probe`
    the probe's answer per booked engine (None when unprobed). `keys_for_plan`
    gives a "0" station no key at all (fail-closed, re-check tb-s7 H2 M-N5)."""
    needs = station.get("needs") or {}
    engines = [e for e in BROWSER_ENGINES if float(needs.get("browser_" + e) or 0) > 0]
    if not engines and "needs" not in station and str(station.get("name")).startswith("browser:"):
        engines = list(BROWSER_ENGINES)  # a hand plan without `needs`: the name books
    if not engines:
        return None
    said = (fingerprint or {}).get("browsers")
    probed = isinstance(said, dict)
    return {"required": "1" if probed else "0",
            "probe": {e: (str(said[e]) if probed and said.get(e) is not None else None)
                      for e in engines}}


def _station_units(ctx, station, specs):
    """Units of `specs` plus the station's argv paths, or None when a spec
    cannot be resolved (then the station is not cached)."""
    if specs is None:
        return None
    units = set()
    for spec in specs:
        got = ctx.spec_units(spec)
        if got is None:
            return None
        units |= got
    return units | ctx.argv_units(station)


def _key(payload):
    raw = json.dumps(payload, sort_keys=True, separators=(",", ":"), ensure_ascii=True)
    return hashlib.sha256(raw.encode("ascii")).hexdigest()


def _default_metadata(root):
    try:
        res = subprocess.run(["cargo", "metadata", "--offline", "--format-version", "1"],
                             cwd=root, capture_output=True, text=True)
    except OSError as exc:
        raise MetadataError("cargo metadata: %s" % exc)
    if res.returncode != 0:
        raise MetadataError("cargo metadata --offline failed: %s"
                            % (res.stderr.strip().splitlines() or ["exit %d" % res.returncode])[-1])
    try:
        return json.loads(res.stdout)
    except ValueError:
        raise MetadataError("cargo metadata printed no JSON")


def _narrow_state(station):
    """`{"on", "map"}` of the station's selection: narrowing used, and the
    coverage map rev it used. An unnarrowed selection is `on: False`."""
    state = ((station.get("select") or {}).get("narrow") or {})
    on = bool(state.get("on"))
    return {"on": on, "map": str(state.get("map") or "") if on else ""}


def keys_for_plan(plan, repo_root, fingerprint, lane_name, *, metadata=None, metadata_fn=None):
    """`{(station, part): {"key", "inputs", "repo"}}` for every cacheable
    station of `plan` (`part` = "" for a whole station). NEVER stations,
    stations without declared inputs and parts whose binary is unknown get no
    key. When `cargo metadata --offline` fails, nothing is cached: `{}`.
    `lane_name` does not enter the key -- a key hits across lanes."""
    del lane_name
    if metadata is None:
        try:
            metadata = (metadata_fn or _default_metadata)(repo_root)
        except (MetadataError, OSError, ValueError, subprocess.SubprocessError):
            return {}
    try:
        # GH #1199 (v2-r3-mergegate): a file the run writes before its readers
        # (`plan.derived_corpus`) is keyed as the run tests it.
        index = tree_index(repo_root)
        index.update(plan.get("derived") or {})
        ctx = _Context(repo_root, metadata, index=index)
    except (subprocess.CalledProcessError, OSError, KeyError, TypeError):
        return {}
    fingerprint = fingerprint or {}
    if fingerprint.get("unanswered"):
        # Re-check tv-bn M7r A-M1 (fail-closed): a lane that did not answer its
        # fingerprint keys nothing -- null tools on both sides were no doubt, and
        # every later null run hit entries made without a tool version.
        return {}
    env = _semantic_env(fingerprint.get("build_env"))
    verdict_names = _verdict_names()
    repo = plan.get("repo") or "meclaw"
    out = {}
    for st in plan.get("stations") or ():
        name = st.get("name")
        if name in NEVER or st.get("run") is False:
            continue
        specs = _key_specs(st)
        units = _station_units(ctx, st, specs)
        if units is None:
            continue
        tools = _tools(fingerprint, specs)
        if any(v is None for v in tools.values()):
            continue  # no tool version, no key (M7r A-M1, like "no probe, no key" below)
        declared = _env_specs(specs)
        if declared is None:
            continue  # `env:` of a name that is no verdict variable: no key
        own_env, env_part = declared
        base = {"v": KEY_VERSION, "repo": repo, "station": name, "tools": tools, "env": env}
        if own_env:
            base["verdict"] = _verdict(fingerprint.get("build_env"), own_env)
        duty = browser_duty(st, fingerprint)
        if duty is not None and duty["required"] != "1":
            # Re-check tb-s7 H2 M-N5: fail-closed. Lane runs key with the
            # fingerprint stored before the run (`fingerprint_expected`); one
            # from before the `browsers` probe would key a lax world "0" whose
            # entry (WebKit SKIPped, build02 10.10.) answers a run that has to
            # require every engine. No probe, no key: never CACHED, never stored.
            continue
        if duty is not None:  # only a booking station: every other key stays as it was
            base["browsers"] = duty
        extra = {} if duty is None else {"browsers": duty}
        if _parts_station(st):
            profile = (st.get("nextest") or {}).get("profile", "default")
            # Review Z-I gap C-A: a part run under a narrowed filter
            # (`binary_id(=X) & test(/^m::/)`) is GREEN only for what ran.
            # The resolved filter and the narrowing state are in the key, so
            # a narrowed result never serves an unnarrowed request.
            # The key carries no feature variant: the build station merges
            # `-p` sets by the diff (#1166), so a part GREEN in one variant is
            # CACHED in another. Kept on purpose; the release station
            # `crate-isolation` checks every crate in its own features.
            filter_digest = hashlib.sha256(
                str((st.get("nextest") or {}).get("filter") or "").encode("utf-8")).hexdigest()
            narrow_state = _narrow_state(st)
            for part in st.get("parts") or ():
                pu = ctx.part_units(part)
                if pu is None:
                    continue
                all_units = sorted(units | pu)
                pbase = base
                if env_part:
                    read = ctx.names_read(pu, verdict_names)
                    if read:
                        pbase = dict(base, verdict=_verdict(fingerprint.get("build_env"),
                                                            set(read) | set(own_env)))
                payload = dict(pbase, part=part, argv=["nextest-profile", profile],
                               units=[[u, ctx.digest(u)] for u in all_units],
                               filter=filter_digest, narrow=narrow_state)
                out[(name, part)] = dict(extra, key=_key(payload), repo=repo,
                                         inputs=all_units + sorted(s for s in specs
                                                                   if s.startswith("tool:")))
            continue
        all_units = sorted(units)
        argv = [[cmd.get("cwd") or "" if isinstance(cmd, dict) else "", a]
                for cmd, a in zip(st.get("cmds") or (), _cmds(st))]
        payload = dict(base, part="", argv=argv,
                       units=[[u, ctx.digest(u)] for u in all_units])
        out[(name, "")] = dict(extra, key=_key(payload), repo=repo,
                               inputs=all_units + sorted(s for s in specs if s.startswith("tool:")))
    return out


# --- the store ---------------------------------------------------------------

def _parse_at(value):
    if isinstance(value, datetime.datetime):
        return value if value.tzinfo else value.replace(tzinfo=datetime.timezone.utc)
    text = str(value).replace("Z", "+00:00")
    dt = datetime.datetime.fromisoformat(text)
    return dt if dt.tzinfo else dt.replace(tzinfo=datetime.timezone.utc)


def part_keys(keys):
    """`{"<station>::<part>": key}` of the plan's part keys -- the executor
    proves with them that a subtracted part's hit has the plan's key (GH #1167 i)."""
    return {"%s::%s" % k: v["key"] for k, v in (keys or {}).items() if k[1]}


def lookup(db, keys, repo, mode, lane_name, now):
    """Cache hits for `keys` as `cache_known` entries: GREEN/FLAKY rows of
    the same repo no older than 7 days; in release mode only rows of the
    same lane no older than 24 hours."""
    want = {v["key"]: k for k, v in keys.items() if k[0] not in NEVER}
    duty = {v["key"]: v.get("browsers") for v in keys.values()}
    now_dt = _parse_at(now)
    limit = RELEASE_AGE if mode == "release" else MAX_AGE
    out, ids = [], sorted(want)
    for i in range(0, len(ids), 500):
        chunk = ids[i:i + 500]
        rows = db.execute("SELECT key, station, part, verdict, run, runner, at, inputs FROM cache "
                          "WHERE key IN (%s)" % ",".join("?" * len(chunk)), chunk).fetchall()
        for row in rows:
            station, part = want[row["key"]]
            if row["verdict"] not in CACHEABLE_VERDICTS:
                continue
            if (row["station"], row["part"] or "") != (station, part):
                continue
            try:
                meta = json.loads(row["inputs"] or "{}")
                at = _parse_at(row["at"])
            except (ValueError, TypeError):
                continue
            if not isinstance(meta, dict) or meta.get("repo") != repo:
                continue
            # OR-TB.S7.10: an entry without the browser duty of the key, or with
            # another, never serves -- fail-closed, not only by the hash.
            if duty.get(row["key"]) is not None and meta.get("browsers") != duty[row["key"]]:
                continue
            if mode == "release" and row["runner"] != lane_name:
                continue
            if now_dt - at > limit:
                continue
            out.append({"station": station, "part": part, "key": row["key"],
                        "from_run": row["run"]})
    out.sort(key=lambda k: (k["station"], k["part"]))
    return out


def record(db, run, runner, keys, results):
    """Store the GREEN/FLAKY results of a run under their keys -- never RED,
    SKIP, NOTE, ASK, ABORTED or CACHED, never a NEVER station."""
    at = store.now()
    for res in results:
        station, part = res.get("station"), res.get("part") or ""
        if res.get("verdict") not in CACHEABLE_VERDICTS or station in NEVER:
            continue
        entry = keys.get((station, part))
        if not entry:
            continue
        meta = {"repo": entry.get("repo"), "inputs": entry.get("inputs", [])}
        if entry.get("browsers") is not None:
            meta["browsers"] = entry["browsers"]
        store.cache_put(db, entry["key"], station, part, res["verdict"], run, runner,
                        meta, at=at)


# --- checks ------------------------------------------------------------------

def uncovered_triggers(plan, root, metadata):
    """`[(station, reason)]` for every resolver trigger (`<label>: <path>` in
    a station's `reason`) whose path lies outside the station's declared
    inputs -- under-declaration, the error the cache must never make.
    Over-declaration is allowed; NEVER stations and stations missing from
    `INPUTS` are not cached and so cannot under-declare. A nextest station is
    judged without its per-diff literal globs: its parts must cover."""
    ctx = _Context(root, metadata)
    gaps = []
    for st in plan.get("stations") or ():
        name = st.get("name")
        if name in NEVER:
            continue
        units = _station_units(ctx, st, _declared(st))
        if units is None:
            continue
        units = set(units)
        for part in st.get("parts") or ():
            units |= ctx.part_units(part) or set()
        for reason in st.get("reason") or ():
            label, sep, path = reason.partition(": ")
            if not sep or reason.startswith("+") or label in _REASON_NO_PATH:
                continue
            if not any(ctx.member(u, path) for u in units):
                gaps.append((name, reason))
    return gaps


# V2-A5 (GH #1199): what a station outside cargo reads shows only when it runs --
# modules on a `sys.path` the code builds, `git ls-files`, directory walks, data
# files next to it (measured: plans/welle-validierung-2-2026-10-14/recheck/skripte/
# v2-fix-b-cache/station_closure.py). Such a station keys the whole tree, like
# gate-selftest. The one exception is a program that reads exactly the files on
# its argv; the flags here make it follow more and void the exception.
# A cargo station answers to `check-dep-info` for its Rust, but a Node driver it
# starts (`browser:display`, `browser:alone`) reads modules no dep-info lists:
# every station keys the import closure of each Node module its key holds
# (review v2-fix-b I2), checked against the tree `ctx`; without a tree a station
# that keys `tool:node` is a gap (fail-closed).
ARGV_ONLY = {"shellcheck": ("-x", "--external-sources", "-P", "--source-path")}


def unkeyed_reads(stations, ctx=None):
    """`[(station, reason)]` for every cacheable station whose key holds less
    than the whole tree and either misses a module a keyed Node module imports
    (`ctx`, a `_Context` of the tree; without one, any station keying
    `tool:node`), or is no cargo station (those answer to `check-dep-info` for
    their Rust) and is not made of `ARGV_ONLY` programs without their follow
    flags. Empty: every cached station keys what it reads."""
    gaps = []
    for st in stations:
        name = st.get("name")
        if name in NEVER:
            continue
        specs = _key_specs(st)
        if specs is None or "tree" in specs or _parts_station(st):
            continue
        if ctx is not None:
            missing = ctx.unkeyed_imports(specs)
            if missing:
                gaps.append((name, "keys %s, not the modules its Node modules import: %s"
                             % (",".join(specs), ", ".join(missing))))
                continue
        elif "tool:node" in specs:
            gaps.append((name, "runs Node; the import closure of its modules needs the tree"))
            continue
        if st.get("cargo"):
            continue
        bad = []
        for argv in _cmds(st):
            prog = posixpath.basename(argv[0]) if argv else ""
            follow = ARGV_ONLY.get(prog)
            if follow is None:
                bad.append(prog or "?")
            elif any(a in follow or any(a.startswith(f + "=") for f in follow) for a in argv[1:]):
                bad.append("%s %s" % (prog, " ".join(a for a in argv[1:] if a.startswith("-"))))
        if bad:
            gaps.append((name, "keys %s, not the tree, but runs %s"
                         % (",".join(specs), ", ".join(sorted(set(bad))))))
    return gaps


def _parse_dep_info(text):
    deps = set()
    for line in text.splitlines():
        if not line.strip() or line.startswith("#"):
            continue
        m = re.match(r"^((?:[^:\\]|\\.)*):(?:\s(.*))?$", line)
        if not m or not m.group(2):
            continue
        for tok in re.split(r"(?<!\\)\s+", m.group(2).strip()):
            if tok:
                deps.add(tok.replace("\\ ", " "))
    return deps


def _parts_for_target(ctx, name):
    out = []
    for crate, p in sorted(ctx.pkgs.items()):
        for t in p["targets"]:
            if t["name"].replace("-", "_") != name:
                continue
            kinds = set(t["kind"])
            if kinds & {"test", "bench", "example"}:
                out.append("%s::%s" % (crate, t["name"]))
            elif "bin" in kinds:
                out.append("%s::bin/%s" % (crate, t["name"]))
            else:
                out.append(crate)
    if name == "build_script_build":
        out += sorted(c for c, p in ctx.pkgs.items()
                      if any("custom-build" in t["kind"] for t in p["targets"]))
    return out


def dep_info_gaps(target, root, metadata, home=None):
    """Compare the dep-info files of a real build (`<target>/*/deps/*.d`)
    with the declared closures: `(checked, gaps)`, each a sorted list of
    `(part, path)`. Paths outside the tree, in the target dir or under
    `~/.cargo`/`~/.rustup` are not checked."""
    ctx = _Context(root, metadata)
    home = os.path.abspath(home or os.path.expanduser("~"))
    target = os.path.abspath(target)
    skip = [target + os.sep, os.path.join(home, ".cargo") + os.sep,
            os.path.join(home, ".rustup") + os.sep]
    files = sorted(set(globmod.glob(os.path.join(target, "*", "deps", "*.d"))
                       + globmod.glob(os.path.join(target, "*", "*", "deps", "*.d"))))
    checked, gaps, closures = set(), set(), {}
    for dfile in files:
        stem = os.path.basename(dfile)[:-2]
        name = stem.rsplit("-", 1)[0] if "-" in stem else stem
        parts = _parts_for_target(ctx, name)
        if not parts:
            continue
        try:
            with open(dfile, encoding="utf-8", errors="replace") as fh:
                deps = _parse_dep_info(fh.read())
        except OSError:
            continue
        for part in parts:
            if part not in closures:
                closures[part] = ctx.part_units(part) or set()
        for dep in deps:
            path = os.path.normpath(dep if os.path.isabs(dep) else os.path.join(ctx.root, dep))
            if any((path + os.sep).startswith(s) or path.startswith(s) for s in skip):
                continue
            rel = _rel(path, ctx.root)
            if not rel:
                continue
            if rel == "target" or rel.startswith("target/"):
                continue
            label = parts[0]
            checked.add((label, rel))
            if not any(ctx.member(u, rel) for part in parts for u in closures[part]):
                gaps.add((label, rel))
    return sorted(checked), sorted(gaps)


# --- CLI -----------------------------------------------------------------------

def _cmd_check_dep_info(args):
    root = os.path.abspath(args.root)
    try:
        if args.metadata:
            with open(args.metadata, encoding="utf-8") as fh:
                metadata = json.load(fh)
        else:
            metadata = _default_metadata(root)
    except (MetadataError, OSError, ValueError) as exc:
        print("check-dep-info: %s" % exc, file=sys.stderr)
        return 2
    checked, gaps = dep_info_gaps(args.target, root, metadata, home=args.home)
    for part, path in gaps:
        print("%s\t%s" % (part, path))
    print("check-dep-info: %d files checked, %d gaps" % (len(checked), len(gaps)), file=sys.stderr)
    return 1 if gaps else 0


def main(argv=None):
    ap = argparse.ArgumentParser(prog="cache", description="station cache tools")
    sub = ap.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("check-dep-info", help="dep-info of a real build against the closures")
    p.add_argument("--target", required=True, help="CARGO_TARGET_DIR of the build")
    p.add_argument("--root", default=".", help="the tree (default: cwd)")
    p.add_argument("--metadata", help="cargo metadata JSON file instead of running cargo")
    p.add_argument("--home", help="home whose ~/.cargo and ~/.rustup are skipped")
    p.set_defaults(func=_cmd_check_dep_info)
    args = ap.parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
