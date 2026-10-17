"""The lifted-file check of the merge (V2-A6, GH #1199, K9).

A test file that a strand lifts to the new format (AGENTS.md rule 11a) or
fixes as FLAKY must meet the testing bans over the WHOLE file -- the precheck
(`scripts/testing_bans.py`) grades added lines only, and gh850 merged as
lifted with a 300 ms absence window left in an unchanged line. `strand.sh
merge` refuses such a strand with the hits (`path:line ban-id`).

WHICH FILES COUNT AS LIFTED
===========================
    (a) declared   `"lifted": ["<path>", ...]` in `plans/<wave>/straenge/<S>.json`,
                   or the head line `lifted: [<path>, ...]` of the strand report
    (b) legacy     a test the diff drops from `.config/flaky-legacy.toml`: the
                   file that defines its test function at the tip (found by
                   `git grep` in the crate, narrowed by the harness name); a
                   test whose file is gone at the tip has nothing to grade; a
                   name found nowhere makes every changed test file lifted
                   (fail closed). This holds without any declaration.
    (c) FLAKY fix  `"flaky_fix": true` in the strand file (or `flaky_fix: true`
                   in the report head): every changed file a ban grades; a
                   pure move (R100, the same blob) is no change (GH #1199 R1.1).

The files are read at the branch tip, with the ban data and the register of
the tip, in a scratch tree where they stand untracked -- so the module grades
every line (`testing_bans.added_lines`: an untracked file adds every line). An
exception comment with a valid ruling stays an exception there too.

ONE PRECISION OF THE WHOLE-FILE VIEW (OR-V2.v2-fix-b.G.1)
=========================================================
The whole file holds code no added line reaches, and one of its forms is no
finding; the precheck (`scripts/testing_bans.py`, reserved) stays as it is:
    paused helper  a hit of a ban with the `start_paused` context in a fn
                   without a test attribute, not `pub`, named once in the file,
                   whose every reference in the file lies in a fn that runs
                   paused -- a `start_paused = true` test or such a helper,
                   to a fixpoint. Its sleep runs on the virtual clock.
An `rs-colony-db` hit has no precision: a test names the colony's database in
no form, a boot included (TV-A3), and this module judges it as the precheck
does (F2 withdrew the boot-name allow list of OR-G.3/F1.1).
"""

import functools
import importlib.util
import os
import re
import subprocess
import tempfile
import tomllib

LEGACY_FILE = ".config/flaky-legacy.toml"
# What the scratch tree needs besides the graded files.
DATA_FILES = (".config/testing-bans.toml", ".config/testing-rulings.toml", "ruff.toml")
_HIT = re.compile(r"^(\S+:\d+) (\S+) ")


@functools.lru_cache(maxsize=1)
def _bans_module():
    """The kit's own `scripts/testing_bans.py` (beside this package)."""
    path = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
                        "testing_bans.py")
    spec = importlib.util.spec_from_file_location("_lifted_testing_bans", path)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def _git(root, *args):
    return subprocess.run(["git", "-C", root, "-c", "core.quotePath=false"] + list(args),
                          capture_output=True, text=True)


def _show(root, rev, rel):
    res = _git(root, "show", "%s:%s" % (rev, rel))
    return res.stdout if res.returncode == 0 else None


def _flag(value):
    return value is True or str(value).strip().lower() in ("true", "yes", "ja")


def declared(doc, head):
    """(lifted paths, flaky_fix) of the strand file `doc` and the report `head`.

    Raises ValueError on a strand file whose fields have the wrong type."""
    paths = doc.get("lifted", [])
    if not isinstance(paths, list) or not all(isinstance(p, str) for p in paths):
        raise ValueError("`lifted` is %r (expected a list of paths)" % (paths,))
    if not isinstance(doc.get("flaky_fix", False), bool):
        raise ValueError("`flaky_fix` is %r (expected true or false)" % (doc.get("flaky_fix"),))
    head = head or {}
    raw = head.get("lifted", "")
    paths = paths + [p.strip().strip("\"'`") for p in raw.strip("[]").split(",") if p.strip()]
    return sorted(set(paths)), bool(doc.get("flaky_fix")) or _flag(head.get("flaky_fix", ""))


