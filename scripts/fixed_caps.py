#!/usr/bin/env python3
"""meclaw -- the precheck check `fixed-caps`: no new fixed window, budget or
rate value in product code without a rule that says why.

WHY THIS EXISTS
===============
The model dictates the window. A token budget, a context window, a character
cap that sizes what a model sees, or a characters-per-token rate written into
a cell as a fixed number is right for exactly one model: the catalog row
(`package.context_window`, `input_soft`, `chars_per_token`,
`tokens_per_image`, ...) is the source, and a fixed number beside it silently
outlives every model change. GH #1532 replaced a set of such numbers by reads
of the row; this check keeps new ones from coming back unseen.

WHAT IT READS
=============
Product code only, inside the repository:

    templates/**/*.json    numeric values under a window/budget/cap-like key,
                           and every `script_inline` (parsed as Python)
    templates/**/*.py      Python sources (a `src/<x>.py` that is the source
                           of the `script_inline` beside it counts once, in
                           the config)
    crates/*/src/**/*.rs   Rust sources; a `#[cfg(test)]` item (a test
                           module up to its closing brace, a test function,
                           constant or `use`) is not read, the code after it is
    examples/**/*.json     like templates

Scenario runners and tests under `templates/` (`scenarios/`, `tests/`,
`test_*.py`, `*_test.py`) are no cell code and are not read.

A FINDING is a number of at least `MIN_VALUE` bound to a name that looks like
a window, budget, token, character or context value (`NAME_RE`, or the line
says so, `LINE_HINT`) -- under a budget, token or `keep_recent` name already
from `SMALL_MIN_VALUE` on (`SMALL_NAME_RE`: a recall budget of 200 or a
`keep_recent` of 40 sizes a window too) --, names of time, size-in-bytes, geometry and ports aside
-- or a RATE: a small number (1 to 8) bound to a characters-per-token name, or
dividing/multiplying a length in a token computation. A generic limit name
(`limit`, `max`, `cap`, `_lines`, `length`: a row, scan or part limit as often
as a window) counts only where the line says it is about a model's input
(`LINE_HINT`). A number is a literal, a constant expression (`32 * 1024`,
`1 << 15`) or a parsed literal (`int("32000")`); in JSON a value counts under a
window-like key or under a window-like parent key (`{"context": {"size": ..}}`,
named `context.size`). A word of time or ports beside the number (`timeout`,
`Duration`, `port`, ...) makes it no window only in the code of its own
statement: a Python comment and the other statements of a Rust line do not
count.

THE VERDICT
===========
A finding is justified by a RULE (`fixed_caps_rules.json`) or held by the
BASELINE (`fixed_caps_baseline.json`, the stock the check was introduced
over, rewritten with `--write-baseline`). Anything else is RED: the line,
the name, the value, and what to do -- read the value from the catalog row,
or add a rule with its reason.

A rule is a structure condition, not a list of paths: a name pattern, the
values it allows, and what the source around the value has to show --

    inside_block   the value stands inside the marked block
                   `# >>> <name>` ... `# <<< <name>` (`//` in Rust);
    block_defines  that block defines this function;
    calls          the source calls this function outside its definition;
    reads          the cell reads this key: a string literal used as a call
                   argument, a subscript or an attribute name in the script,
                   a key of the config JSON, a string literal or a field
                   access in Rust code.

A rule names its `values` or, for a map a render script writes from the model
catalog (`input_soft_fallback` {model_id: input_soft}), its `catalog` field
instead (GH #1544): the hit `<key>.<model_id> = v` is justified exactly while
the catalog row of `<model_id>` holds `v` in that field. A current render is
green; a stale render, a model without a row and a missing catalog are not
justified. The catalog is the file `--catalog` names, else the file the rules
file names as `catalog_file` (repository-relative: a repository whose catalog
comes from a release asset carries a snapshot of it beside its render, GH #1548),
else `CATALOG_FILE` of the tree, else -- only in a tree without one -- the file
`FIXED_CAPS_CATALOG` names. A diff at the catalog the tree reads grades the whole
scope: a new row value can stale a render.

Every rule carries its reason in English. The baseline is a count per
fingerprint `<path>|<name>|<value>` (line numbers stay out, so moving code is
no finding); one occurrence more than the baseline holds is RED.

In the precheck the check grades the files of the diff. A diff that touches
this module, the rules, the baseline or the catalog the tree reads grades the
whole scope, and only then a baseline entry the tree no longer carries is a
NOTE (`--write-baseline`).

A FROZEN COPY is stock: a file that `git diff -C -C` against the merge base
finds as a 100 % copy (`C100`) of a file of the base -- a template frozen as
`templates/<name>@<version>/` -- is not graded, and its baseline entries are
neither stale nor new (`wall_clock._exact_copies`, the same reading as the
check `wall-clock`, OR-58-116). A copy with any changed line is a new file.

THE RATCHET: the baseline only shrinks. A diff that touches the baseline is
held against the baseline of the merge base (`git show <base>:<baseline>`): an
entry the base does not hold, or a count above the base's, is RED with the
entry -- a new value is let through by a rule with its reason, never by a
rewrite of the baseline (`--write-baseline` in the same diff as the value).
Entries of a frozen copy are no growth. Where the merge base carries no
baseline yet (the check's own introduction), the baseline of the tree is the
stock and has no ratchet; where there is no merge base at all, the precheck
notes that the ratchet cannot run. `--repo` grades the whole tree the same way
(copies and ratchet included).

USAGE
=====
    fixed_caps.py [--repo DIR]                 grade the whole scope
    fixed_caps.py [--repo DIR] --list          every finding with its verdict
    fixed_caps.py [--repo DIR] [--catalog FILE] ...
                                               read the catalog rules against FILE
    fixed_caps.py [--repo DIR] --write-baseline
                                               write the baseline from the tree
                                               (every finding no rule justifies)

Exit 1 when a RED finding was printed, 0 otherwise.
"""

