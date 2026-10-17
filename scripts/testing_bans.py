#!/usr/bin/env python3
"""meclaw -- the testing bans: added test lines graded against `.config/testing-bans.toml`.

WHY THIS EXISTS
===============
Wall-clock verdicts flaked under load and cost about 25 minutes of isolation
reruns per gate; a sleep, an assert on elapsed time or a timeout used as a
window judge the machine, not the code (`docs/development-rules.md` § 11,
ADR-0050). The rules are text; this module is the part of them that a gate
can hold. It runs inside the precheck station, before the cargo lock, and
costs seconds: it reads ONE diff, not the tree.

WHAT IT GRADES
==============
    test-bans       every ban of `.config/testing-bans.toml` on the ADDED
                    statements of test code (a statement counts once one of
                    its lines is added; comments never match; string
                    contents match only where the ban is about a string).
                    Rust test code is a test file, a `#[cfg(test)]` block, or a
                    file a `#[cfg(test)] mod x;` declares (transitively).
                    The lines come from `git diff -M -U0 <merge-base>`: a
                    moved file adds nothing, an untracked one adds every line.
                    Without a merge base (the published tree) the check is a
                    NOTE `no base` and grades nothing.
                    An exception is a comment on the line or the line above:
                        testing-ban: allow <ban-id> <OR-id>: <reason>
                    The OR-id must stand in `.config/testing-rulings.toml`
                    (written only by the owner or the coordinator), name the
                    ban and the path, not be expired and not be a strand
                    ruling `OR-<wave>.<strand>.<n>`; the reason has ten
                    characters at least, and an exception that covers no
                    finding is RED. A used one is a NOTE `test-ban-exception`.
    testing-rules   the change reservation (VG § 5.7): a diff that loosens
                    the ban data, adds a test to `.config/flaky-legacy.toml`,
                    removes a ban from `ruff.toml` or the clock-ban clippy
                    config, raises, drops or deletes a count of
                    `.config/testing-bans.baseline.json` (it only sinks), or
                    changes § 11 of `docs/development-rules.md` or this module
                    at all is RED without `MECLAW_TESTING_RULES_RULING=<OR-id>`
                    whose register entry names the file under `loosens`.
                    A strand diff on the register is RED; in an integration or
                    release pass a register commit that changes other files
                    is RED. Tightening is free.

Python bans run as regex while ruff is missing (NOTE `ruff missing, regex
fallback`, never a silent SKIP); `ruff.toml` holds the same bans for the day
ruff is installed.

USAGE
=====
    testing_bans.py --files PATH...          the precheck findings of these paths
    testing_bans.py --inventory              count every ban over the tree
    testing_bans.py --inventory --write      lower the baseline (wave end, R-TV-W8)
    testing_bans.py --inventory --check      exit 1 when a count rose over the
                                             baseline or a ban has none
                                             (station `test-bans-inventory`)
"""

import argparse
import bisect
import collections
import datetime
import functools
import json
import os
import re
import shutil
import subprocess
import sys
import tomllib

Finding = collections.namedtuple("Finding", "level check message")

BANS_FILE = ".config/testing-bans.toml"
REGISTER_FILE = ".config/testing-rulings.toml"
LEGACY_FILE = ".config/flaky-legacy.toml"
BASELINE_FILE = ".config/testing-bans.baseline.json"
RUFF_FILE = "ruff.toml"
CLOCK_BANS_FILE = ".github/gates/clock-bans/clippy.toml"
# The ceilings of the clock-bans/unwrap ratchet: they only sink (re-check
# tv-p1b M1, OR-TV-10), like the baseline.
BUDGET_FILE = ".github/gates/unwrap_budget.txt"
# This module: ten of the seventeen bans are decided by its refine functions,
# its clock list and its string views, so it is reserved like the data
# (re-check tv-p0 A5).
CODE_FILE = "scripts/testing_bans.py"
RULES_DOC = "docs/development-rules.md"
RULING_ENV = "MECLAW_TESTING_RULES_RULING"

SUFFIX = {"rust": ".rs", "python": ".py", "js": ".mjs"}
# `by` of a register entry: a role, never a person (re-check tv-p0 C2, export R5).
OWNERS = ("owner", "coordinator")
MIN_REASON = 10

_EXCEPTION = re.compile(
    r"testing-ban:\s*allow\s+([A-Za-z0-9_-]+)(?:\s+([^\s:]+))?\s*:\s*(.*?)\s*(?:\*/)?\s*$")
# A strand ruling (PREAMBLE § 3 P. 7): `OR-<wave>.<strand>.<n>`.
_STRAND_RULING = re.compile(r"^OR-[^.\s]+\.[^.\s]+\.\d+$")
_SECTION_11 = re.compile(r"^## 11\.", re.MULTILINE)
_NEXT_SECTION = re.compile(r"^## ", re.MULTILINE)

_UNSET = object()


# --- git -----------------------------------------------------------------------

def _git(tree, *args):
    """stdout of `git -C tree args`, or None when git fails."""
    try:
        res = subprocess.run(["git", "-C", tree, "-c", "core.quotePath=false"] + list(args),
                             capture_output=True, text=True, timeout=60)
    except (OSError, subprocess.TimeoutExpired):
        return None
    return res.stdout if res.returncode == 0 else None


def merge_base(tree):
    """The merge base with master, else main (the order `locks.merge_base` uses)."""
    for name in ("master", "main", "origin/master", "origin/main"):
        out = _git(tree, "merge-base", name, "HEAD")
        if out and out.strip():
            return out.strip()
    return None


def _show(tree, base, rel):
    """The text of `rel` at `base`, or None where it did not exist."""
    return _git(tree, "show", "%s:%s" % (base, rel)) if base else None


def _rc(tree, *args):
    """The exit code of `git -C tree args`, or None when git cannot run."""
    try:
        return subprocess.run(["git", "-C", tree] + list(args), capture_output=True,
                              timeout=60).returncode
    except (OSError, subprocess.TimeoutExpired):
        return None


def _born(tree, base, rel):
    """True when `rel` is absent at `base`, False when it is there, None when
    git cannot tell (a missing or broken base, a timeout, a broken repo).
    Only a real absence is "born": an error must never read as one."""
    if _rc(tree, "cat-file", "-e", "%s^{commit}" % base) != 0:
        return None
    rc = _rc(tree, "cat-file", "-e", "%s:%s" % (base, rel))
    return {0: False, 128: True}.get(rc)


def _unchanged(tree, base, rel):
    """True when `rel` in the working tree reads as it did at `base`."""
    old = _show(tree, base, rel)
    if old is None:
        return False
    try:
        with open(os.path.join(str(tree), rel), encoding="utf-8") as fh:
            return fh.read() == old
    except OSError:
        return False


