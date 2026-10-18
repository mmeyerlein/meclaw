"""meclaw -- the precheck check `wall-clock`: no new wall-clock read in product
code without the test override that steers it.

WHY THIS EXISTS
===============
K-UHREN (16./17.10.2026): the session keeper's close cell read the wall clock
for its night window (22:00-04:00 UTC). About 24 session-close tests were red
only inside that window, and every gate until 06:00 local time with them;
KB-N2 fixed the fixtures (`meclaw_testing::quiet_keeper_night`, the params
`night_from_utc`/`night_until_utc`). The testing bans (`testing_bans.py`) read
test code only -- nothing looked at the product line that read the clock.

THE RULE
========
An ADDED line of product code that reads the wall clock directly is RED,
unless a marker `wall-clock: <name>` on the line or the comment line right
above names its test override AND `<name>` is one, in one of three forms --
the exact name, never part of a word:

    param    a top-level key of `params` in a templates/**/config.json of the
             working tree (parsed as JSON, tracked or untracked);
    env      an env read in crates/*/src (without crates/meclaw-testing/) or
             templates/**, the source itself and every `script_inline`
             included: `env::var("N")`/`env::var_os("N")`, `os.environ["N"]`/
             `os.environ.get("N")`/`os.getenv("N")`, `process.env.N`/
             `process.env["N"]`, `${N` in a config.json;
    literal  a string literal `"N"`/`'N'` of the same source on a line other
             than the read line.

A word of the read line itself never counts, and neither does a comment. Stock
lines -- unchanged since the merge base -- are not graded (the OR-TV-12 shape:
added lines only).

    Rust    crates/*/src/**/*.rs, without crates/meclaw-testing/ and without
            `#[cfg(test)]` items and files: `Type::method` (the last two path
            segments) of every `disallowed-methods` entry of
            `.github/gates/clock-bans/clippy.toml` the unwrap ratchet counts
            as `wall` (`unwrap_budget.read_bans`: the `reason` prefix before
            the colon) -- the ratchet's own list, read by its own function,
            from the tree of this script; a new wall clock is a new line there.
            The path form only: `SystemTime::elapsed(&t)` is a read here,
            `t.elapsed()` on a value is not -- without its type it is not
            told from `tokio::time::Instant::elapsed()`; the typed ratchet
            counts that form
    Python  templates/**/*.py and the `params.script_inline` of every
            templates/**/config.json (the code cell runs python3 only),
            crates/*/src/**/*.py: `time.time(`, `datetime.now(`,
            `datetime.utcnow(`
    JS      templates/**/*.js|*.mjs, crates/*/src/**/*.js|*.mjs: `Date.now(`,
            `new Date()`
    Test sources under templates/ (`*.test.js`, `test_*.py`, `*_test.py`,
    `scenarios/`) are test code, not product code.

A Rust marker makes precheck green, but the new read still raises the `wall`
column of `.github/gates/unwrap_budget.txt` ("may only go down"): the station
`unwrap-budget` then asks for a re-pin with its reason.

There is no injectable wall clock in the tree to prefer (grep 17.10.: the
crates read monotonic time through `tokio::time::Instant`, and no crate,
template or cell param offers a wall-clock source); the marker is the way, and
the rule checks that it names a real override -- a param, an env read or a
literal of the forms above, never a word of the read. Comments and string
contents are blanked before the patterns run (`testing_bans.Source`), so a
mention is no read.

FAIL-CLOSED: what the check cannot read is RED (`cannot read ...`), never a
silent green -- a failed `git diff` against the merge base or listing of the
untracked files (the helper `testing_bans.added_lines` reads both as "nothing
added", so the check reads them itself and holds the helper's answer against
them), a file of the diff it cannot read, a config.json of the diff that does
not parse, and, with a Rust file in the diff, a clock-bans list that is
missing, broken or names no `wall:` read.

A `script_inline` is one JSON line: a change anywhere in the script changes
that line. Its added lines are the added lines of the decoded script against
the script at the merge base (difflib), reported as
`<config.json>:<json line> (script_inline line <n>)`.
"""

import difflib
import importlib.util
import json
import os
import pathlib
import re
import subprocess