import argparse
import ast
import collections
import json
import os
import posixpath
import re
import sys
import tokenize

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import testing_bans as tb  # the git helpers and the merge base
import wall_clock  # the frozen-copy reading (OR-58-116)

CHECK = "fixed-caps"
RULES_FILE = "scripts/fixed_caps_rules.json"
BASELINE_FILE = "scripts/fixed_caps_baseline.json"
# The model catalog a `catalog` rule reads (GH #1544); `--catalog` names
# another file, the environment one for a tree that carries none.
CATALOG_FILE = "templates/llm-registry/store/seed/models.jsonl"
CATALOG_ENV = "FIXED_CAPS_CATALOG"
# The files whose change makes the precheck grade the whole scope.
SELF_FILES = ("scripts/fixed_caps.py", RULES_FILE, BASELINE_FILE, CATALOG_FILE)

Finding = collections.namedtuple("Finding", "level check message")
# kind: "NUM" or "RATE"; path: repo-relative; line: the line in the file (for a
# script: the line of its `script_inline` key); script_line: the line inside
# the script, else None; unit: the parsed source the value stands in.
Hit = collections.namedtuple("Hit", "kind path line script_line name value text unit")

# --- what counts as a window, budget or rate value ---------------------------

# A name that is about a model's window. The generic limit names (`limit`,
# `max`, `cap`, `_lines`, `length`) are not in it: they count only where the
# line carries `LINE_HINT`. The model words (`input`, `prompt`, `output`,
# `completion`) count as a whole part of the name: `MAX_INPUT`, `max_output`.
NAME_RE = re.compile(
    r"token|budget|window|chars|char_|context|(^|_)ctx($|_)|keep_recent|compress_at|"
    r"(^|_)(input|prompt|output|completion)($|_)|"
    r"rebuild_to|(^|_)soft($|_)|(^|_)hard($|_)|SOFT|HARD|section_window|excerpt|clip|trunc|preview|snippet|"
    r"horizon|room|headroom|fill", re.I)
TIME_RE = re.compile(
    r"(^|_)(ms|s|sec|secs|seconds|timeout|ttl|interval|delay|port|year|days?|hours?|minutes?|mins?|millis|epoch|"
    r"wait|sleep|retry|retries|backoff|ts|deadline|age|stale|hz|rate_hz|sample_rate|width|height|px|w|h|x|y|dpi|"
    r"fps|bitrate|freq|khz|mb|kb|gb|bytes?|memory|mem)($|_)|TIMEOUT|_MS\b|_S\b|SECS|MILLIS|WIDTH|HEIGHT|BYTES|"
    r"_MB\b|_KB\b|PORT|DPI|SAMPLE", re.I)
LINE_HINT = re.compile(r"token|budget|window|chars|\bcap\b|context|input_soft|input_hard|excerpt|clip|trunc|"
                       r"max_tokens", re.I)
RATE_NAME = re.compile(r"chars?_per_token|tokens?_per_char|per_token|token_rate|char_rate", re.I)
# A word, bounded by non-letters: `port` is no part of `import` or `report`,
# `timeout` is one of `timeout_s`.
_WORD = r"(?<![A-Za-z])(?:%s)(?![A-Za-z])"
# Python code (its comment cut off) that is about time, sizes in bytes or
# ports, not about a model.
PY_NOT_A_WINDOW = re.compile(
    r"(?<![A-Za-z])time\.|" + _WORD % r"sleep|timeouts?|ms|monotonic|datetime|timedelta|ports?|deadlines?|durations?"
    + r"|_ms\b|ms_|1e3|_us\b|\* ?1000\b|3600|60000000|1440|_kb\b|\* ?1024\b", re.I)
# The same for the Rust statement a number stands in.
RS_NOT_A_WINDOW = re.compile(
    _WORD % r"Duration|Instant|timeouts?|sleep|ports?|capacity" + r"|from_millis|from_secs|_ms\b|error_status|milli_")
MIN_VALUE = 500
# A budget, token or `keep_recent` name sizes a window already with a small
# number (a recall budget of 200, the last 40 turns kept); the other names keep
# the floor of `MIN_VALUE`, under it they are lengths of ids and tags. The
# random-id helpers of `secrets` (`token_hex(8)`) name no model token.
SMALL_NAME_RE = re.compile(r"budget|keep_recent|token(?!_(?:hex|urlsafe|bytes)(?![A-Za-z]))", re.I)
SMALL_MIN_VALUE = 8


def floor_of(name):
    """The smallest number that counts under `name`."""
    return SMALL_MIN_VALUE if name and SMALL_NAME_RE.search(name) else MIN_VALUE

SKIP_DIRS = {"target", ".git", "node_modules", "__pycache__"}
_IN_SCOPE = re.compile(r"^(templates/.+\.(json|py)|crates/[^/]+/src/.+\.rs|examples/.+\.json)$")
# Scenario runners and tests under templates/ are no cell code: the test and
# scenario paths of the check `wall-clock`, and `tests/` directories.
_TEMPLATE_TESTS = re.compile(r"^templates/(?:[^/]+/)*tests/")
_BLOCK_OPEN = re.compile(r"^\s*(?:#|//)\s*>>>\s*(.+?)\s*(?:\(.*)?$")
_BLOCK_CLOSE = re.compile(r"^\s*(?:#|//)\s*<<<\s*(.+?)\s*$")