def added_lines(tree, base, paths):
    """{path: set of added line numbers | None for every line}, ONE diff for all paths.

    `-M`: a renamed file adds only what its rename changed. A path absent from
    the diff and untracked adds every line; tracked and unchanged adds none.
    """
    out = {}
    if not paths:
        return out
    roots = sorted({p.split("/", 1)[0] for p in paths})
    text = _git(tree, "diff", "-M", "-U0", "--no-color", "--no-ext-diff", base, "--", *roots) or ""
    current = None
    for line in text.splitlines():
        if line.startswith("+++ "):
            target = line[4:]
            current = target[2:] if target.startswith("b/") else None
            if current is not None:
                out.setdefault(current, set())
        elif line.startswith("@@") and current is not None:
            m = re.match(r"@@ -\S+ \+(\d+)(?:,(\d+))? @@", line)
            if m:
                start, count = int(m.group(1)), int(m.group(2) or "1")
                out[current].update(range(start, start + count))
    missing = [p for p in paths if p not in out]
    if missing:
        listing = _git(tree, "ls-files", "--others", "--exclude-standard", "--", *missing) or ""
        for p in listing.splitlines():
            out[p] = None
    return out


# --- data ------------------------------------------------------------------------

def _read_toml(path):
    try:
        with open(path, "rb") as fh:
            return tomllib.load(fh)
    except (OSError, tomllib.TOMLDecodeError):
        return None


def _toml_text(text):
    try:
        return tomllib.loads(text or "")
    except tomllib.TOMLDecodeError:
        return {}


def load_bans(repo):
    doc = _read_toml(os.path.join(str(repo), BANS_FILE)) or {}
    return list(doc.get("ban", []))


def load_register(repo):
    """{OR-id: entry}, or None when the register does not exist."""
    doc = _read_toml(os.path.join(str(repo), REGISTER_FILE))
    if doc is None:
        return None
    return {e.get("id"): e for e in doc.get("ruling", []) if e.get("id")}


def read_legacy(repo):
    doc = _read_toml(os.path.join(str(repo), LEGACY_FILE)) or {}
    return list(doc.get("flaky", []))


@functools.lru_cache(maxsize=256)
def _glob_re(glob):
    out, i = [], 0
    while i < len(glob):
        if glob.startswith("**/", i):
            out.append(r"(?:.*/)?")
            i += 3
        elif glob.startswith("**", i):
            out.append(r".*")
            i += 2
        elif glob[i] == "*":
            out.append(r"[^/]*")
            i += 1
        elif glob[i] == "?":
            out.append(r"[^/]")
            i += 1
        else:
            out.append(re.escape(glob[i]))
            i += 1
    return re.compile("^" + "".join(out) + "$")


def matches(path, glob):
    return bool(_glob_re(glob).match(path))


def ruling_problem(register, rid, path=None, ban=None, loosens=None, today=None):
    """Why `rid` does not hold here, or None when it does (VG § 5.4, B4)."""
    if not rid:
        return "no ruling id"
    if _STRAND_RULING.match(rid):
        return "%s is a strand ruling, which never opens a ban" % rid
    if register is None:
        return "%s: the register %s does not exist" % (rid, REGISTER_FILE)
    entry = register.get(rid)
    if entry is None:
        return "%s is not in the register %s" % (rid, REGISTER_FILE)
    if entry.get("by") not in OWNERS:
        return "%s: `by` is %r, not the owner or the coordinator" % (rid, entry.get("by"))
    # The form of the entry, fail-closed (re-check tv-bn M7m B-H3): a string
    # under `loosens` matched any substring, a malformed `expires` never expired.
    for field in ("bans", "paths", "loosens"):
        value = entry.get(field, [])
        if not (isinstance(value, list) and all(isinstance(v, str) for v in value)):
            return "%s: `%s` is not a list of strings in the register %s" % (rid, field, REGISTER_FILE)
    expires = entry.get("expires")
    if expires not in (None, ""):
        if isinstance(expires, datetime.date) and not isinstance(expires, datetime.datetime):
            until = expires
        elif isinstance(expires, str) and re.fullmatch(r"\d{4}-\d{2}-\d{2}", expires):
            try:
                until = datetime.date.fromisoformat(expires)
            except ValueError:
                until = None
        else:
            until = None
        if until is None:
            return "%s: `expires` is %r, not a date YYYY-MM-DD in the register %s" % (rid, expires, REGISTER_FILE)
        if until < (today or datetime.date.today()):
            return "%s expired on %s" % (rid, until.isoformat())
    if ban is not None and ban not in entry.get("bans", []):
        return "%s does not name %s (out of its scope)" % (rid, ban)
    if path is not None and not any(matches(path, g) for g in entry.get("paths", [])):
        return "%s does not cover %s (out of its scope)" % (rid, path)
    if loosens is not None and loosens not in entry.get("loosens", []):
        return "%s does not name %s under `loosens`" % (rid, loosens)
    return None


# --- source --------------------------------------------------------------------------

_LEX = {
    "rust": re.compile(r"""(?P<c>//[^\n]*|/\*.*?\*/)
        |(?P<s>(?<![\w])b?r(?P<h>\#*)"(?:.|\n)*?"(?P=h)|b?"(?:\\.|[^"\\])*"|b?'(?:\\.|[^\\'\n])')""",
                       re.S | re.X),
    "python": re.compile(r"""(?P<c>\#[^\n]*)
        |(?P<s>(?<![\w])[rRbBuUfF]{0,2}(?:'''(?:\\.|[^\\])*?'''|\"\"\"(?:\\.|[^\\])*?\"\"\"
            |'(?:\\.|[^'\\\n])*'|"(?:\\.|[^"\\\n])*"))""", re.S | re.X),
    "js": re.compile(r"""(?P<c>//[^\n]*|/\*.*?\*/)
        |(?P<s>'(?:\\.|[^'\\\n])*'|"(?:\\.|[^"\\\n])*"|`(?:\\.|[^`\\])*`)""", re.S | re.X),
}


def _blank(text):
    return re.sub(r"[^\n]", " ", text)


# A `/` that is not a comment starts a regex literal where an operand is
# expected: at the start, after an operator or opening punctuation, or after a
# keyword. After a value (`)`, `]`, a name, a number, a literal) it divides.
_JS_NEXT = re.compile(r"[/'\"`]")
_JS_REGEX = re.compile(r"/(?:\\.|\[(?:\\.|[^\]\\\n])*\]|[^/\\\[\n])+/[A-Za-z]*")
_JS_OPERAND_WORDS = {"return", "typeof", "instanceof", "in", "of", "new", "delete", "void",
                     "throw", "case", "do", "else", "yield", "await"}


