"""`spawn_closure` -- code that runs only in a spawned binary (welle-tb S1b, GH #1165).

WHY THIS EXISTS
===============
Integration tests start workspace binaries as child processes
(`env!("CARGO_BIN_EXE_meclaw")`). The child inherits LLVM_PROFILE_FILE, so its
counters land in the test's profile, but `llvm-cov export` attributes a
counter to a source line only through an object that carries the function's
coverage mapping. Code linked into the child and NOT into the test binary --
the bin's `main.rs`, and every lib function the test binary does not link
itself -- is invisible unless the child's executable is an `-object` of the
export. Until S1b none was (`objects_from_nextest_list` asked for kind `bin`,
nextest says `bin-exe`): a change to such code could drop exactly the tests
that run it. That is a fail-open hole in the narrowing.

Two halves:

* BUILD (`child_objects`, `marker`): every executable of a non-test binary
  -- the uplifted `debug/<name>` and its `debug/deps/<name>-<hash>` copies
  (two packages may emit the same bin name; the uplift keeps one) -- goes to
  every export, and the map gets

      "subprocess": {"<crate>::bin/<name>": {"crate", "name", "root",
                                            "proven": [[binary, module|null], ...]}}

  An entry exists only when the build passed an executable of that bin name;
  `proven` lists the parts with >= 1 executed line in the bin's ROOT file. The
  root is compiled into that bin alone, so a hit proves that the part's child
  profiles were merged AND mapped. Only the build writes this field.

* NARROWING (`fail_closed_terms`): when the map replaced `rdeps(X)`, every
  changed Rust source of a crate in the closure of a spawned bin selects the
  units that spawn it (`CARGO_BIN_EXE_<name>` in their source, the rule-3
  term of that file) -- unless the map proves the unit for a bin of that
  name whose closure holds the file, and the file is no other bin's root.
  No marker means fail-closed. Strand, narrowed merge and narrowed
  integration all plan through `impact`, so all three get it.
"""

import os
import re
import stat

_SCRIPTS = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

FIELD = "subprocess"
# a name may be empty: `concat!("CARGO_BIN_EXE_", name)` spawns a bin of the
# package nobody can name statically -- every bin of the crate (fail-closed)
_SPAWN = re.compile(r"CARGO_BIN_EXE_([A-Za-z0-9_-]*)")
_DEPS_HASH = re.compile(r"^(.+)-[0-9a-f]{16}$")
_PART = re.compile(r"^\(binary_id\(=([^)]+)\) & test\(/\^(.+)::/\)\)$")
_BIN = re.compile(r"^binary_id\(=([^)]+)\)$")

_BIN_CACHE = {}
_SPAWN_CACHE = {}


def clear_caches():
    _BIN_CACHE.clear()
    _SPAWN_CACHE.clear()


def _norm_name(name):
    return name.replace("-", "_")


# --- the workspace's bins ----------------------------------------------------

def _bins_of_crate(cdir, crate):
    """`{name: root}` of one crate's bin targets (Cargo's target rules, paths
    relative to the crate). A manifest that does not parse yields nothing --
    its spawners then never prove (fail-closed)."""
    import tomllib
    try:
        with open(os.path.join(cdir, "Cargo.toml"), "rb") as fh:
            man = tomllib.load(fh)
    except (OSError, ValueError):
        return {}
    pkg = man.get("package") or {}
    out = {}
    for b in man.get("bin") or []:
        name = b.get("name")
        if not name:
            continue
        path = b.get("path")
        if not path:
            for cand in ("src/bin/%s.rs" % name, "src/bin/%s/main.rs" % name):
                if os.path.isfile(os.path.join(cdir, cand)):
                    path = cand
                    break
            else:
                path = "src/main.rs" if name == pkg.get("name") else "src/bin/%s.rs" % name
        out[name] = os.path.normpath(path)
    if pkg.get("autobins", True) is not False:
        taken = set(out.values())
        if os.path.isfile(os.path.join(cdir, "src/main.rs")) and "src/main.rs" not in taken:
            out.setdefault(pkg.get("name") or crate, "src/main.rs")
        bdir = os.path.join(cdir, "src/bin")
        if os.path.isdir(bdir):
            for e in sorted(os.listdir(bdir)):
                if e.endswith(".rs"):
                    path, name = "src/bin/" + e, e[:-3]
                elif os.path.isfile(os.path.join(bdir, e, "main.rs")):
                    path, name = "src/bin/%s/main.rs" % e, e
                else:
                    continue
                if path not in taken:
                    out.setdefault(name, path)
    return out