def in_scope(path):
    """A repository-relative path this check reads."""
    name = path.rsplit("/", 1)[-1]
    if path.startswith("templates/") and (wall_clock._TEMPLATE_TEST.search(path) or _TEMPLATE_TESTS.match(path)):
        return False
    return bool(_IN_SCOPE.match(path)) and not name.startswith(".env") and not name.endswith(".env")


def _num(v):
    return isinstance(v, (int, float)) and not isinstance(v, bool)


# --- the structure a rule can ask for ------------------------------------------

class Unit:
    """One parsed source: its marked blocks, the functions it defines and calls,
    and the keys it reads. A script and the config it stands in share one
    `reads` set for JSON hits (the config's keys and the script's reads)."""

    def __init__(self, lines):
        self.blocks = _blocks(lines)
        self.defines = {}           # function name -> (first line, last line)
        self.calls = []             # (function name, line)
        self.reads = set()

    def block_at(self, line):
        """The names of the marked blocks `line` stands in."""
        return {name for name, a, b in self.blocks if a < line < b}

    def defines_in_block(self, fn, block):
        span = self.defines.get(fn)
        return bool(span) and any(name == block and a < span[0] and span[1] < b for name, a, b in self.blocks)

    def calls_outside_definition(self, fn):
        span = self.defines.get(fn)
        return any(name == fn and not (span and span[0] <= ln <= span[1]) for name, ln in self.calls)


def _blocks(lines):
    out, open_at = [], {}
    for i, line in enumerate(lines, 1):
        m = _BLOCK_OPEN.match(line)
        if m:
            open_at[m.group(1)] = i
            continue
        m = _BLOCK_CLOSE.match(line)
        if m and m.group(1) in open_at:
            out.append((m.group(1), open_at.pop(m.group(1)), i))
    return out


def _py_unit(tree, lines):
    unit = Unit(lines)
    if tree is None:
        return unit
    for n in ast.walk(tree):
        if isinstance(n, (ast.FunctionDef, ast.AsyncFunctionDef)):
            unit.defines.setdefault(n.name, (n.lineno, getattr(n, "end_lineno", n.lineno)))
        elif isinstance(n, ast.Call):
            f = n.func
            name = f.id if isinstance(f, ast.Name) else f.attr if isinstance(f, ast.Attribute) else None
            if name:
                unit.calls.append((name, n.lineno))
            for a in list(n.args) + [k.value for k in n.keywords]:
                if isinstance(a, ast.Constant) and isinstance(a.value, str):
                    unit.reads.add(a.value)
        elif isinstance(n, ast.Subscript):
            s = n.slice
            if isinstance(s, ast.Constant) and isinstance(s.value, str):
                unit.reads.add(s.value)
        elif isinstance(n, ast.Attribute):
            unit.reads.add(n.attr)
    return unit


# --- Python (files and script_inline) ------------------------------------------

def _const_val(n):
    if isinstance(n, ast.Constant) and _num(n.value):
        return n.value
    if isinstance(n, ast.UnaryOp) and isinstance(n.op, ast.USub):
        v = _const_val(n.operand)
        return -v if v is not None else None
    if isinstance(n, ast.BinOp) and isinstance(n.op, (ast.Mult, ast.Add, ast.Pow, ast.LShift)):
        a, b = _const_val(n.left), _const_val(n.right)
        if a is not None and b is not None:
            try:
                if isinstance(n.op, ast.Mult):
                    return a * b
                if isinstance(n.op, ast.Add):
                    return a + b
                if isinstance(n.op, ast.LShift):
                    return a << b if isinstance(a, int) and isinstance(b, int) and 0 <= b < 64 else None
                return a ** b if abs(b) < 64 else None
            except (OverflowError, ValueError):
                return None
    return None


_DECIMAL = re.compile(r"^\s*[+-]?\d[\d_]*(?:\.\d+)?\s*$")


def _parsed_val(n):
    """The number of `int("32000")` / `float("0.5")`: a literal written as a string."""
    if not (isinstance(n, ast.Call) and isinstance(n.func, ast.Name) and n.func.id in ("int", "float")
            and len(n.args) == 1 and not n.keywords):
        return None
    a = n.args[0]
    if not (isinstance(a, ast.Constant) and isinstance(a.value, str) and _DECIMAL.match(a.value)):
        return None                       # `float("inf")` and the like are no fixed size
    try:
        return int(a.value) if n.func.id == "int" else float(a.value)
    except ValueError:
        return None


def _comment_cols(src):
    """line -> the column its comment starts at (Python), for cutting it off."""
    out = {}
    try:
        for tok in tokenize.generate_tokens(iter(src.splitlines(True)).__next__):
            if tok.type == tokenize.COMMENT:
                out[tok.start[0]] = tok.start[1]
    except (tokenize.TokenError, IndentationError, SyntaxError):
        pass
    return out