def _js_regex_allowed(before):
    """Can a regex literal start after `before` (the tail of the code since
    the last literal, None right after a string or regex literal)?"""
    if before is None:
        return False
    tail = before.rstrip()
    if not tail:
        return True
    ch = tail[-1]
    if ch in ")]." or ch.isdigit():
        return False
    if ch.isalpha() or ch in "_$":
        return re.search(r"[\w$]+$", tail).group(0) in _JS_OPERAND_WORDS
    return True


def _tokens(text, lang):
    """[(kind, start, end)] of comments ("c"), strings ("s") and, in js,
    regex literals ("r") -- a `//` inside a regex is no comment (#1208)."""
    if lang != "js":
        return [("c" if m.group("c") is not None else "s", m.start(), m.end())
                for m in _LEX[lang].finditer(text)]
    lex, out, pos, before = _LEX["js"], [], 0, ""
    while True:
        m = _JS_NEXT.search(text, pos)
        if m is None:
            return out
        at, gap = m.start(), text[pos:m.start()]
        if before is not None or gap.strip():
            before = ((before or "") + gap).rstrip()[-32:]
        tok = lex.match(text, at)
        if tok is not None:
            kind = "c" if tok.group("c") is not None else "s"
            out.append((kind, at, tok.end()))
            pos = tok.end()
            if kind == "s":
                before = None
            continue
        rx = _JS_REGEX.match(text, at) if _js_regex_allowed(before) else None
        if rx is not None:
            out.append(("r", at, rx.end()))
            pos, before = rx.end(), None
            continue
        before = (before or "")[-31:] + text[at]
        pos = at + 1


class Source:
    """One file: comments removed, string contents blanked in `code`, kept in `strings`."""

    def __init__(self, text, lang):
        self.lang = lang
        code, strings, comments, last = [], [], [], 0
        self._text, self._literals = text, []
        for kind, start, end in _tokens(text, lang):
            code.append(text[last:start])
            strings.append(text[last:start])
            tok = text[start:end]
            if kind == "c":
                comments.append((start, tok))
                code.append(_blank(tok))
                strings.append(_blank(tok))
            else:
                if kind == "r":
                    # a regex literal: `/` and `/flags` stay code, the body is blanked
                    head, tail = 1, len(tok) - tok.rindex("/")
                else:
                    self._literals.append((start, end))
                    pm = re.match(r"[rRbBuUfF]*(#*)(\"\"\"|\'\'\'|\"|\'|`)", tok)
                    head = pm.end() if pm else 0
                    tail = len(pm.group(2)) + len(pm.group(1)) if pm else 0
                    tail = min(tail, len(tok) - head)
                code.append(tok[:head] + _blank(tok[head:len(tok) - tail]) + tok[len(tok) - tail:])
                strings.append(tok)
            last = end
        code.append(text[last:])
        strings.append(text[last:])
        self.code = "".join(code)
        self.strings = "".join(strings)
        # `#![cfg(test)]` makes the whole file test code; the module lookup of
        # `cfg_test_file` sets it for a file a `#[cfg(test)] mod x;` declares (A3)
        self.test_file = lang == "rust" and bool(re.search(r"#!\[cfg\((?:all\()?test\b", self.code))
        self.nl = [i for i, ch in enumerate(text) if ch == "\n"]
        self.comments = comments
        self._statements()
        self._braces()

    @functools.cached_property
    def only_strings(self):
        """The string literals alone, code and comments blanked: a quote-to-quote
        match cannot span code between two literals, and a literal over several
        lines stays one (re-check tv-p0 A2). Built on first use."""
        blank, text, parts, last = _blank(self._text), self._text, [], 0
        for a, b in self._literals:
            parts.append(blank[last:a])
            parts.append(text[a:b])
            last = b
        parts.append(blank[last:])
        return "".join(parts)

    def line(self, pos):
        return bisect.bisect_left(self.nl, pos) + 1

    def _statements(self):
        """Statement spans: `;` and (rust/js) braces at paren depth 0 end one;
        python and js end one at a newline at depth 0 as well."""
        code, lang = self.code, self.lang
        opens = "([{" if lang == "python" else "(["
        closes = ")]}" if lang == "python" else ")]"
        starts, depth, start = [], 0, 0
        for m in re.finditer(r"[;{}()\[\]\n]", code):
            ch, pos = m.group(0), m.start()
            if ch in opens:
                depth += 1
                continue
            if ch in closes:
                depth = max(0, depth - 1)
                continue
            if depth:
                continue
            if ch == "\n":
                if lang == "rust":
                    continue
                if lang == "python" and code[start:pos].rstrip().endswith("\\"):
                    continue
                if lang == "js":
                    after = code[pos + 1:pos + 200].lstrip()
                    before = code[start:pos].rstrip()
                    if after[:1] in (".", "?", ":") or after[:2] in ("&&", "||") \
                            or before[-1:] in ("=", ">", "+", "-", ",", "(", "&", "|", "?", ":"):
                        continue
            starts.append((start, pos + 1))
            start = pos + 1
        starts.append((start, len(code)))
        self.spans = [(a, b) for a, b in starts if code[a:b].strip()]
        self._span_starts = [a for a, _ in self.spans]

    def statement(self, pos):
        i = bisect.bisect_right(self._span_starts, pos) - 1
        return self.spans[max(i, 0)]

    def _braces(self):
        stack, pairs = [], {}
        for m in re.finditer(r"[{}]", self.code):
            if m.group(0) == "{":
                stack.append(m.start())
            elif stack:
                pairs[stack.pop()] = m.start()
        self.pairs = pairs
        self._open = sorted(pairs)

    def block_after(self, pos):
        """The (open, close) of the first brace block opening after `pos`, before a `;`."""
        i = bisect.bisect_left(self._open, pos)
        if i >= len(self._open):
            return None
        semi = self.code.find(";", pos)
        if semi != -1 and semi < self._open[i]:
            return None
        return self._open[i], self.pairs[self._open[i]]

    @functools.cached_property
    def fns(self):
        """[(attrs, open, close)] of every fn body, attrs = the attribute text above it."""
        out = []
        for m in re.finditer(r"\bfn\s+\w+", self.code):
            block = self.block_after(m.end())
            if block is None:
                continue
            head = self.code[max(0, m.start() - 400):m.start()]
            attrs = re.findall(r"#\[[^\]]*\]", head.rsplit("}", 1)[-1].rsplit(";", 1)[-1])
            out.append((" ".join(attrs), block[0], block[1]))
        return out

    def enclosing_fn(self, pos):
        best = None
        for attrs, a, b in self.fns:
            if a <= pos <= b and (best is None or a > best[1]):
                best = (attrs, a, b)
        return best

    @functools.cached_property
    def cfg_test(self):
        out = []
        for m in re.finditer(r"#\[cfg\((?:all\()?test\b[^\]]*\]", self.code):
            block = self.block_after(m.end())
            if block:
                out.append((m.start(), block[1]))
        return out

    def in_cfg_test(self, pos):
        return self.test_file or any(a <= pos <= b for a, b in self.cfg_test)

    def test_mods(self):
        """The names of the out-of-line modules (`mod x;`) this file declares as
        test code: under a `#[cfg(test)]` attribute, inside a `#[cfg(test)]`
        block, or anywhere in a file that is test code itself."""
        out = set()
        for m in re.finditer(r"\bmod\s+(\w+)\s*;", self.code):
            head = self.code[max(0, m.start() - 400):m.start()]
            attrs = head.rsplit("}", 1)[-1].rsplit(";", 1)[-1]
            if re.search(r"#\[cfg\((?:all\()?test\b", attrs) or self.in_cfg_test(m.start()):
                out.add(m.group(1))
        return out

    def start_paused(self, pos):
        """Whether `pos` lies in a fn whose attributes carry `start_paused = true`."""
        return any(a <= pos <= b and re.search(r"start_paused\s*=\s*true", attrs)
                   for attrs, a, b in self.fns)

    def call_args(self, open_paren):
        """The comma-split top-level arguments of the call whose `(` is at `open_paren`."""
        depth, args, last = 0, [], open_paren + 1
        for i in range(open_paren, min(len(self.strings), open_paren + 4000)):
            ch = self.code[i]
            if ch in "([{":
                depth += 1
            elif ch in ")]}":
                depth -= 1
                if depth == 0:
                    args.append(self.strings[last:i])
                    return [a.strip() for a in args]
            elif ch == "," and depth == 1:
                args.append(self.strings[last:i])
                last = i + 1
        return [a.strip() for a in args]