import testing_bans as tb

CHECK = "wall-clock"

# The ratchet's clock bans, the one list of the Rust wall-clock reads -- read
# from the tree of this script, which is the tree precheck checks.
CLOCK_BANS_REL = ".github/gates/clock-bans/clippy.toml"
_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CLOCK_BANS = os.path.join(_ROOT, *CLOCK_BANS_REL.split("/"))
# ... and the ratchet that counts them: its `read_bans` is the one reading of
# the list (re-check 9f61a3224 M1: a second reading here skipped `wall : ...`).
RATCHET_REL = ".github/gates/unwrap_budget.py"
RATCHET = os.path.join(_ROOT, *RATCHET_REL.split("/"))

READS = {
    "python": re.compile(r"\btime\.time\s*\(|\bdatetime\.(?:now|utcnow)\s*\("),
    "js": re.compile(r"\bDate\.now\s*\(|\bnew\s+Date\s*\(\s*\)"),
}
MARKER = re.compile(r"wall-clock:\s*([A-Za-z_][A-Za-z0-9_]*)")

# Env reads per language; `{n}` is the escaped name (THE RULE, form env).
_ENV_READS = {
    "rust": (r'\benv::var(?:_os)?\(\s*"{n}"\s*\)',),
    "python": (r"""\bos\.environ\[\s*(["']){n}\1\s*\]""", r"""\bos\.environ\.get\(\s*(["']){n}\1""",
               r"""\bos\.getenv\(\s*(["']){n}\1"""),
    "js": (r"\bprocess\.env\.{n}(?![\w$])", r"""\bprocess\.env\[\s*(["']){n}\1\s*\]"""),
    "json": (r"\$\{{n}(?!\w)",),
}

_CRATE_SRC = re.compile(r"^crates/(?!meclaw-testing/)[^/]+/src/")
_TEMPLATE_TEST = re.compile(r"(?:^|/)(?:test_[^/]*\.py|[^/]*_test\.py|[^/]*\.test\.m?js)$"
                            r"|^templates/(?:[^/]+/)*scenarios/")
_SUFFIX_LANG = ((".rs", "rust"), (".py", "python"), (".js", "js"), (".mjs", "js"))

HINT = ("name the test override on the line (`wall-clock: <param|env>`, a param or env the "
        "tree carries) or take the time from it")


def unit_of(path):
    """("file", lang) for a product source, ("inline", "python") for a template
    config.json, None for everything else."""
    lang = _lang(path)
    if _CRATE_SRC.match(path):
        return ("file", lang) if lang else None
    if path.startswith("templates/") and not _TEMPLATE_TEST.search(path):
        if path.endswith("/config.json"):
            return ("inline", "python")
        if lang in ("python", "js"):
            return ("file", lang)
    return None


def _lang(path):
    return next((lg for suf, lg in _SUFFIX_LANG if path.endswith(suf)), None)


def _ratchet():
    """The unwrap ratchet (`.github/gates/unwrap_budget.py`) as a module."""
    spec = importlib.util.spec_from_file_location("unwrap_budget", RATCHET)
    if spec is None or spec.loader is None:
        raise ImportError("no module at %s" % RATCHET_REL)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def rust_reads(path=None):
    """The Rust reads: `Type::method` of every `disallowed-methods` entry of the
    clock bans the unwrap ratchet counts as `wall` -- read by the ratchet's own
    `read_bans`, one reading of one list. ValueError when the ratchet cannot
    be loaded, or the list is missing, broken, refused by the ratchet or names
    no wall clock."""
    try:
        bans = _ratchet().read_bans(pathlib.Path(path or CLOCK_BANS))
    except (OSError, ImportError, SyntaxError, ValueError, KeyError, TypeError,
            AttributeError, SystemExit) as exc:
        raise ValueError(str(exc) or type(exc).__name__) from exc
    forms = set()
    for ban, column in bans.items():
        tail = str(ban).split("::")[-2:]
        if column == "wall" and len(tail) == 2 and all(tail):
            forms.add("::".join(tail))
    if not forms:
        raise ValueError("no `wall:` entry in `disallowed-methods`")
    return re.compile(r"\b(?:%s)\b" % "|".join(re.escape(f) for f in sorted(forms)))