def _node_name(n):
    if isinstance(n, ast.Name):
        return n.id
    if isinstance(n, ast.Attribute):
        return n.attr
    if isinstance(n, ast.Subscript):
        s = n.slice
        if isinstance(s, ast.Constant) and isinstance(s.value, str):
            return s.value
        return _node_name(n.value)
    if isinstance(n, ast.Constant) and isinstance(n.value, str):
        return n.value
    if isinstance(n, ast.Call):
        f = n.func
        if isinstance(f, ast.Attribute) and f.attr == "get" and n.args and isinstance(n.args[0], ast.Constant) \
                and isinstance(n.args[0].value, str):
            return n.args[0].value
        if isinstance(f, (ast.Name, ast.Attribute)):
            # int(x.get("k") or 4000), _int("k", 4000), len(x)
            for a in n.args:
                nm = _node_name(a)
                if nm:
                    return nm
            return _node_name(f)
    if isinstance(n, ast.BoolOp):
        for v in n.values:
            nm = _node_name(v)
            if nm:
                return nm
    return None


def py_hits(src, path, line=None):
    """The hits of a Python source. `line`: the line of the `script_inline` key
    when the source is a script inside a config, else None."""
    lines = src.split("\n")
    try:
        tree = ast.parse(src)
    except SyntaxError:
        tree = None
    unit = _py_unit(tree, lines)
    if tree is None:
        return [], unit
    parents, funcs, defaults = {}, {}, {}
    for p in ast.walk(tree):
        for c in ast.iter_child_nodes(p):
            parents[c] = p
    for f in ast.walk(tree):
        if isinstance(f, (ast.FunctionDef, ast.AsyncFunctionDef)):
            a = f.args
            pos = a.posonlyargs + a.args
            for arg, d in zip(pos[len(pos) - len(a.defaults):], a.defaults):
                defaults[id(d)] = arg.arg
            for arg, d in zip(a.kwonlyargs, a.kw_defaults):
                if d is not None:
                    defaults[id(d)] = arg.arg
            for n in ast.walk(f):
                funcs.setdefault(id(n), f.name)

    def ctx_name(n):
        """The name a number is bound to: target, keyword, dict key, `.get` key,
        default, the other side of a comparison, a slice."""
        if id(n) in defaults:
            return defaults[id(n)]
        cur, p, hops = n, parents.get(n), 0
        while p is not None and hops < 6:
            if isinstance(p, ast.keyword) and p.arg:
                return p.arg
            if isinstance(p, ast.Dict):
                for k, v in zip(p.keys, p.values):
                    if v is cur and isinstance(k, ast.Constant) and isinstance(k.value, str):
                        return k.value
            if isinstance(p, (ast.Assign, ast.AnnAssign, ast.AugAssign)):
                t = p.targets[0] if isinstance(p, ast.Assign) else p.target
                return _node_name(t)
            if isinstance(p, (ast.Call, ast.BoolOp)):
                nm = _node_name(p)
                if nm:
                    return nm
            if isinstance(p, ast.Compare):
                for side in [p.left] + p.comparators:
                    if side is not cur:
                        nm = _node_name(side)
                        if nm:
                            return nm
            if isinstance(p, ast.Slice):
                sub = parents.get(p)
                if isinstance(sub, ast.Subscript):
                    return "slice:" + (_node_name(sub.value) or "?")
            if isinstance(p, ast.Return):
                return "return:" + funcs.get(id(p), "?")
            if isinstance(p, ast.stmt):
                return None
            cur, p = p, parents.get(p)
            hops += 1
        return None

    comments = _comment_cols(src)
    out, seen = [], set()
    for n in ast.walk(tree):
        if isinstance(n, ast.Constant) and _num(n.value):
            p = parents.get(n)
            top = p if isinstance(p, (ast.BinOp, ast.UnaryOp)) and _const_val(p) is not None else n
            v = _const_val(top)
        elif _parsed_val(n) is not None:
            p, top, v = parents.get(n), n, _parsed_val(n)
        else:
            continue
        if v is None:
            continue
        ln = n.lineno
        text = lines[ln - 1] if ln - 1 < len(lines) else ""
        code = text[:comments[ln]] if ln in comments else text
        name = ctx_name(top) or ""
        key = (ln, name, v)
        if key in seen:
            continue
        is_rate = False
        if RATE_NAME.search(name) and 1 <= abs(v) <= 8:
            is_rate = True
        elif isinstance(p, ast.BinOp) and isinstance(p.op, (ast.Div, ast.FloorDiv, ast.Mult)) and 2 <= v <= 6 \
                and re.search(r"token", text, re.I) and re.search(r"len\(|chars|char", text, re.I):
            is_rate = True
        kind = "RATE" if is_rate else None
        if kind is None:
            if abs(v) < floor_of(name) or (name and TIME_RE.search(name)):
                continue
            if not ((name and NAME_RE.search(name)) or LINE_HINT.search(text)):
                continue
            if PY_NOT_A_WINDOW.search(code) and not LINE_HINT.search(name or ""):
                continue
            if 100 <= v <= 599 and re.search(r"status|http", text, re.I):
                continue
            kind = "NUM"
        seen.add(key)
        if line is None:
            out.append(Hit(kind, path, ln, None, name or "(expression)", v, text.strip(), unit))
        else:
            out.append(Hit(kind, path, line, ln, name or "(expression)", v, text.strip(), unit))
    return out, unit


# --- JSON ----------------------------------------------------------------------