# --- refinements per ban --------------------------------------------------------------

_CLOCK = re.compile(r"\belapsed\s*\(|\bInstant::now\b|\bSystemTime::now\b|\bUtc::now\b"
                    r"|\bduration_since\s*\(")


# GH #1199 R1.3: GN2 found a wall clock behind a file-local helper
# (`fn epoch_now() -> f64 { SystemTime::now()... }`) that the ban did not see.
# A helper counts as a clock when the value it RETURNS is read from one -- its
# tail expression or a `return` names the clock or a variable of it -- and its
# return type carries time or a number of it; one level, the file's own fns
# only. A helper that reads the clock for a deadline (a hang marker) and hands
# back a count, a body or the event's outcome stays out: the class search over
# the tree found 18 such files when the body alone decided.
# OR-TV-12 (coordinator, 2026-10-16): like every diff ban, the helper is judged
# on ADDED statements only (`scan` hands `findings_in` the added lines); a
# statement that was there before is not red. The inventory counts it, and the
# baseline was written once at the stock this sharper detection measures.
_TIME_RET = re.compile(r"\b(?:Instant|SystemTime|Duration|DateTime|NaiveDateTime"
                       r"|f32|f64|u16|u32|u64|u128|usize|i32|i64|i128|isize)\b")
_TEST_FN = re.compile(r"#\[(?:tokio::)?test\b")


def _returned(body):
    """The returned expressions of a fn body: its tail (the text after the last
    `;` or block `}` on the body's own level) and every `return <expr>`."""
    depth, cut = 0, -1
    for i, ch in enumerate(body):
        if ch in "([{":
            depth += 1
        elif ch in ")]}":
            depth -= 1
            if depth == 0 and ch == "}":
                cut = i
        elif ch == ";" and depth == 0:
            cut = i
    return [body[cut + 1:]] + re.findall(r"\breturn\b([^;]*)", body)


def _returns_clock(body):
    out = set()   # the fn's own clock variables, as `_clock_vars` finds them
    for m in re.finditer(r"\blet\s+(?:mut\s+)?(\w+)\s*(?::[^=;]+)?=([^;]*);", body):
        if _CLOCK.search(m.group(2)) or any(re.search(r"\b%s\b" % v, m.group(2)) for v in out):
            out.add(m.group(1))
    return any(_CLOCK.search(e) or any(re.search(r"\b%s\b" % v, e) for v in out)
               for e in _returned(body))


def _clock_re(src):
    """`_CLOCK` plus a call of every clock helper of `src` (cached on `src`)."""
    got = getattr(src, "_clock_helper_re", None)
    if got is not None:
        return got
    names = set()
    for m in re.finditer(r"\bfn\s+(\w+)\s*(?:<[^>{;]*>)?\s*\(", src.code):
        block = src.block_after(m.end())
        if block is None:
            continue
        sig = src.code[m.end():block[0]]
        ret = sig.rsplit("->", 1)[1] if "->" in sig else ""
        attrs = src.code[max(0, m.start() - 400):m.start()].rsplit("}", 1)[-1]
        if (_TIME_RET.search(ret) and not _TEST_FN.search(attrs)
                and _returns_clock(src.code[block[0] + 1:block[1]])):
            names.add(m.group(1))
    got = re.compile(_CLOCK.pattern + "".join(
        r"|\b%s\s*\(" % re.escape(n) for n in sorted(names)))
    src._clock_helper_re = got
    return got


def _clock_vars(src, fn):
    if fn is None:
        return set()
    body = src.code[fn[1]:fn[2]]
    clock = _clock_re(src)
    out = set()
    for m in re.finditer(r"\blet\s+(?:mut\s+)?(\w+)\s*(?::[^=;]+)?=([^;]*);", body):
        if clock.search(m.group(2)) or any(re.search(r"\b%s\b" % v, m.group(2)) for v in out):
            out.add(m.group(1))
    return out


def _refine_elapsed(src, m, span, path):
    text = src.code[span[0]:span[1]]
    if _clock_re(src).search(text):
        return True
    return any(re.search(r"\b%s\b" % re.escape(v), text)
               for v in _clock_vars(src, src.enclosing_fn(m.start())))


def _refine_short_deadline(src, m, span, path):
    args = src.call_args(m.end() - 1)
    first = args[0] if args else ""
    return bool(re.search(r"from_millis\s*\(\s*\d|from_secs\s*\(\s*\d\s*\)", first))


