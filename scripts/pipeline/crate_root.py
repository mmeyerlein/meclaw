"""Crate roots in the narrowing: which edits of `lib.rs`/`main.rs`/`mod.rs`
leave the crate's coverage map valid (welle-tb S1 part A, GH #1159).

WHY THIS EXISTS
===============
A crate root that only declares modules has no executed line in the coverage
map, so `impact` could not replace `rdeps(X)` for it: the measured case lib.rs
planned 8727 tests where the same edit in cell.rs planned 4056 (HO V2), and
the zk-fix case (one `//` line in lib.rs) fell to rdeps for the same reason.

The rule is a POSITIVE list, fail-closed: the edit is harmless to the map only
when, after dropping whitespace, non-doc comments (`//`, `/* */`) and the
bodies of non-`const` functions, the old and the new file are the SAME token
stream. Everything else -- `mod`, `#[path]`, `include!`, `use`, `pub use`, doc
comments (doctests), module-level items (const, static, type, struct, enum,
trait, impl head, `macro_rules!`), attributes incl. `#![...]`/`#[cfg]`,
`extern crate`, a `const fn` body -- changes the skeleton and keeps rdeps.

A function body is collapsed only where the `fn` is an item: at file level or
directly inside `mod {}`, `impl {}`, `trait {}`, `extern {}`. A body that
holds an `impl` (crate-wide, coherence) or a linkage attribute
(`no_mangle`, `export_name`, `link_section`, `used`) is not collapsed, and
neither is anything inside a macro invocation or `macro_rules!`. A `{` inside
the generics of a signature (`<{N}>`) or a token the lexer does not know is
doubt: `skeleton` returns None and the caller keeps rdeps.
"""

import os
import re
import subprocess

ROOT_NAMES = ("lib.rs", "main.rs", "mod.rs")

# Rust identifiers are Unicode (XID_Start/XID_Continue). `[^\W\d]\w*` is a
# superset; `_word_at` trims a non-ASCII match to what `str.isidentifier`
# (XID as well) takes. ASCII only, every file with `fn h\u00ebper` was doubt --
# valid Rust read as "unknown character" (re-check tb-s4-st-h2 L-b).
_IDENT = re.compile(r"(?:r#)?[^\W\d]\w*")
_NUMBER = re.compile(r"[0-9][0-9A-Za-z_.]*")
_CHAR = re.compile(r"'(?:\\(?:x[0-9a-fA-F]{2}|u\{[0-9a-fA-F_]{1,6}\}|.)|[^\\'\n])'")
_LIFETIME = re.compile(r"'(?:r#)?[^\W\d]\w*")
_RAW_STR = re.compile(r'(?:b|c)?r(#*)"')
_STR = re.compile(r'(?:b|c)?"(?:\\[\s\S]|[^"\\])*"')
_PUNCT2 = ("::", "->", "=>", "==", "!=", "<=", ">=", "&&", "||", "..", "+=", "-=",
           "*=", "/=", "%=", "^=", "&=", "|=", "<<", ">>")
_CONTAINERS = ("mod", "impl", "trait", "extern")
_FN_QUALIFIERS = {"pub", "crate", "self", "super", "in", "const", "async", "unsafe",
                  "extern", "default", "(", ")"}
_LINKAGE = ("no_mangle", "export_name", "link_section", "used")
# Tokens that make a fn body reach beyond the fn (re-check H1, M2): a macro
# exported crate-wide, a module or a file pulled in, linkage under cfg_attr,
# and macros that emit crate items (re-check m-N4: `global_asm!` a symbol,
# `inventory::submit!` a registry entry). Item attributes such as linkme's
# `#[distributed_slice]` or `#[ctor]` fail `_BODY_ATTRS`; any other macro that
# expands to items is a known blind spot (0 in the tree, 10.10.2026).
_CRATE_WIDE = {"impl", "macro_rules", "macro_export", "mod", "include", "cfg_attr", "extern",
               "global_asm", "inventory"}
# The only attributes a collapsible body may carry: they stay inside it.
_BODY_ATTRS = {"allow", "expect", "warn", "deny", "forbid", "cfg", "inline", "cold",
               "must_use", "rustfmt", "clippy", "track_caller"}


class Doubt(Exception):
    """The file holds a shape the skeleton does not know -- keep rdeps."""


def _word_at(rx, text, i):
    """The identifier (`_IDENT`) or lifetime (`_LIFETIME`) at `text[i]`, or None.

    A non-ASCII match is cut back to its longest prefix that is an identifier
    to `str.isidentifier`; what stays behind is read on (and doubted)."""
    m = rx.match(text, i)
    if not m:
        return None
    word = m.group(0)
    if word.isascii():
        return word
    pre = word[:len(word) - len(word.lstrip("'"))]
    pre += "r#" if word[len(pre):].startswith("r#") else ""
    body = word[len(pre):]
    k = len(body)
    while k and not body[:k].isidentifier():
        k -= 1
    if k:
        return pre + body[:k]
    return "r" if pre == "r#" else None