def json_hits(text, path):
    """The hits of a config JSON: numeric values under a window-like key, and
    the hits of every `script_inline` inside it."""
    try:
        doc = json.loads(text)
    except ValueError:
        return []
    lines = text.split("\n")
    out, used, script_lines = [], {}, iter(i for i, ln in enumerate(lines, 1) if '"script_inline"' in ln)
    keys, scripts = set(), []

    def line_of(key, val):
        pat = re.compile(r'"%s"\s*:\s*%s(?![\d.])' % (re.escape(key), re.escape(json.dumps(val))))
        for i in range(used.get((key, val), 0), len(lines)):
            if pat.search(lines[i]):
                used[(key, val)] = i + 1
                return i + 1
        return 0

    def walk(o, trail):
        if isinstance(o, dict):
            for k, v in o.items():
                keys.add(k)
                if k == "script_inline" and isinstance(v, str):
                    scripts.append((v, next(script_lines, 0)))
                elif _num(v):
                    ctx = ".".join(trail[-3:] + [k])
                    keys_up = [t for t in trail if not t.isdigit()]
                    parent = keys_up[-1] if keys_up else ""
                    if RATE_NAME.search(k) and 1 <= v <= 8:
                        out.append(Hit("RATE", path, line_of(k, v), None, k, v, ctx, None))
                    elif TIME_RE.search(k):
                        pass
                    elif NAME_RE.search(k):
                        if abs(v) >= floor_of(k):
                            out.append(Hit("NUM", path, line_of(k, v), None, k, v, ctx, None))
                    elif parent and NAME_RE.search(parent) and not TIME_RE.search(parent):
                        # a value under a window-like key: {"context": {"size": 32000}}; the
                        # `default` of a declared param is the param's value
                        name = parent if k == "default" else "%s.%s" % (parent, k)
                        if abs(v) >= floor_of(name):
                            out.append(Hit("NUM", path, line_of(k, v), None, name, v, ctx, None))
                else:
                    walk(v, trail + [k])
        elif isinstance(o, list):
            for i, v in enumerate(o):
                walk(v, trail + [str(i)])

    walk(doc, [])
    cell = Unit([])
    cell.reads |= keys
    for src, at in scripts:
        hits, unit = py_hits(src, path, line=at)
        out.extend(hits)
        cell.reads |= unit.reads
    # a JSON value stands in the cell: its keys and what its scripts read
    return [h._replace(unit=cell) if h.unit is None else h for h in out]


# --- Rust ----------------------------------------------------------------------

RS_NUM = re.compile(r"(?<![\w.])(\d[\d_]*(?:\.\d+)?)(?:_?(?:u8|u16|u32|u64|usize|i32|i64|f32|f64))?(?![\w.])")
RS_CONST = re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:const|static)\s+([A-Z0-9_]+)\s*:\s*[^=]+=\s*(.+?);")
RS_STRING = re.compile(r'"(?:[^"\\]|\\.)*"')
RS_FN = re.compile(r"\bfn\s+([a-z_][a-z0-9_]*)")
RS_CHAR = re.compile(r"'(?:[^'\\]|\\.)'")
RS_CONST_HEAD = re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:const|static)\s+[A-Z0-9_]+\s*:")
RS_TEST_ATTR = re.compile(r"^\s*#\[cfg\(test\)\]\s*")
RS_JOIN_MAX = 8


def _rs_clean(line):
    """The code of a Rust line: string and char literals blanked, the comment cut."""
    return re.sub(r"//.*$", "", RS_CHAR.sub("' '", RS_STRING.sub('""', line)))


def _rs_logical(lines):
    """(line number, text) per line; a `const`/`static` written over several
    lines is joined up to its `;` (at most `RS_JOIN_MAX` lines) and stands at
    its first line."""
    out, i = [], 0
    while i < len(lines):
        joined, j = lines[i], i
        if RS_CONST_HEAD.match(joined):
            while ";" not in _rs_clean(joined) and j + 1 < len(lines) and j - i < RS_JOIN_MAX:
                j += 1
                joined = joined.rstrip() + " " + lines[j].strip()
            if ";" not in _rs_clean(joined):
                joined, j = lines[i], i
        out.append((i + 1, joined))
        i = j + 1
    return out


def _rs_item_closed(clean, opened, depth):
    """A test item ends at the brace that closes its body, or -- without a
    body (`mod x;`, `use ..;`, `const ..;`) -- at its `;`."""
    return (opened and depth <= 0) or (not opened and clean.rstrip().endswith(";"))