def _refine_neg_window(src, m, span, path):
    text = src.code[span[0]:span[1]]
    if re.search(r"\btimeout\s*\(", text) and re.search(r"\.await", text):
        if m.group(0).startswith("matches"):
            return bool(re.search(r"Err\s*\(\s*_\s*\)", text))
        return True
    fn = src.enclosing_fn(m.start())
    if fn is None:
        return False
    body = src.code[fn[1]:fn[2]]
    windows = set(re.findall(r"\blet\s+(?:mut\s+)?(\w+)\s*=\s*(?:tokio::time::)?timeout\s*\([^;]*"
                             r"\.await\s*;", body))
    if not windows:
        return False
    head = src.code[max(span[0], m.start() - 80):m.start()]
    if m.group(0).startswith("matches"):
        args = src.call_args(m.end() - 1)
        return bool(args) and args[0] in windows and len(args) > 1 and "Err" in args[1]
    return any(head.rstrip().endswith(v) for v in windows)


def _refine_wall_verdict(src, m, span, path):
    return bool(re.search(r"took|secs|elapsed|monotonic", src.code[span[0]:span[1]]))


def _refine_raises(src, m, span, path):
    if m.group(0).startswith("pytest"):
        return "match=" not in src.code[span[0]:span[1]]
    return True


def _refine_js_sleep(src, m, span, path):
    args = src.call_args(m.end() - 1)
    value = (args[0] if m.group(0).startswith("sleep") else
             (args[1] if len(args) > 1 else "0")) if args else ""
    return value not in ("0", "")


def _refine_unawaited(src, m, span, path):
    return src.code[span[0]:m.start()].strip() == ""


def _refine_duration(src, m, span, path):
    text = src.code[span[0]:span[1]]
    verdict = re.match(r"\s*(?:if\s*\(|while\s*\()", text) or re.search(r"\bassert\w*|\bexpect\s*\(",
                                                                       text)
    return bool(verdict) and bool(re.search(r"now\s*\(\s*\)\s*-|-\s*(?:Date|performance)\.now",
                                            text))


def _refine_screenshot(src, m, span, path):
    text = src.strings[span[0]:span[1]]
    return not (re.search(r"scale\s*:\s*['\"]css['\"]", text)
                and re.search(r"animations\s*:\s*['\"]disabled['\"]", text)
                and re.search(r"\btimeout\s*:", text))


def _refine_tokio_sleep(src, m, span, path):
    if "::" in m.group(0):
        return True
    return bool(re.search(r"use\s+tokio::time::(?:\{[^}]*\bsleep\b|sleep\b)", src.code))


REFINE = {
    "rs-tokio-sleep": _refine_tokio_sleep,
    "rs-elapsed-verdict": _refine_elapsed,
    "rs-short-deadline": _refine_short_deadline,
    "rs-neg-window": _refine_neg_window,
    "py-wall-verdict": _refine_wall_verdict,
    "py-raises": _refine_raises,
    "js-sleep": _refine_js_sleep,
    "js-unawaited": _refine_unawaited,
    "js-duration-verdict": _refine_duration,
    "js-screenshot": _refine_screenshot,
}

# Bans whose subject is a string, and the text they match: "only" = the string
# literals alone (a path or SQL, re-check tv-p0 A2), "also" = code with the string
# contents kept (`networkidle` stands in a string, A1). Comments never match.
ON_STRINGS = {"rs-colony-db": "only", "js-racy-api": "also"}


def _module_parents(path):
    """[(file, name)]: where `mod name;` would declare the Rust file `path`."""
    d, f = path.rsplit("/", 1) if "/" in path else ("", path)
    if f == "mod.rs":
        if "/" not in d:
            return []
        d, name = d.rsplit("/", 1)
    else:
        name = f[:-3]
        if name in ("lib", "main"):
            return []
    out = [(d + ".rs", name), (d + "/mod.rs", name)]
    if d.endswith("/src"):
        out += [(d + "/lib.rs", name), (d + "/main.rs", name)]
    return out


def cfg_test_file(repo, path, _depth=0):
    """Whether the Rust file `path` is test code as a whole: declared by a
    `#[cfg(test)] mod x;` (or inside a test module), transitively (re-check
    tv-p0 A3: 3 files, 18 statements were out of every ban's scope)."""
    if not path.endswith(".rs") or _depth > 12:
        return False
    for parent, name in _module_parents(path):
        full = os.path.join(str(repo), parent)
        try:
            st = os.stat(full)
        except OSError:
            continue
        mods = _mods_of(full, st.st_mtime_ns, st.st_size)
        if mods is None:
            continue
        test_mods, all_mods = mods
        if name in test_mods:
            return True
        if name in all_mods and cfg_test_file(repo, parent, _depth + 1):
            return True
    return False


@functools.lru_cache(maxsize=4096)
def _mods_of(full, _mtime_ns, _size):
    """(test modules, all modules) a Rust file declares out of line; the stat
    in the key keeps a changed file from reading stale (the inventory asks the
    same parents for every child)."""
    try:
        with open(full, encoding="utf-8", errors="replace") as fh:
            src = Source(fh.read(), "rust")
    except OSError:
        return None
    return (frozenset(src.test_mods()),
            frozenset(re.findall(r"\bmod\s+(\w+)\s*;", src.code)))


def _read_source(repo, path, bans):
    """The Source of `path`, its whole-file test flag set when a cfg-test scope
    grades it; None when it cannot be read."""
    try:
        with open(os.path.join(str(repo), path), encoding="utf-8", errors="replace") as fh:
            src = Source(fh.read(), _lang_of(path))
    except OSError:
        return None
    if src.lang == "rust" and not src.test_file \
            and any(in_scope(b, path) == "cfg-test" for b in bans):
        src.test_file = cfg_test_file(repo, path)
    return src


# --- matching -------------------------------------------------------------------------

def in_scope(ban, path):
    """'file' | 'cfg-test' | None: whether `path` is graded by `ban`, and how."""
    if not path.endswith(SUFFIX.get(ban.get("lang"), "\0")):
        return None
    how = None
    for g in ban.get("scope", []):
        if g.startswith("cfg-test:"):
            if matches(path, g[len("cfg-test:"):]):
                how = how or "cfg-test"
        elif matches(path, g):
            return "file"
    return how


def _context_allows(ban, src, pos, path):
    ctx = ban.get("context", [])
    for c in ctx:
        if c.startswith("allow:") and matches(path, c[len("allow:"):]):
            return True
    if "start_paused" in ctx and src.start_paused(pos):
        if "tokio_instant" not in ctx:
            return True
        return bool(re.search(r"tokio::time::(?:\{[^}]*\bInstant\b|Instant\b)", src.code))
    return False