def is_crate_root(path):
    return path.endswith(".rs") and path.rsplit("/", 1)[-1] in ROOT_NAMES


def tokens(text):
    """The token list of `text` without whitespace and non-doc comments.

    Doc comments stay as ONE token each (a doctest is code). Raises Doubt on
    an unterminated comment or literal.

    Public and stable: other modules import it by this name and signature
    (`gate_plan._rust_code` moves onto it, welle-tb S4) -- do not rename.
    """
    return [t for _i, t in token_spans(text)]


def token_spans(text):
    """`[(offset, token)]` -- the tokens of `tokens`, each with the offset in
    `text` where it starts; same tokens, same doubts. A code token is
    `text[offset:offset + len(token)]`; a line doc comment is that slice
    right-stripped. For a reader that must keep lines (the browser marker
    rule, `gate_plan._rust_code_aligned`, GH #1161 x #1162).
    """
    out, i, n = [], 0, len(text)
    while i < n:
        c = text[i]
        if c.isspace():
            i += 1
            continue
        if text.startswith("//", i):
            end = text.find("\n", i)
            end = n if end < 0 else end
            body = text[i:end]
            # `///x` and `//!x` are docs; `////` is a plain comment
            if (body.startswith("///") and not body.startswith("////")) or body.startswith("//!"):
                out.append((i, body.rstrip()))
            i = end
            continue
        if text.startswith("/*", i):
            depth, j = 1, i + 2
            while j < n and depth:
                if text.startswith("/*", j):
                    depth, j = depth + 1, j + 2
                elif text.startswith("*/", j):
                    depth, j = depth - 1, j + 2
                else:
                    j += 1
            if depth:
                raise Doubt("unterminated block comment")
            body = text[i:j]
            if (body.startswith("/**") and not body.startswith("/***") and body != "/**/") \
                    or body.startswith("/*!"):
                out.append((i, body))
            i = j
            continue
        m = _RAW_STR.match(text, i)
        if m:
            close = '"' + m.group(1)
            end = text.find(close, m.end())
            if end < 0:
                raise Doubt("unterminated raw string")
            out.append((i, text[i:end + len(close)]))
            i = end + len(close)
            continue
        m = _STR.match(text, i)
        if m:
            out.append((i, m.group(0)))
            i = m.end()
            continue
        if c == '"':
            raise Doubt("unterminated string at %d" % i)
        if c == "'":
            m = _CHAR.match(text, i)
            word = m.group(0) if m else _word_at(_LIFETIME, text, i)
            if not word:
                raise Doubt("stray quote at %d" % i)
            out.append((i, word))
            i += len(word)
            continue
        word = _word_at(_IDENT, text, i)
        if word:
            out.append((i, word))
            i += len(word)
            continue
        m = _NUMBER.match(text, i)
        if m:
            out.append((i, m.group(0)))
            i = m.end()
            continue
        two = text[i:i + 2]
        if two in _PUNCT2:
            out.append((i, two))
            i += 2
            continue
        if ord(c) < 128:
            out.append((i, c))
            i += 1
            continue
        raise Doubt("unknown character %r" % c)
    return out


def _match_close(toks, i):
    """Index of the bracket closing `toks[i]`."""
    pairs = {"{": "}", "(": ")", "[": "]"}
    stack = [pairs[toks[i]]]
    j = i + 1
    while j < len(toks):
        t = toks[j]
        if t in pairs:
            stack.append(pairs[t])
        elif t in (")", "]", "}"):
            if not stack or t != stack[-1]:
                raise Doubt("unbalanced %s" % t)
            stack.pop()
            if not stack:
                return j
        j += 1
    raise Doubt("unclosed %s" % toks[i])


def _is_const_fn(toks, fn_at, start):
    """Does a `const` qualify the `fn` at `fn_at` (looking back to `start`)?"""
    j = fn_at - 1
    while j >= start:
        t = toks[j]
        if t == "const":
            return True
        if t in _FN_QUALIFIERS or t.startswith('"'):
            j -= 1
            continue
        break
    return False


def _body_open(toks, i, end):
    """Index of the `{` opening the body of the fn whose name is at `i`, or
    None for a declaration without body (`;`)."""
    angle = paren = 0
    j = i + 1
    while j < end:
        t = toks[j]
        if t in ("(", "["):
            paren += 1
        elif t in (")", "]"):
            paren -= 1
        elif t == "<":
            angle += 1
        elif t == ">" and angle:
            angle -= 1
        elif t == ">>" and angle:
            angle = max(0, angle - 2)
        elif t == "{":
            if angle or paren:
                raise Doubt("brace inside a signature")
            return j
        elif t == ";" and not paren:
            return None
        j += 1
    raise Doubt("fn without body or semicolon")