def rs_hits(text, path):
    lines = text.split("\n")
    unit = Unit(lines)
    out = []
    # a `#[cfg(test)]` item is blind up to its end, the code after it is read
    pending, skipping, opened, depth = False, False, False, 0
    for i, line in _rs_logical(lines):
        clean = _rs_clean(line)
        if skipping:
            depth += clean.count("{") - clean.count("}")
            opened = opened or "{" in clean
            skipping = not _rs_item_closed(clean, opened, depth)
            continue
        m = RS_TEST_ATTR.match(line)
        if m:
            pending = True
            line = line[m.end():]
            clean = _rs_clean(line)
        s = line.strip()
        if pending:
            if not s or s.startswith("#[") or s.startswith("//"):
                continue                  # more attributes or comments before the item
            pending = False
            depth = clean.count("{") - clean.count("}")
            opened = "{" in clean
            skipping = not _rs_item_closed(clean, opened, depth)
            continue
        if s.startswith("//") or s.startswith("*") or s.startswith("/*"):
            continue
        code = re.sub(r"//.*$", "", line)
        for lit in RS_STRING.findall(code):
            unit.reads.add(lit[1:-1])
        unit.reads.update(re.findall(r"\.([a-z_][a-z0-9_]*)\b(?!\s*\()", code))
        for fn in RS_FN.findall(code):
            unit.defines.setdefault(fn, (i, i))
        unit.calls.extend((fn, i) for fn in re.findall(r"\b([a-z_][a-z0-9_]*)\s*\(", code))
        bare = RS_STRING.sub('""', code)
        nums = [(m.group(1), m.start()) for m in RS_NUM.finditer(bare)]
        if not nums:
            continue
        m = RS_CONST.match(code)
        name = m.group(1) if m else None
        if (name and RATE_NAME.search(name)) or (re.search(r"token", bare, re.I) and re.search(r"len\(\)|chars", bare)
                                                 and re.search(r"[/*]\s*\d(\.\d+)?\b", bare)):
            small = [x for x, _ in nums if 1 <= float(x.replace("_", "")) <= 8]
            if small and (re.search(r"[/*]\s*(%s)\b" % "|".join(re.escape(x) for x in small), bare)
                          or (name and RATE_NAME.search(name))):
                v = float(small[0].replace("_", ""))
                out.append(Hit("RATE", path, i, None, name or "(expression)",
                               int(v) if v.is_integer() else v, s, unit))
                continue
        for x, pos in nums:
            try:
                v = float(x.replace("_", ""))
            except ValueError:
                continue
            nm = name
            if not nm:
                ids = re.findall(r"([A-Za-z_][A-Za-z0-9_]*)\s*(?:[:=]|\(|,|<|>|\.unwrap_or\(|\bmin\(|\bmax\()",
                                 bare[:pos])
                nm = ids[-1] if ids else ""
            if v < floor_of(nm):
                continue
            v = int(v) if v.is_integer() else v
            # a word of time or ports counts in the statement of the number only
            stmt = bare[bare.rfind(";", 0, pos) + 1:(bare.find(";", pos) + 1 or len(bare) + 1) - 1]
            if TIME_RE.search(nm or "") or RS_NOT_A_WINDOW.search(stmt):
                continue
            if not ((nm and NAME_RE.search(nm)) or LINE_HINT.search(bare)):
                continue
            out.append(Hit("NUM", path, i, None, nm or "(literal)", v, s, unit))
    return out


# --- the tree --------------------------------------------------------------------

def _read(path):
    try:
        with open(path, encoding="utf-8") as fh:
            return fh.read()
    except (OSError, UnicodeDecodeError):
        return None


def scope_files(repo):
    """Every path of the scope in the tree at `repo`, repository-relative."""
    out = []
    for top in ("templates", "crates", "examples"):
        root = os.path.join(repo, top)
        for d, ds, fs in os.walk(root):
            ds[:] = sorted(x for x in ds if x not in SKIP_DIRS)
            for f in sorted(fs):
                rel = os.path.relpath(os.path.join(d, f), repo).replace(os.sep, "/")
                if in_scope(rel):
                    out.append(rel)
    return out


def _is_script_source(repo, path):
    """`templates/<cell>/src/<x>.py` is the source of the `script_inline` beside
    it (`scripts/inline_scripts.py`): it counts once, in the config."""
    parts = path.split("/")
    if len(parts) < 3 or parts[-2] != "src" or not path.endswith(".py"):
        return False
    cfg = os.path.join(repo, *parts[:-2], "config.json")
    try:
        with open(cfg, encoding="utf-8") as fh:
            params = json.load(fh).get("params") or {}
    except (OSError, ValueError, AttributeError):
        return False
    return isinstance(params, dict) and isinstance(params.get("script_inline"), str)


def hits_of(repo, path):
    """The hits of one file of the scope; none for a file the tree does not have."""
    if not in_scope(path) or _is_script_source(repo, path):
        return []
    text = _read(os.path.join(repo, path))
    if text is None:
        return []
    if path.endswith(".json"):
        return json_hits(text, path)
    if path.endswith(".py"):
        return py_hits(text, path)[0]
    return rs_hits(text, path)


def fingerprint(hit):
    """`<path>|<name>|<value>`, `#script` after the path of a hit inside a script."""
    where = hit.path + ("#script" if hit.script_line is not None else "")
    return "%s|%s|%s" % (where, hit.name, hit.value)


def location(hit):
    if hit.script_line is not None:
        return "%s:%d (script line %d)" % (hit.path, hit.line, hit.script_line)
    return "%s:%d" % (hit.path, hit.line)


# --- rules and baseline ----------------------------------------------------------

# `catalog_file` names no usable file: a catalog rule then justifies nothing,
# and `grade` reports the rules file (never a fallback to another catalog).
INVALID_CATALOG = "\0invalid"


def catalog_file(repo):
    """The catalog the rules file names (`catalog_file`): the normalized
    repository-relative path; None when the rules file names none;
    `INVALID_CATALOG` when the value is no non-empty string or leaves the
    repository (absolute, `..`) -- GH #1548."""
    try:
        doc = json.loads(_read(os.path.join(repo, RULES_FILE)) or "{}")
    except ValueError:
        return None                       # the rules file itself is reported by `grade`
    if not isinstance(doc, dict) or "catalog_file" not in doc:
        return None
    named = doc["catalog_file"]
    if not isinstance(named, str) or not named.strip():
        return INVALID_CATALOG
    norm = posixpath.normpath(named.strip())
    if posixpath.isabs(norm) or norm in (".", "..") or norm.startswith("../"):
        return INVALID_CATALOG
    return norm


def load_rules(repo):
    """The rules of the tree; none when it carries no rules file (every hit then
    needs the baseline)."""
    text = _read(os.path.join(repo, RULES_FILE))
    if text is None:
        return []
    rules = json.loads(text)["rules"]
    for r in rules:
        r["_names"] = re.compile(r["names"])
    return rules