def findings_in(src, path, bans, lines=None):
    """[(ban, line, (start_line, end_line))] of the statements a ban hits.

    `lines` = the added line numbers (None = every line)."""
    out = []
    for ban in bans:
        how = in_scope(ban, path)
        if how is None:
            continue
        view = ON_STRINGS.get(ban["id"])
        text = src.only_strings if view == "only" else src.strings if view == "also" else src.code
        seen = set()
        for m in re.finditer(ban["pattern"], text):
            span = src.statement(m.start())
            if span in seen:
                continue
            lead = re.match(r"\s*", src.code[span[0]:span[1]]).end()   # OR-TV-7: first non-blank
            first, last = src.line(span[0] + lead), src.line(max(span[0], span[1] - 1))
            if lines is not None and not any(n in lines for n in range(first, last + 1)):
                continue
            if how == "cfg-test" and not src.in_cfg_test(m.start()):
                continue
            refine = REFINE.get(ban["id"])
            if refine is not None and not refine(src, m, span, path):
                continue
            if _context_allows(ban, src, m.start(), path):
                continue
            seen.add(span)
            out.append((ban, src.line(m.start()), (first, last)))
    return out


def _lang_of(path):
    for lang, suffix in SUFFIX.items():
        if path.endswith(suffix):
            return lang
    return None


def _render(path, line, ban, extra=""):
    return "%s:%d %s (%s): %s -- %s%s" % (path, line, ban["id"], ban["rule"], ban["explanation"],
                                          ban["replacement"], extra)


def scan(files, repo, base=_UNSET, today=None):
    """The `test-bans` findings of the paths `files` (precheck's diff)."""
    repo = str(repo)
    bans = load_bans(repo)
    if not bans:
        return []
    graded = [p for p in files if any(in_scope(b, p) for b in bans)
              and os.path.isfile(os.path.join(repo, p))]
    out = []
    py = [p for p in graded if any(in_scope(b, p) for b in bans if b.get("lang") == "python")]
    ruff = shutil.which("ruff") if py else None
    if py and ruff is None:
        out.append(Finding("NOTE", "test-bans", "(python) ruff missing, regex fallback"))
    if not graded:
        return out
    if base is _UNSET:
        base = merge_base(repo)
    if not base:
        return out + [Finding("NOTE", "test-bans", "(no base) no merge base -- the published "
                              "tree; nothing graded")]
    added = added_lines(repo, base, graded)
    register = _UNSET
    by_id = {b["id"]: b for b in bans}
    ruff_hits, ruff_bans = (_ruff(repo, ruff, py, by_id) if ruff else ({}, set()))
    for path in graded:
        lines = added.get(path, set())
        if lines is not None and not lines:
            continue
        src = _read_source(repo, path, bans)
        if src is None:
            continue
        hits = findings_in(src, path, [b for b in bans if b["id"] not in ruff_bans], lines)
        for line, bid in ruff_hits.get(path, ()):
            if lines is None or line in lines:
                hits.append((by_id[bid], line, (line, line)))
        exceptions = []
        for pos, comment in src.comments:
            m = _EXCEPTION.search(comment)
            if m:
                exceptions.append((src.line(pos), m.group(1), m.group(2), m.group(3)))
        if exceptions and register is _UNSET:
            register = load_register(repo)
        used = set()
        for ban, line, (first, last) in hits:
            covering = [e for e in exceptions if e[1] == ban["id"] and first - 1 <= e[0] <= last]
            if not covering:
                out.append(Finding(ban["level"], "test-bans", _render(path, line, ban)))
                continue
            exc = covering[0]
            used.add(exc)
            why = (ruling_problem(register, exc[2], path=path, ban=ban["id"], today=today)
                   or ("reason shorter than %d characters" % MIN_REASON
                       if len(exc[3]) < MIN_REASON else None))
            if why:
                out.append(Finding("RED", "test-bans", _render(
                    path, line, ban, " [exception refused: %s]" % why)))
            else:
                out.append(Finding("NOTE", "test-ban-exception",
                                   "%s:%d %s %s" % (path, line, ban["id"], exc[2])))
        for exc in exceptions:
            if exc in used or (lines is not None and exc[0] not in lines):
                continue
            what = ("unknown ban id" if exc[1] not in by_id else
                    "unused: no finding of the ban on its line or the next")
            out.append(Finding("RED", "test-bans", "%s:%d %s exception %s %s" % (
                path, exc[0], exc[1], exc[2] or "(no ruling id)", what)))
    return out


def _ruff(repo, ruff, paths, by_id):
    """({path: [(line, ban id)]}, ban ids ruff grades) -- ruff with `ruff.toml`.

    The ban id is the `msg` of a TID251 entry; the bans ruff names replace their
    regex, the others keep it. A ruff that fails grades nothing and the regex
    stays (an empty set)."""
    cfg = os.path.join(repo, RUFF_FILE)
    try:
        res = subprocess.run([ruff, "check", "--config", cfg, "--no-cache", "--exit-zero",
                              "--output-format", "json"] + list(paths), cwd=repo,
                             capture_output=True, text=True, timeout=120)
        rows = json.loads(res.stdout or "[]")
    except (OSError, subprocess.TimeoutExpired, ValueError):
        return {}, set()
    api = (_read_toml(cfg) or {}).get("lint", {}).get("flake8-tidy-imports", {}).get(
        "banned-api", {})
    graded = {v.get("msg") for v in api.values() if isinstance(v, dict)} & set(by_id)
    out = {}
    for row in rows:
        m = re.search(r"([a-z]+-[a-z0-9-]+)\s*$", row.get("message", ""))
        if row.get("code") != "TID251" or not m or m.group(1) not in graded:
            continue
        rel = os.path.relpath(row.get("filename", ""), repo).replace(os.sep, "/")
        out.setdefault(rel, []).append((row.get("location", {}).get("row", 0), m.group(1)))
    return out, graded


# --- testing-rules ------------------------------------------------------------------------

def _section_11(text):
    m = _SECTION_11.search(text or "")
    if not m:
        return None
    rest = text[m.end():]
    n = _NEXT_SECTION.search(rest)
    return text[m.start():m.end() + (n.start() if n else len(rest))]


def _ban_loosenings(old, new):
    old_b = {b.get("id"): b for b in _toml_text(old).get("ban", [])}
    new_b = {b.get("id"): b for b in _toml_text(new).get("ban", [])}
    out = []
    for bid, ob in old_b.items():
        nb = new_b.get(bid)
        if nb is None:
            out.append("ban %s removed" % bid)
            continue
        if set(ob.get("scope", [])) - set(nb.get("scope", [])):
            out.append("ban %s: scope narrowed" % bid)
        if set(nb.get("context", [])) - set(ob.get("context", [])):
            out.append("ban %s: context widened" % bid)
        if ob.get("level") == "RED" and nb.get("level") != "RED":
            out.append("ban %s: level lowered" % bid)
        if ob.get("pattern") != nb.get("pattern") or ob.get("lang") != nb.get("lang"):
            out.append("ban %s: pattern changed" % bid)
    return out