def _added(repo, base, paths):
    """`testing_bans.added_lines`, fail-closed: None when git cannot diff
    against `base` or list the untracked files, or when the helper -- which
    reads a failed git as "nothing added" -- misses a file that diff or listing
    names. The helper stays as it is (its code is the testing bans' own)."""
    roots = sorted({p.split("/", 1)[0] for p in paths})
    text = tb._git(repo, "diff", "-M", "-U0", "--no-color", "--no-ext-diff", base, "--", *roots)
    untracked = tb._git(repo, "ls-files", "--others", "--exclude-standard", "--", *paths)
    if text is None or untracked is None:
        return None
    added = tb.added_lines(repo, base, paths)
    named = {ln[6:] for ln in text.splitlines() if ln.startswith("+++ b/")}
    named.update(untracked.splitlines())
    return added if named <= set(added) else None


def _read(repo, rel):
    try:
        with open(os.path.join(str(repo), rel), encoding="utf-8") as fh:
            return fh.read()
    except (OSError, UnicodeDecodeError):
        return None


def _script_inline(text):
    """`params.script_inline` of a config.json text, or None."""
    try:
        doc = json.loads(text)
    except (TypeError, ValueError):
        return None
    script = (doc.get("params") or {}).get("script_inline") if isinstance(doc, dict) else None
    return script if isinstance(script, str) else None


def _script_line(text):
    """The line number of the `"script_inline"` key in a config.json text."""
    for n, line in enumerate(text.splitlines(), 1):
        if '"script_inline"' in line:
            return n
    return 1


def _added_script_lines(old, new):
    """Line numbers of `new` that are not in `old` (difflib, both scripts)."""
    if old is None:
        return None
    out = set()
    matcher = difflib.SequenceMatcher(None, old.splitlines(), new.splitlines(), autojunk=False)
    for op, _i1, _i2, j1, j2 in matcher.get_opcodes():
        if op in ("insert", "replace"):
            out.update(range(j1 + 1, j2 + 1))
    return out


def _markers(src, lineno):
    """Names a `wall-clock:` marker gives on `lineno`, or on a comment-only line
    right above it."""
    lines = src._text.splitlines()
    names = []
    for start, tok in src.comments:
        n = src.line(start)
        above = n == lineno - 1 and lines[n - 1].strip() == tok.strip()
        if n == lineno or above:
            names += MARKER.findall(tok)
    return names


def _env_read(text, lang, name):
    """`text` (comments blanked) reads the env `name` in `lang`."""
    n = re.escape(name)
    return any(re.search(p.replace("{n}", n), text) for p in _ENV_READS.get(lang, ()))


def _literal_elsewhere(src, name, lineno):
    """A string literal `"name"`/`'name'` of `src` on a line other than `lineno`."""
    quoted = ('"%s"' % name, "'%s'" % name)
    return any(src._text[a:b] in quoted and src.line(a) != lineno for a, b in src._literals)


def _config_carries(text, name):
    """A config.json text carries `name`: a `params` key, `${name`, or an env
    read in its `script_inline`."""
    if _env_read(text, "json", name):
        return True
    try:
        doc = json.loads(text)
    except ValueError:
        return False
    params = doc.get("params") if isinstance(doc, dict) else None
    if not isinstance(params, dict):
        return False
    if name in params:
        return True
    script = params.get("script_inline")
    return isinstance(script, str) and _env_read(tb.Source(script, "python").strings, "python", name)


def _tree_carries(repo, name):
    """`name` is a param or an env read of the working tree (tracked or
    untracked): a templates/**/config.json, or a source of crates/*/src or
    templates/**."""
    try:
        res = subprocess.run(
            ["git", "-C", str(repo), "-c", "core.quotePath=false", "grep", "-l", "-F",
             "--untracked", "-e", name, "--", "crates", "templates"],
            capture_output=True, text=True, timeout=60)
    except (OSError, subprocess.TimeoutExpired):
        return False
    for path in res.stdout.splitlines():
        if not (_CRATE_SRC.match(path) or path.startswith("templates/")):
            continue
        text = _read(repo, path)
        if text is None:
            continue
        if path.endswith("/config.json"):
            if path.startswith("templates/") and _config_carries(text, name):
                return True
            continue
        lang = _lang(path)
        if lang and _env_read(tb.Source(text, lang).strings, lang, name):
            return True
    return False