def dropped_legacy(root, base, tip):
    """The test names `.config/flaky-legacy.toml` holds at `base` and not at `tip`."""
    def names(rev):
        text = _show(root, rev, LEGACY_FILE)
        try:
            doc = tomllib.loads(text or "")
        except tomllib.TOMLDecodeError:
            return set()
        return {e.get("test") for e in doc.get("flaky", []) if isinstance(e, dict)
                and isinstance(e.get("test"), str)}
    return sorted(names(base) - names(tip))


def _crate_dir(root, rev, crate):
    """`crates/<dir>` of the package `crate` at `rev`, or None."""
    res = _git(root, "grep", "-l", "-E", r'^name\s*=\s*"%s"' % re.escape(crate), rev,
               "--", "crates/*/Cargo.toml")
    for line in res.stdout.splitlines():
        rel = line.split(":", 1)[1] if ":" in line else line
        return os.path.dirname(rel)
    return None


def files_of_test(root, rev, name):
    """The files at `rev` that define the legacy test `name` ([] = none there).

    `<path>.py::...` names its file; a dotted unittest id `<module>.<Class>.<test>`
    lives in `scripts/tests/<module>.py`; a nextest name `<crate>::<harness>
    [<stem>::]<test>` is the file of the crate that defines `fn <test>`,
    narrowed to the paths that carry the harness or stem name."""
    head = name.split("::", 1)[0]
    if head.endswith(".py"):
        return [head] if _show(root, rev, head) is not None else []
    if "::" not in name:
        rel = "scripts/tests/%s.py" % name.split(".", 1)[0]
        return [rel] if _show(root, rev, rel) is not None else []
    segs = [s for s in re.split(r"::|\s+", name) if s]
    crate, fn = segs[0], segs[-1]
    cdir = _crate_dir(root, rev, crate)
    if cdir is None:
        return []
    res = _git(root, "grep", "-l", "-E", r"\bfn\s+%s\s*[(<]" % re.escape(fn), rev, "--", cdir)
    found = [line.split(":", 1)[1] for line in res.stdout.splitlines() if ":" in line]
    def score(path):   # harness and stem name path components, not substrings
        parts = {os.path.splitext(c)[0] for c in path.split("/")}
        return sum(1 for s in segs[1:-1] if s in parts)
    best = max((score(p) for p in found), default=0)
    return sorted(p for p in found if score(p) == best)


def changed_graded(root, base, tip):
    """The files of `base..tip` that a ban grades and that exist at `tip`.

    A pure move (rename with the same blob, R100) changes no line and lifts
    nothing: the gh877 merge (16.10.) was refused with 13 hits, three files of
    them byte-equal moves of a harness refold (GH #1199 R1.1). A moved file
    with a change counts under its new path, and so does a pure move that
    changes how a ban grades the file (into or out of a ban scope, M1 of the
    v2-r3-bans re-check)."""
    tb = _bans_module()
    bans = tb._toml_text(_show(root, tip, DATA_FILES[0]) or "").get("ban", [])
    res = _git(root, "diff", "--find-renames", "--raw", "--no-abbrev", "-z", base, tip)
    if res.returncode != 0:
        raise ValueError("git diff %s %s failed: %s" % (base[:9], tip[:9], res.stderr.strip()))
    paths, fields = [], res.stdout.split("\0")
    i = 0
    while i < len(fields) - 1:
        meta = fields[i].split()        # :<mode> <mode> <src> <dst> <status>
        status = meta[4] if len(meta) == 5 else ""
        if status[:1] in ("R", "C"):
            src, dst, i = fields[i + 1], fields[i + 2], i + 3
            if status[:1] == "R" and meta[2] == meta[3] and all(
                    tb.in_scope(b, src) == tb.in_scope(b, dst) for b in bans):
                continue                # a pure move within one scope: same grading
        else:
            dst, i = fields[i + 1], i + 2
        paths.append(dst)
    return sorted(p for p in paths
                  if any(tb.in_scope(b, p) for b in bans) and _show(root, tip, p) is not None)