def load_catalog(repo, path=None):
    """model_id -> row of the catalog (`models.jsonl`, its schema line aside):
    `path`, else the rules file's `catalog_file`, else `CATALOG_FILE` of the
    tree, else -- a tree without one -- the file `FIXED_CAPS_CATALOG` names.
    Empty when no file is found: a catalog rule then justifies nothing."""
    own = os.path.join(repo, CATALOG_FILE)
    named = catalog_file(repo)
    if not path:
        if named == INVALID_CATALOG:
            return {}
        if named:
            path = os.path.join(repo, named)
        else:
            path = own if os.path.isfile(own) else os.environ.get(CATALOG_ENV)
    text = _read(path) if path else None
    rows = {}
    for line in (text or "").splitlines():
        try:
            row = json.loads(line)
        except ValueError:
            continue
        if isinstance(row, dict) and isinstance(row.get("model_id"), str):
            rows[row["model_id"]] = row
    return rows


def _rendered(hit, field, catalog):
    """`hit` is `<key>.<model_id> = v` and the catalog row of `<model_id>`
    holds `v` in `field`."""
    _, sep, model = str(hit.name).partition(".")
    row = (catalog or {}).get(model) if sep else None
    v = row.get(field) if row else None
    return _num(v) and v == hit.value


def parse_baseline(text):
    """fingerprint -> count of a baseline text."""
    return {"%s|%s|%s" % (e["path"], e["name"], e["value"]): int(e["count"])
            for e in json.loads(text)["entries"]}


def load_baseline(repo):
    """fingerprint -> count; empty when the tree carries no baseline."""
    text = _read(os.path.join(repo, BASELINE_FILE))
    if text is None:
        return {}
    return parse_baseline(text)


def fp_path(fp):
    """The file of a fingerprint (`#script` dropped)."""
    path = fp.rsplit("|", 2)[0]
    return path[:-len("#script")] if path.endswith("#script") else path


def rule_for(hit, rules, catalog=None):
    """The first rule that justifies `hit`, else None; `catalog` (model_id ->
    row) answers the catalog rules."""
    for r in rules:
        if not r["_names"].search(str(hit.name)):
            continue
        if "catalog" in r:
            if not _rendered(hit, r["catalog"], catalog):
                continue                  # stale, no row or no catalog: not justified
        elif hit.value not in r.get("values", ()):
            continue                      # a rule without values justifies no value
        if "kind" in r and hit.kind != r["kind"]:
            continue
        unit = hit.unit or Unit([])
        at = hit.script_line if hit.script_line is not None else hit.line
        block = r.get("inside_block")
        if block and block not in unit.block_at(at):
            continue
        if r.get("block_defines") and not (block and unit.defines_in_block(r["block_defines"], block)):
            continue
        if r.get("calls") and not unit.calls_outside_definition(r["calls"]):
            continue
        if r.get("reads") and r["reads"] not in unit.reads:
            continue
        return r
    return None


def grade(repo, paths, whole=False, copies=frozenset(), catalog=None):
    """The findings over `paths` (the whole scope with `whole`); a frozen copy
    of `copies` is stock and not graded; `catalog` names the catalog file."""
    try:
        rules, base = load_rules(repo), load_baseline(repo)
    except (ValueError, KeyError, TypeError, AttributeError, re.error) as e:
        return [Finding("RED", CHECK, "%s or %s is unreadable (%s: %s); nothing is graded until it reads"
                                      % (RULES_FILE, BASELINE_FILE, type(e).__name__, e))]
    rows = load_catalog(repo, catalog)
    bad = [] if catalog or catalog_file(repo) != INVALID_CATALOG else [Finding(
        "RED", CHECK, "%s names an invalid catalog_file (a non-empty repository-relative path, never absolute or "
                      "outside the repository); the catalog rules justify nothing" % RULES_FILE)]
    by_fp = collections.OrderedDict()
    for path in paths:
        if path in copies:
            continue
        for hit in hits_of(repo, path):
            if rule_for(hit, rules, rows) is None:
                by_fp.setdefault(fingerprint(hit), []).append(hit)
    out = list(bad)
    for fp, hits in by_fp.items():
        held = base.get(fp, 0)
        if len(hits) <= held:
            continue
        for hit in hits:
            out.append(Finding(
                "RED", CHECK,
                "%s: %s = %s is a fixed %s value no rule justifies (%d here, the baseline holds %d); "
                "read it from the model's catalog row (package/limits), or add a rule with its reason "
                "to %s" % (location(hit), hit.name, hit.value,
                           "rate" if hit.kind == "RATE" else "window/budget", len(hits), held, RULES_FILE)))
    if whole:
        for fp in sorted(set(base) - set(by_fp)):
            if fp_path(fp) in copies:
                continue                  # a frozen copy is stock, not graded
            out.append(Finding("NOTE", CHECK, "baseline entry %s is no longer in the tree; rewrite the baseline "
                                              "(scripts/fixed_caps.py --write-baseline)" % fp))
    return out


def copies_of(repo, base, paths):
    """The frozen copies among `paths`: 100 % copies of a file of the merge
    base (`wall_clock._exact_copies`, OR-58-116). Empty without a base or when
    git cannot answer -- then every file is graded."""
    if not base or not paths:
        return frozenset()
    return frozenset(wall_clock._exact_copies(repo, base, sorted(paths)))