def _legacy_loosenings(old, new):
    before = {e.get("test") for e in _toml_text(old).get("flaky", [])}
    return ["entry %s added" % e.get("test") for e in _toml_text(new).get("flaky", [])
            if e.get("test") not in before]


def _ruff_loosenings(old, new):
    def bans(text):
        lint = _toml_text(text).get("lint", {})
        api = lint.get("flake8-tidy-imports", {}).get("banned-api", {})
        ignores = {(k, c) for k, v in lint.get("per-file-ignores", {}).items() for c in v}
        return set(api), set(lint.get("select", [])), ignores
    (oa, osel, oign), (na, nsel, nign) = bans(old), bans(new)
    return (["banned-api %s removed" % a for a in sorted(oa - na)]
            + ["rule %s no longer selected" % s for s in sorted(osel - nsel)]
            + ["per-file-ignore %s %s added" % i for i in sorted(nign - oign)])


def _clock_loosenings(old, new):
    """A removed ban, or a ban moved off the `mono:` column (re-check tv-p1b M1):
    `mono` is pinned at 0 where `wall` is not, so the move would carry a new
    std `Instant` read past the ratchet."""
    def columns(text):
        out = {}
        for m in _toml_text(text).get("disallowed-methods", []):
            path = m.get("path") if isinstance(m, dict) else m
            reason = m.get("reason", "") if isinstance(m, dict) else ""
            out[path] = reason.split(":", 1)[0].strip() if ":" in reason else ""
        return out
    before, after = columns(old), columns(new)
    out = ["disallowed method %s removed" % p for p in sorted(set(before) - set(after))]
    out += ["disallowed method %s: reason %s -> %s" % (p, before[p], after[p] or "(none)")
            for p in sorted(set(before) & set(after))
            if before[p] == "mono" and after[p] != "mono"]
    return out


def _workspace_packages(repo):
    """Package names of the workspace members, or None when unreadable."""
    try:
        members = _read_toml(os.path.join(repo, "Cargo.toml"))["workspace"]["members"]
        return {_read_toml(os.path.join(repo, m, "Cargo.toml"))["package"]["name"]
                for m in members}
    except (OSError, KeyError, TypeError, ValueError):
        return None


def _budget_loosenings(old, new, repo=None):
    """The unwrap/clock ceilings only sink (re-check tv-p1b M1, OR-TV-10): a raised
    column, a dropped row of a package still in the workspace, an unreadable row
    or a deleted file is a loosening; a lower pin or a new package row is free."""
    def rows(text, widths):
        got, bad = {}, []
        for raw in text.splitlines():
            line = raw.split("#", 1)[0].split()
            if not line:
                continue
            if len(line) not in widths or not all(n.isdigit() for n in line[1:]):
                bad.append(line[0])
                continue
            got[line[0]] = dict(zip(("unwrap", "mono", "wall"), map(int, line[1:])))
        return got, bad
    # the base may still carry the one-column form of GH #115 (unwrap only)
    (before, _), (after, bad) = rows(old, (2, 4)), rows(new, (4,))
    if not after and not bad:
        return ["budget deleted or empty"]
    live = _workspace_packages(repo) if repo else None
    out = ["row %s unreadable" % n for n in bad]
    for name, pins in sorted(before.items()):
        now = after.get(name)
        if now is None:
            if name not in bad and (live is None or name in live):
                out.append("row %s removed" % name)
            continue
        out += ["%s %s %s -> %s" % (name, col, pins[col], now[col])
                for col in pins if now[col] > pins[col]]
    return out


def _baseline_loosenings(old, new):
    """The baseline only sinks (re-check tv-p0 A4): a raised count, a removed
    count, or a deleted/unreadable file is a loosening; a lower count or the
    count of a new ban is free."""
    def counts(text):
        try:
            doc = json.loads(text)
        except ValueError:
            return None
        got = doc.get("counts") if isinstance(doc, dict) else None
        return got if isinstance(got, dict) else None
    before, after = counts(old) or {}, counts(new)
    if after is None:
        return ["baseline deleted or unreadable"]
    out = []
    for k, v in sorted(before.items()):
        if k not in after:
            out.append("count %s removed" % k)
        elif not isinstance(after[k], int) or after[k] > v:
            out.append("count %s %s -> %s" % (k, v, after[k]))
    return out


def _code_loosenings(old, new):
    """Any change of the ban code: a loosening there is not machine-readable
    (re-check tv-p0 A5), so the change itself needs the ruling."""
    return ["code changed"] if old != new else []


def _section_loosenings(old, new):
    before, after = _section_11(old), _section_11(new)
    if before is None:
        return []          # introducing the section is not a change of it
    return ["§ 11 changed"] if before != after else []


RESERVED = (
    (BANS_FILE, _ban_loosenings),
    (LEGACY_FILE, _legacy_loosenings),
    (RUFF_FILE, _ruff_loosenings),
    (CLOCK_BANS_FILE, _clock_loosenings),
    (RULES_DOC, _section_loosenings),
    (BASELINE_FILE, _baseline_loosenings),
    (BUDGET_FILE, _budget_loosenings),
    (CODE_FILE, _code_loosenings),
)


def ruling_ids(value):
    """The ids of `MECLAW_TESTING_RULES_RULING`: one or more, separated by commas
    or blanks (R-TV-W21 d) -- a diff with two reserved files under two rulings."""
    return [i for i in re.split(r"[,\s]+", value or "") if i]


def _carrier(register, ids, rel, today):
    """(why not, None) or (None, the id that carries `rel`): every id holds in
    the register (an unknown or expired one is RED), one of them names `rel`."""
    if not ids:
        return "no %s" % RULING_ENV, None
    bad = [w for w in (ruling_problem(register, i, today=today) for i in ids) if w]
    if bad:
        return "; ".join(bad), None
    for i in ids:
        if ruling_problem(register, i, loosens=rel, today=today) is None:
            return None, i
    if len(ids) == 1:
        return ruling_problem(register, ids[0], loosens=rel, today=today), None
    return "none of %s names %s under `loosens`" % (", ".join(ids), rel), None