def lifted_files(root, base, tip, doc, head):
    """(sorted lifted paths that exist at `tip`, notes) -- (a), (b) and (c) above."""
    paths, flaky_fix = declared(doc, head)
    notes = []
    out = set(paths)
    if flaky_fix:
        out |= set(changed_graded(root, base, tip))
    for name in dropped_legacy(root, base, tip):
        at_tip = files_of_test(root, tip, name)
        if at_tip:
            out |= set(at_tip)
        elif files_of_test(root, base, name):
            notes.append("%s: its file is gone at the tip" % name)
        else:
            notes.append("%s: no file found -- every changed test file counts" % name)
            out |= set(changed_graded(root, base, tip))
    return sorted(p for p in out if _show(root, tip, p) is not None), notes


def missing_declared(root, base, tip, doc, head):
    """(refusals, notes) of the declared paths (a) that are not at `tip`.

    Fail-closed: a declared path the diff `base..tip` does not delete (a typo,
    a moved file) refuses the merge; one the diff deletes is a note."""
    paths, _ = declared(doc, head)
    gone = [p for p in paths if _show(root, tip, p) is None]
    if not gone:
        return [], []
    res = _git(root, "diff", "--no-renames", "--name-only", "--diff-filter=D", base, tip)
    if res.returncode != 0:
        raise ValueError("git diff %s %s failed: %s" % (base[:9], tip[:9], res.stderr.strip()))
    deleted = set(res.stdout.splitlines())
    return (["%s: declared lifted, missing at the tip" % p for p in gone if p not in deleted],
            ["%s: declared lifted, deleted in the diff" % p for p in gone if p in deleted])


def _materialize(root, tip, paths, into):
    """Write `paths`, the ban data and the module parents of `src` files from
    `tip` into the scratch tree `into`."""
    extra = set(DATA_FILES)
    for p in paths:
        m = re.match(r"(crates/[^/]+/src)/", p)
        if m:   # cfg-test scope: the module tree decides whether `p` is test code
            ls = _git(root, "ls-tree", "-r", "--name-only", tip, "--", m.group(1))
            extra |= {q for q in ls.stdout.splitlines() if q.endswith(".rs")}
    for rel in sorted(set(paths) | extra):
        text = _show(root, tip, rel)
        if text is None:
            continue
        full = os.path.join(into, rel)
        os.makedirs(os.path.dirname(full), exist_ok=True)
        with open(full, "w", encoding="utf-8") as fh:
            fh.write(text)


def hits(root, tip, paths, today=None):
    """The ban findings of `paths` at `tip` over every line: ["path:line ban-id", ...]."""
    if not paths:
        return []
    tb = _bans_module()
    if _show(root, tip, DATA_FILES[0]) is None:
        return ["%s: no %s at the tip -- nothing to grade against" % (p, DATA_FILES[0])
                for p in paths]
    env = dict(os.environ, GIT_AUTHOR_NAME="kit", GIT_AUTHOR_EMAIL="kit@invalid",
               GIT_COMMITTER_NAME="kit", GIT_COMMITTER_EMAIL="kit@invalid")
    with tempfile.TemporaryDirectory(prefix="meclaw-lifted-") as tmp:
        subprocess.run(["git", "init", "-q", tmp], check=True, capture_output=True)
        subprocess.run(["git", "-C", tmp, "commit", "-q", "--allow-empty", "-m", "empty"],
                       check=True, capture_output=True, env=env)
        _materialize(root, tip, paths, tmp)
        found = tb.scan(list(paths), tmp, base="HEAD", today=today)
        bans = {b["id"]: b for b in tb.load_bans(tmp)}
        out, sources = [], {}
        for f in found:
            if f.check != "test-bans" or f.message[:1] == "(":
                continue   # an exception with a valid ruling, or a note about the tooling
            m = _HIT.match(f.message)
            if m is None:
                out.append(f.message)
                continue
            path, line = m.group(1).rsplit(":", 1)
            ban = bans.get(m.group(2))
            if ban is not None and "[exception refused" not in f.message:
                if path not in sources:
                    sources[path] = tb._read_source(tmp, path, list(bans.values()))
                if sources[path] is not None and _precise(tb, sources[path], ban, int(line)):
                    continue
            out.append("%s %s" % (m.group(1), m.group(2)))
    return sorted(set(out))