def _body_is_opaque(body):
    """A body we never collapse: crate-wide items, linkage or any attribute
    outside `_BODY_ATTRS` inside it (fail-closed)."""
    if _CRATE_WIDE.intersection(body):
        return True
    for k, t in enumerate(body):
        if t == "#" and k + 1 < len(body) and body[k + 1] in ("[", "!"):
            window = body[k + 1:k + 6]
            if any(w in _LINKAGE for w in window):
                return True
            j = k + 1 + (body[k + 1] == "!")
            if j >= len(body) or body[j] != "[" or j + 1 >= len(body) \
                    or body[j + 1] not in _BODY_ATTRS:
                return True
    return False


def _skeleton(toks, start, end):
    out, i = [], start
    while i < end:
        t = toks[i]
        if t == "fn" and i + 1 < end and _IDENT.fullmatch(toks[i + 1]):
            o = _body_open(toks, i + 1, end)
            if o is None:
                out.extend(toks[i:i + 2])
                i += 2
                continue
            close = _match_close(toks, o)
            body = toks[o + 1:close]
            out.extend(toks[i:o])
            if _is_const_fn(toks, i, start) or _body_is_opaque(body):
                out.extend(toks[o:close + 1])
            else:
                out.extend(["{", "<fn-body>", "}"])
            i = close + 1
            continue
        if t in _CONTAINERS:
            # the container's own `{` is the next brace before a `;`
            j = i + 1
            while j < end and toks[j] not in ("{", ";", "fn"):
                if toks[j] in ("(", "["):
                    j = _match_close(toks, j)
                j += 1
            if j < end and toks[j] == "{" and toks[j - 1] != "!":   # `extern "C" fn` is no block
                close = _match_close(toks, j)
                out.extend(toks[i:j + 1])
                out.extend(_skeleton(toks, j + 1, close))
                out.append("}")
                i = close + 1
                continue
            out.extend(toks[i:j])
            i = j
            continue
        if t in ("{", "(", "["):
            close = _match_close(toks, i)   # verbatim: struct bodies, macros, consts
            out.extend(toks[i:close + 1])
            i = close + 1
            continue
        out.append(t)
        i += 1
    return out


def skeleton(text):
    """The token stream of `text` with non-const fn bodies collapsed; None in doubt."""
    try:
        toks = tokens(text)
        return _skeleton(toks, 0, len(toks))
    except Doubt:
        return None


def body_only_change(old, new):
    """True when `old` -> `new` changes only whitespace, non-doc comments and
    bodies of non-const fns (the positive list); False otherwise or in doubt."""
    if old is None or new is None:
        return False
    a, b = skeleton(old), skeleton(new)
    return a is not None and b is not None and a == b


# The last path segment of a test-making attribute (re-check H1, m4): a name
# that merely holds `test` (`#[attestation]`, `#[test_helper]`) is none.
_TEST_ATTR = re.compile(r"test|rstest|test_case|quickcheck|proptest|wasm_bindgen_test")


def test_attribute_count(text):
    """How many attributes of `text` make a test (`#[test]`, `#[tokio::test(..)]`,
    `#[rstest]`, ...: the last path segment holds `test`); None in doubt.

    The token lexer, not `gate_plan._rust_code`: that regex lexer loses its
    phase on `"'in_tool'"` inside `json!` (measured on
    `meclaw-cells/tests/misc_1/cogny_template.rs`: 4 tests, counted 0) --
    125 of 1184 gone graph names would have read "hits no test".
    """
    try:
        toks = tokens(text)
    except Doubt:
        return None
    n = 0
    for k in range(len(toks) - 2):
        if toks[k] != "#" or toks[k + 1] != "[":
            continue
        j, last = k + 2, None
        while j < len(toks) and (_IDENT.fullmatch(toks[j]) or toks[j] == "::"):
            if toks[j] != "::":
                last = toks[j]
            j += 1
        if last and _TEST_ATTR.fullmatch(last):
            n += 1
    return n


def _show(repo, rev, path):
    res = subprocess.run(["git", "-C", str(repo), "show", "%s:%s" % (rev, path)],
                         capture_output=True, text=True, check=False)
    return res.stdout if res.returncode == 0 else None


def tree_text(repo, path):
    """`path` as the working tree of `repo` holds it; None when missing."""
    try:
        with open(os.path.join(str(repo), path), encoding="utf-8") as fh:
            return fh.read()
    except (OSError, UnicodeDecodeError):
        return None


def root_change_is_harmless(repo, rev, path, head="HEAD", tree=False):
    """The positive list for crate root `path` between the map's `rev` and
    `head` (the same span `narrow.map_fits` diffs). With `tree` the new side
    is the working tree -- what a plan that reads the dirty tree tests
    (re-check H1, B1: before the commit HEAD IS the map's rev). False in
    doubt: no repo, no rev, the file missing on either side."""
    if repo is None or not rev or not is_crate_root(path):
        return False
    new = tree_text(repo, path) if tree else _show(repo, head, path)
    return body_only_change(_show(repo, rev, path), new)