def rules(files, repo, base=_UNSET, mode=None, env=None, today=None):
    """The `testing-rules` findings: the change reservation of VG § 5.7."""
    repo = str(repo)
    env = os.environ if env is None else env
    mode = os.environ.get("MECLAW_GATE_MODE", "") if mode is None else mode
    touched = [p for p in files if p == REGISTER_FILE or any(p == f for f, _ in RESERVED)]
    if not touched:
        return []
    if base is _UNSET:
        base = merge_base(repo)
    if not base:
        return [Finding("RED", "testing-rules", "%s: no merge base with master or main -- "
                        "the change reservation cannot be checked" % ", ".join(touched))]
    out = []
    if REGISTER_FILE in touched and _unchanged(repo, base, REGISTER_FILE):
        touched.remove(REGISTER_FILE)   # offered (whole tree), not changed by the diff
    if REGISTER_FILE in touched:
        if mode in ("integration", "release"):
            log = _git(repo, "log", "--format=%H", "%s..HEAD" % base, "--", REGISTER_FILE)
            if log is None:
                out.append(Finding("RED", "testing-rules", "%s: git log %s..HEAD failed -- the "
                                   "register commits cannot be checked" % (REGISTER_FILE, base)))
            for rev in (log or "").split():
                names = _git(repo, "show", "--format=", "--name-only", rev)
                if names is None:
                    out.append(Finding("RED", "testing-rules", "%s: git show %s failed -- the "
                                       "register commit cannot be checked" % (
                                           REGISTER_FILE, rev[:9])))
                    continue
                others = [n for n in names.split() if n != REGISTER_FILE]
                if others:
                    out.append(Finding("RED", "testing-rules", "%s: the register commit %s "
                                       "changes other files as well (%s)" % (
                                           REGISTER_FILE, rev[:9], ", ".join(others[:5]))))
        else:
            out.append(Finding("RED", "testing-rules", "%s: a strand never changes the register "
                               "-- the owner or the coordinator commits it on master" % REGISTER_FILE))
    ids = ruling_ids(env.get(RULING_ENV))
    register = _UNSET
    for rel, judge in RESERVED:
        if rel not in touched:
            continue
        born = _born(repo, base, rel)
        if born:
            continue       # the file is born in this diff: nothing to loosen
        old = None if born is None else _show(repo, base, rel)
        if old is None:
            out.append(Finding("RED", "testing-rules", "%s: git cannot read it at %s -- the "
                               "change reservation fails closed" % (rel, base)))
            continue
        try:
            with open(os.path.join(repo, rel), encoding="utf-8") as fh:
                new = fh.read()
        except OSError:
            new = ""
        loose = judge(old, new, repo) if rel == BUDGET_FILE else judge(old, new)
        if not loose:
            continue
        if register is _UNSET:
            register = load_register(repo)
        why, rid = _carrier(register, ids, rel, today)
        what = "; ".join(loose[:6])
        if why:
            out.append(Finding("RED", "testing-rules", "%s loosened (%s) -- an owner ruling "
                               "only: %s" % (rel, what, why)))
        else:
            out.append(Finding("NOTE", "testing-rules", "%s loosened (%s) under %s" % (
                rel, what, rid)))
    return out


# --- inventory -----------------------------------------------------------------------------

def _tree_files(repo):
    out = _git(repo, "ls-files", "--cached", "--others", "--exclude-standard")
    if out is not None:
        return sorted(set(out.splitlines()))
    found = []
    for dirpath, _dirs, names in os.walk(repo):
        for n in names:
            found.append(os.path.relpath(os.path.join(dirpath, n), repo).replace(os.sep, "/"))
    return sorted(found)


def inventory(repo):
    """{ban id: count of statements over the whole tree that the ban hits}."""
    repo = str(repo)
    bans = load_bans(repo)
    counts = {b["id"]: 0 for b in bans}
    for path in _tree_files(repo):
        mine = [b for b in bans if in_scope(b, path)]
        if not mine:
            continue
        src = _read_source(repo, path, mine)
        if src is None:
            continue
        for ban, _line, _span in findings_in(src, path, mine):
            counts[ban["id"]] += 1
    return counts


def read_baseline(repo):
    try:
        with open(os.path.join(str(repo), BASELINE_FILE), encoding="utf-8") as fh:
            return json.load(fh).get("counts", {})
    except (OSError, ValueError):
        return {}


def write_baseline(repo, counts):
    """Lower existing numbers, set new bans to their count, never raise (R-TV-W8)."""
    old = read_baseline(repo)
    merged = {k: (min(v, old[k]) if k in old else v) for k, v in counts.items()}
    doc = {"note": "testing-bans inventory baseline: counts only shrink; written by "
                   "`scripts/testing_bans.py --inventory --write` at a wave end (R-TV-W8)",
           "counts": dict(sorted(merged.items()))}
    path = os.path.join(str(repo), BASELINE_FILE)
    with open(path, "w", encoding="utf-8") as fh:
        json.dump(doc, fh, indent=2)
        fh.write("\n")
    return merged


def check_inventory(repo):
    """(exit code, summary line): 1 when a count rose over the baseline."""
    counts, base = inventory(repo), read_baseline(repo)
    rose = ["%s %d > %d" % (k, v, base[k]) for k, v in sorted(counts.items())
            if k in base and v > base[k]]
    # a ban without a baseline count is unchecked: RED, not silent (re-check tv-p0 A4)
    rose += ["%s %d, no baseline count" % (k, v) for k, v in sorted(counts.items())
             if k not in base]
    total = sum(counts.values())
    if rose:
        return 1, "TEST-BANS-INVENTORY rose over the baseline: %s" % ", ".join(rose)
    return 0, "TEST-BANS-INVENTORY %d statements under %d bans, none over the baseline" % (
        total, len(counts))


# --- cli -------------------------------------------------------------------------------------

def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0],
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--repo", default=os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
    ap.add_argument("--files", nargs="*", metavar="PATH")
    ap.add_argument("--inventory", action="store_true")
    how = ap.add_mutually_exclusive_group()
    how.add_argument("--write", action="store_true")
    how.add_argument("--check", action="store_true")
    args = ap.parse_args(argv)
    if args.inventory:
        if args.check:
            rc, line = check_inventory(args.repo)
            for k, v in sorted(inventory(args.repo).items()):
                print("%-22s %d" % (k, v))
            print(line)
            return rc
        counts = inventory(args.repo)
        for k, v in sorted(counts.items()):
            print("%-22s %d" % (k, v))
        print("total %d" % sum(counts.values()))
        if args.write:
            write_baseline(args.repo, counts)
        return 0
    findings = scan(args.files or [], args.repo) + rules(args.files or [], args.repo)
    for f in findings:
        print("%s %s: %s" % f)
    return 1 if any(f.level == "RED" for f in findings) else 0


if __name__ == "__main__":
    sys.exit(main())