# --- the paused-helper precision (OR-V2.v2-fix-b.G.1) ------------------------------

_TEST_ATTR = re.compile(r"#\[(?:tokio::)?test\b")
_PAUSED = re.compile(r"start_paused\s*=\s*true")


def _fns(src):
    """[(name, attrs, open, close, pub, at)] of every fn body of `src`."""
    out = []
    for m in re.finditer(r"\bfn\s+(\w+)", src.code):
        block = src.block_after(m.end())
        if block is None:
            continue
        head = src.code[max(0, m.start() - 400):m.start()]
        attrs = " ".join(re.findall(r"#\[[^\]]*\]", head.rsplit("}", 1)[-1].rsplit(";", 1)[-1]))
        lead = src.code[src.code.rfind("\n", 0, m.start()) + 1:m.start()]
        out.append((m.group(1), attrs, block[0], block[1], bool(re.search(r"\bpub\b", lead)),
                    m.start()))
    return out


def _innermost(fns, pos):
    best = None
    for f in fns:
        if f[2] <= pos <= f[3] and (best is None or f[2] > best[2]):
            best = f
    return best


def _paused(src):
    """The fns of `src` that run on the virtual clock: `start_paused = true`
    tests, and helpers whose every reference lies in one of them (fixpoint)."""
    fns = _fns(src)
    names = [f[0] for f in fns]
    paused = {f for f in fns if _PAUSED.search(f[1])}
    helpers = {}
    for f in fns:
        if _TEST_ATTR.search(f[1]) or f[4] or names.count(f[0]) != 1:
            continue
        refs = [r.start() for r in re.finditer(r"\b%s\b" % re.escape(f[0]), src.code)
                if r.start() != f[5] + src.code[f[5]:].index(f[0])]
        callers = [_innermost(fns, p) for p in refs]
        callers = [c for c in callers if c is not f]   # recursion is no caller
        if callers:
            helpers[f] = callers
    grew = True
    while grew:
        grew = False
        for f, callers in helpers.items():
            if f not in paused and all(c is not None and c in paused for c in callers):
                paused.add(f)
                grew = True
    return paused


def _precise(tb, src, ban, line):
    """Whether the hit of `ban` on `line` is the paused-helper precision."""
    ctx = ban.get("context", [])
    if "start_paused" not in ctx:
        return False
    if "tokio_instant" in ctx and not re.search(
            r"tokio::time::(?:\{[^}]*\bInstant\b|Instant\b)", src.code):
        return False
    view = src.strings if ban["id"] in tb.ON_STRINGS else src.code
    at = [m.start() for m in re.finditer(ban["pattern"], view) if src.line(m.start()) == line]
    paused = _paused(src)
    return bool(at) and all(_innermost(_fns(src), p) in paused for p in at)


def check(root, base, tip, doc, head, today=None):
    """(hits, lifted paths, notes) of a strand: `hits` empty = the merge may go on."""
    paths, notes = lifted_files(root, base, tip, doc, head)
    refused, gone = missing_declared(root, base, tip, doc, head)
    return sorted(refused + hits(root, tip, paths, today=today)), paths, notes + gone