def bin_targets(repo):
    """`{"<crate>::bin/<name>": {"crate", "name", "root"}}` over `crates/*`."""
    key = os.path.abspath(repo)
    hit = _BIN_CACHE.get(key)
    if hit is not None:
        return hit
    out = {}
    cdir = os.path.join(repo, "crates")
    if os.path.isdir(cdir):
        for crate in sorted(os.listdir(cdir)):
            for name, root in sorted(_bins_of_crate(os.path.join(cdir, crate), crate).items()):
                out["%s::bin/%s" % (crate, name)] = {
                    "crate": crate, "name": name, "root": "crates/%s/%s" % (crate, root)}
    _BIN_CACHE[key] = out
    return out


def _bin_only(path, b):
    """Is `path` compiled into bin `b` alone -- its root, or a file of its
    `src/bin/<d>/` directory? (A module `src/main.rs` declares next to a lib
    counts as lib, as in `impact.own_unit_term`; none does today.)"""
    if path == b["root"]:
        return True
    if b["root"].endswith("/main.rs") and "/src/bin/" in b["root"]:
        return path.startswith(b["root"][:-len("main.rs")])
    return False


# --- the build half ------------------------------------------------------------

def _exe_name(path):
    """The bin name of an executable: `debug/meclaw` and `debug/deps/meclaw-<16 hex>`."""
    base = os.path.basename(path)
    m = _DEPS_HASH.match(base)
    return _norm_name(m.group(1) if m else base)


def child_objects(bins):
    """`bins` (the uplifted `<target>/<profile>/<name>` executables) plus every
    `<target>/<profile>/deps/<name>-<16 hex>` executable, deduplicated by inode.

    Two packages that emit one bin name (`meclaw` and `meclaw-cli` both build
    `meclaw`) leave ONE uplifted file; a test may have spawned the other. Every
    hashed copy goes to the export -- an extra object costs load time, a
    missing one silently drops the child's lines."""
    out, seen = [], set()

    def add(path):
        try:
            st = os.stat(path)
        except OSError:
            return
        if not stat.S_ISREG(st.st_mode) or not st.st_mode & 0o111:
            return
        if (st.st_dev, st.st_ino) in seen:
            return
        seen.add((st.st_dev, st.st_ino))
        out.append(path)

    for path in sorted(bins):
        add(path)
        name = _exe_name(path)
        deps = os.path.join(os.path.dirname(path), "deps")
        try:
            entries = sorted(os.listdir(deps))
        except OSError:
            continue
        for e in entries:
            m = _DEPS_HASH.match(e)
            if m and _norm_name(m.group(1)) == name:
                add(os.path.join(deps, e))
    return out


def marker(parts, objects, root):
    """The map's `subprocess` field: one entry per bin of `root` whose name an
    executable in `objects` carries -- the export saw its coverage mapping --
    with the parts that executed its root file (`proven`)."""
    names = {_exe_name(o) for o in objects or ()}
    out = {}
    for key, b in sorted(bin_targets(root).items()):
        if _norm_name(b["name"]) not in names:
            continue
        proven = sorted({(p["binary"], p.get("module")) for p in parts or ()
                         if (p.get("files") or {}).get(b["root"], 0) > 0},
                        key=lambda x: (x[0], x[1] or ""))
        out[key] = dict(b, proven=[list(x) for x in proven])
    return out


# --- the narrowing half --------------------------------------------------------

def _is_comment(line):
    return line.lstrip().startswith("//")