def _exists(repo, name, src, lineno):
    """`name` is a real test override (THE RULE): a literal of `src` on a line
    other than the read line, an env read of `src`, or a param or an env read
    the working tree carries."""
    return (_literal_elsewhere(src, name, lineno)
            or _env_read(src.strings, src.lang, name)
            or _tree_carries(repo, name))


def findings_in(repo, path, text, lang, added, where=None, reads=None):
    """Findings of one source. `added`: the graded line numbers (None = every
    line). `where(lineno)` renders the location (default `path:lineno`).
    `reads`: the pattern of the reads (default `READS[lang]`)."""
    src = tb.Source(text, lang)
    rust_test = lang == "rust" and tb.cfg_test_file(repo, path)
    out = []
    for m in (reads or READS[lang]).finditer(src.code):
        lineno = src.line(m.start())
        if added is not None and lineno not in added:
            continue
        if lang == "rust" and (rust_test or src.in_cfg_test(m.start())):
            continue
        at = where(lineno) if where else "%s:%d" % (path, lineno)
        names = _markers(src, lineno)
        if not names:
            out.append(tb.Finding("RED", CHECK, "%s: `%s` reads the wall clock in product code -- %s"
                                  % (at, m.group(0).strip(), HINT)))
            continue
        missing = [n for n in names if not _exists(repo, n, src, lineno)]
        if missing:
            out.append(tb.Finding("RED", CHECK, "%s: `wall-clock: %s` names no param or env in "
                                  "crates/*/src or templates/ -- %s" % (at, missing[0], HINT)))
    return out


def scan(files, repo, base=tb._UNSET):
    """The `wall-clock` findings of the added product lines of `files`."""
    repo = str(repo)
    units = {p: unit_of(p) for p in files}
    units = {p: u for p, u in units.items() if u is not None}
    if not units:
        return []
    if base is tb._UNSET:
        base = tb.merge_base(repo)
    if not base:
        return [tb.Finding("NOTE", CHECK, "no merge base with master or main -- the added lines "
                           "cannot be told from the stock, not graded")]
    added = _added(repo, base, sorted(units))
    if added is None:
        return [tb.Finding("RED", CHECK, "cannot read the diff against the merge base %s -- the "
                           "added lines are unknown, so none of them is graded green" % base[:12])]
    out = []
    reads = dict(READS)
    if any(u == ("file", "rust") and p in added for p, u in units.items()):
        try:
            reads["rust"] = rust_reads()
        except ValueError as exc:
            out.append(tb.Finding("RED", CHECK, "cannot read the Rust wall-clock reads from %s (%s) "
                                  "-- the clock bans are their one list" % (CLOCK_BANS_REL, exc)))
    for path, (kind, lang) in sorted(units.items()):
        if path not in added or lang not in reads:
            continue                      # unchanged/deleted, or Rust without its list (RED above)
        text = _read(repo, path)
        if text is None:
            out.append(tb.Finding("RED", CHECK, "%s: cannot read the file (not UTF-8 or gone) -- "
                                  "a source of the diff the check cannot read is never green" % path))
            continue
        if kind == "file":
            out += findings_in(repo, path, text, lang, added[path], reads=reads[lang])
            continue
        try:
            json.loads(text)
        except ValueError as exc:
            out.append(tb.Finding("RED", CHECK, "%s: cannot read the config.json (%s) -- its "
                                  "script_inline cannot be graded" % (path, exc)))
            continue
        script = _script_inline(text)
        if script is None:
            continue
        old = _script_inline(tb._show(repo, base, path)) if added[path] is not None else None
        json_line = _script_line(text)
        out += findings_in(repo, path, script, lang, _added_script_lines(old, script),
                           where=lambda n, p=path, j=json_line: "%s:%d (script_inline line %d)"
                           % (p, j, n))
    return out