def ratchet(repo, base):
    """RED for every entry the baseline of the tree holds above the baseline
    of the merge base -- the baseline only shrinks. No ratchet where the base
    carries no baseline (the check's introduction); RED where git cannot say."""
    unread = [Finding("RED", CHECK, "cannot read %s at the merge base %s -- the baseline cannot be held against "
                                    "its base, so its growth is not graded green" % (BASELINE_FILE, base[:12]))]
    listed = tb._git(repo, "ls-tree", "--name-only", base, "--", BASELINE_FILE)
    if listed is None:
        return unread
    if not listed.strip():
        return []
    text = tb._show(repo, base, BASELINE_FILE)
    try:
        old = parse_baseline(text) if text is not None else None
    except (ValueError, KeyError, TypeError):
        old = None
    if old is None:
        return unread
    try:
        mine = load_baseline(repo)
    except (ValueError, KeyError, TypeError) as e:
        return [Finding("RED", CHECK, "%s is unreadable (%s: %s); its growth is not graded until it reads"
                                      % (BASELINE_FILE, type(e).__name__, e))]
    grown = [(fp, old.get(fp, 0), n) for fp, n in sorted(mine.items()) if n > old.get(fp, 0)]
    copies = copies_of(repo, base, {fp_path(fp) for fp, _, _ in grown})
    return [Finding("RED", CHECK, "the baseline grew against the merge base %s: %s (the base holds %d, the tree %d); "
                                  "the baseline only shrinks -- read the value from the model's catalog row, or "
                                  "add a rule with its reason to %s" % (base[:12], fp, held, n, RULES_FILE))
            for fp, held, n in grown if fp_path(fp) not in copies]


def grade_tree(repo, base=tb._UNSET, catalog=None):
    """The whole scope of the tree at `repo` against its merge base: frozen
    copies aside, the baseline held against the base's (the CLI and the
    tree's own pin)."""
    if base is tb._UNSET:
        base = tb.merge_base(repo)
    files = scope_files(repo)
    out = grade(repo, files, whole=True, copies=copies_of(repo, base, files), catalog=catalog)
    return out + (ratchet(repo, base) if base else [])


def scan(files, repo, base=tb._UNSET):
    """The precheck station: the files of the diff, the whole scope when the
    diff touches the check, its rules, its baseline or the catalog the tree
    reads (`CATALOG_FILE`, the rules file's `catalog_file`); a diff at the
    baseline is held against the baseline of the merge base."""
    if base is tb._UNSET:
        base = tb.merge_base(repo)
    out = []
    if BASELINE_FILE in files:
        if base:
            out += ratchet(repo, base)
        else:
            out.append(Finding("NOTE", CHECK, "no merge base with master or main -- the baseline cannot be "
                                              "held against its base, its growth is not graded"))
    if any(f in SELF_FILES for f in files) or catalog_file(repo) in {posixpath.normpath(f) for f in files}:
        paths = scope_files(repo)
        return grade(repo, paths, whole=True, copies=copies_of(repo, base, paths)) + out
    paths = [f for f in files if in_scope(f)]
    return grade(repo, paths, copies=copies_of(repo, base, paths)) + out


def write_baseline(repo, catalog=None):
    rules, rows = load_rules(repo), load_catalog(repo, catalog)
    counts = collections.Counter()
    for path in scope_files(repo):
        for hit in hits_of(repo, path):
            if rule_for(hit, rules, rows) is None:
                counts[(fingerprint(hit).rsplit("|", 2)[0], hit.name, hit.value)] += 1
    entries = [{"path": p, "name": n, "value": v, "count": c}
               for (p, n, v), c in sorted(counts.items(), key=lambda kv: (kv[0][0], kv[0][1], str(kv[0][2])))]
    about = ("The fixed window, budget and rate values the check `fixed-caps` holds as stock "
             "(scripts/fixed_caps.py). Rewrite with `scripts/fixed_caps.py --write-baseline`; "
             "never add an entry by hand to let a new value through.")
    # one entry a line: a rewrite diffs by entry
    body = ",\n".join("  " + json.dumps(e, ensure_ascii=False) for e in entries)
    with open(os.path.join(repo, BASELINE_FILE), "w", encoding="utf-8") as fh:
        fh.write('{\n "about": %s,\n "entries": [\n%s\n ]\n}\n' % (json.dumps(about), body))
    return len(entries), sum(counts.values())


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0],
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--repo", default=os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
    ap.add_argument("--catalog", help="the catalog file the catalog rules read (default: the rules file's "
                    "catalog_file, else %s of the tree, else $%s)" % (CATALOG_FILE, CATALOG_ENV))
    mode = ap.add_mutually_exclusive_group()
    mode.add_argument("--write-baseline", action="store_true", help="write the baseline from the tree")
    mode.add_argument("--list", action="store_true", help="every finding with its verdict")
    args = ap.parse_args(argv)
    repo = os.path.abspath(args.repo)
    if args.write_baseline:
        n, total = write_baseline(repo, args.catalog)
        print("%s: %d entries, %d occurrences" % (BASELINE_FILE, n, total))
        return 0
    if args.list:
        rules, base, rows = load_rules(repo), load_baseline(repo), load_catalog(repo, args.catalog)
        for path in scope_files(repo):
            for hit in hits_of(repo, path):
                r = rule_for(hit, rules, rows)
                verdict = "rule:" + r["id"] if r else ("baseline" if fingerprint(hit) in base else "NEW")
                print("%s\t%s\t%s\t%s\t%s" % (verdict, hit.kind, location(hit), hit.name, hit.value))
        return 0
    findings = grade_tree(repo, catalog=args.catalog)
    for f in findings:
        print("%s %s: %s" % (f.level, f.check, f.message))
    red = sum(1 for f in findings if f.level == "RED")
    print("%s: %d red, %d notes" % (CHECK, red, len(findings) - red))
    return 1 if red else 0


if __name__ == "__main__":
    sys.exit(main())