def spawners(repo):
    """`{(crate, name): [file, ...]}` -- test sources under `crates/<c>/tests/`
    that name `CARGO_BIN_EXE_<name>` outside a `//` comment (name "" for a
    dynamic one)."""
    key = os.path.abspath(repo)
    hit = _SPAWN_CACHE.get(key)
    if hit is not None:
        return hit
    out = {}
    cdir = os.path.join(repo, "crates")
    for crate in sorted(os.listdir(cdir)) if os.path.isdir(cdir) else ():
        tdir = os.path.join(cdir, crate, "tests")
        for dirpath, dirnames, filenames in os.walk(tdir):
            dirnames.sort()
            for f in sorted(filenames):
                if not f.endswith(".rs"):
                    continue
                full = os.path.join(dirpath, f)
                try:
                    with open(full, encoding="utf-8", errors="replace") as fh:
                        lines = fh.read().splitlines()
                except OSError:
                    continue
                rel = os.path.relpath(full, repo).replace(os.sep, "/")
                for name in sorted({m.group(1) for ln in lines if not _is_comment(ln)
                                    for m in _SPAWN.finditer(ln)}):
                    out.setdefault((crate, name), []).append(rel)
    _SPAWN_CACHE[key] = out
    return out


def _unit_of(term):
    """`(binary, module|None)` of a part term, None for anything else (rdeps)."""
    m = _PART.match(term)
    if m:
        return m.group(1), m.group(2)
    m = _BIN.match(term)
    if m:
        return m.group(1), None
    return None


def _proven(unit, proven):
    if unit is None:
        return False
    binary, module = unit
    for b, m in proven:
        if b != binary:
            continue
        # a binary-level part proves its modules; a whole-binary unit needs
        # one proven part of its binary
        if m is None or module is None or m == module:
            return True
    return False


def fail_closed_terms(files, coverage, repo, users_of, kept=()):
    """`{term: {reason}}` the narrowing must add for `files`.

    `users_of(crate)` -> the crates whose build holds `crate` (`impact.rdeps`);
    `kept` are the terms already planned -- a spawner a kept `rdeps(X)` already
    selects is not repeated."""
    import gate_plan
    marks = (coverage or {}).get(FIELD)
    marks = marks if isinstance(marks, dict) else {}
    bins = bin_targets(repo)
    spawn = spawners(repo)
    kept_rdeps = [t[len("rdeps("):-1] for t in kept if t.startswith("rdeps(")]
    covered = set()
    for x in kept_rdeps:
        covered |= set(users_of(x))
    users_cache = {}
    terms_cache = {}
    out = {}
    for p in files:
        if "rust_src" not in gate_plan._classes_of_path(p):
            continue
        c = gate_plan._crate_of(p)
        if c not in users_cache:
            users_cache[c] = set(users_of(c))
        users = users_cache[c]
        for (crate, name), srcs in sorted(spawn.items()):
            if crate not in users or crate in covered:
                continue
            key = "%s::bin/%s" % (crate, name) if name else "%s::bin/*" % crate
            for src in srcs:
                if src not in terms_cache:
                    terms_cache[src] = sorted(gate_plan.test_terms([src], repo))
                for term in terms_cache[src]:
                    if _trusted(_unit_of(term), p, name, users, marks, bins):
                        continue
                    out.setdefault(term, set()).add(
                        "spawn:unproven %s file:%s" % (key, p))
    return out


def _trusted(unit, path, name, users, marks, bins):
    """Does the map prove `unit`'s children for a bin called `name` that holds `path`?"""
    if not name or unit is None:
        return False
    for key, m in marks.items():
        if not isinstance(m, dict) or _norm_name(str(m.get("name"))) != _norm_name(name):
            continue
        if m.get("crate") not in users:
            continue    # that bin does not build `path`
        if any(_bin_only(path, b) for k, b in bins.items() if k != key):
            continue    # another bin's own file: this proof does not reach it
        if _proven(unit, [tuple(x) for x in m.get("proven") or ()
                          if isinstance(x, (list, tuple)) and len(x) == 2]):
            return True
    return False
