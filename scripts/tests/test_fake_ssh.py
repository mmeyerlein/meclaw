"""Pins of the one environment rule of the kit tests' fake `ssh` (GH #1209).

`scripts/tests/fake_ssh.py` derives the names a fake `ssh` drops from the
transport's own lists. Here: every fake of the kit tests is built from it, a
name of the list never reaches the remote side of any of them, and a NEW name
in `lane_sync.sh VERDICT_ENV` reaches every user without touching one of them
(re-check tv-bn M7, M3: the hand copy in `test_pipeline_lane` would have let a
fifth name through, and its transport pin would have been green without a
transport).

Re-check tv-bn M7c: the sweep for a NEW fake knew one spelling
(`exec bash -c "$*"`) in one directory (M2) -- it now reads every kit test
place (Python and shell, nested) and every form that runs the arguments here;
and the rule held a hand copy of the brake while `lane_env` carried more names
by word than the two lists named (M3) -- `lane_sync.sh LANE_WORDS` is now the
third list, read here and by `lane_env`.

Re-check tv-bn M7d: the sweep still was a denylist of forms and places (M2,
10 of 10 probe mutants unseen) -- it is inverted now: every place of the
tracked files that writes an `ssh` is a candidate whatever its body, green
only when built from `fake_ssh.script` or marked HARMLESS with a reason; the
deadline pin read one form (M3); and the word check stood at two producers
only (B-H1) -- it sits at the narrow point `remote`/`remote_long` now.

Re-check tv-bn M7e (orchestrator decision M7f): three rounds of the sweep
each showed new forms -- a list of forms does not converge. The GUARANTEE is
at run time now: `fake_ssh.install`/`install_inert` is the one maker of a
fake `ssh` and registers it (`MECLAW_FAKE_SSH_REGISTRY`); the narrow point
refuses an `ssh` on PATH the registry lacks (pins: the maker's contract, and
the eleven writers of A-M1 run for real and are not admitted). The SWEEP
below is the early warning beside it, cheap and red-leaning: every writer but
the maker is red (no marker, no credit over a function -- A-M3), a file that
does not decode is read as bytes and one that cannot be read is red (A-M2),
writers without a path form (A-M1) are candidates, the makers are EQUAL to
the lists (A-M4). Its rest: a writer whose `ssh` comes from outside the
module's text (computed, read from a file, a variable across modules) is not
seen -- the registry catches it. The deadline pin reads every Python file of
`scripts/tests/` (A-M5) and names its rest (DEADLINE_DATA).
"""

import ast
import importlib
import os
import pathlib
import re
import shlex
import shutil
import stat
import subprocess
import sys
import tempfile
import tomllib
import unittest
import unittest.mock

from scripts.pipeline import lane
from scripts.tests import fake_ssh, kit_time, kit_tree, private_tree, side_effect_guard, store_isolation

# lane_sync.sh's functions in a file beside the kit libraries they source
# (M7w): sourced as `source "$0"`, never pasted into a `bash -c`.
_FNS_DIR = tempfile.TemporaryDirectory(prefix="fake-ssh-fns-")


def _fns():
    path = pathlib.Path(_FNS_DIR.name) / "lane_sync.sh"
    if not path.exists():
        kit_tree.functions(_FNS_DIR.name)
    return str(path)

TESTS = pathlib.Path(__file__).resolve().parent
REPO = TESTS.parents[1]

# (module, template, script, a host the fake answers for)
USERS = (
    ("scripts.tests.test_pipeline_lane", "FAKE_SSH_TEMPLATE", "FAKE_SSH", "lane-a"),
    ("scripts.tests.test_strand_sh", "FAKE_SSH_TEMPLATE", "FAKE_SSH", "north"),
    ("scripts.tests.test_pipeline_pre", "FAKE_SSH_TEMPLATE", "FAKE_SSH", "u@lane"),
    ("scripts.tests.test_pipeline_gate_cli", "PASS_SSH_TEMPLATE", "PASS_SSH", "lane-l"),
)

_SSH_NAME = re.compile(r"ssh", re.I)

# The modules that make a fake with the rule from a template of their own
# (USERS), the ones that make it from a USERS template they import, and the
# ones that make an inert fake (`fake_ssh.install_inert`). Re-check M7e A-M4:
# the makers of the tree are EQUAL to these lists, never a superset -- a new
# maker is a diff here, and so it reaches the USERS pins.
IMPORTERS = (("scripts.tests.test_pipeline_ir_path", "scripts.tests.test_pipeline_gate_cli", "PASS_SSH_TEMPLATE"),
             ("scripts.tests.test_pipeline_two_phase_build", "scripts.tests.test_pipeline_gate_cli",
              "PASS_SSH_TEMPLATE"))
INERT = ("scripts.tests.side_effect_guard", "scripts.tests.test_lane_engstelle",
         "scripts.tests.test_pipeline_gate_cli", "scripts.tests.test_pipeline_hygiene",
         "scripts.tests.test_strand_sh")

# The scan set: every tracked or new file (`git ls-files`) but the prose of
# the repository (top directories) and the two files of the rule itself (they
# name the forms).
PROSE_ROOTS = ("docs", "plans", "ideas")
OWN = ("scripts/tests/fake_ssh.py", "scripts/tests/test_fake_ssh.py")

# In a shell text (any file but Python, and every string of a Python file):
# a path on `ssh` (`"$d/ssh"`, `bin/ssh`), a quoted `ssh` (a path part:
# `dir.join("ssh")`), a redirect into `ssh`; the bare word `ssh` as the target
# of `ln`/`cp`/`install`/`mv`/`tee`; a loop or an assignment that binds the
# word `ssh` to a name the text then puts on a path (`/$t`, `/${n}`).
_TEXT_SITE = re.compile(r"""/ssh(?![\w.\-/])|["']ssh["']|>>?[ \t]*["']?ssh\b(?![\w.\-/])""")
_BARE_TARGET = re.compile(r"""\b(?:ln|cp|install|mv|tee)\b[^\n;|&]*?[ \t]["']?ssh["']?(?=[ \t]*(?:$|[;&|)]))""",
                          re.M)
_SH_BIND = re.compile(r"""(?:\bfor[ \t]+(\w+)[ \t]+in\b[^\n;]*?[ \t]|\b(\w+)=)["']?ssh["']?(?![\w.\-/])""")
_PATH_CALLS = {"join", "joinpath", "Path", "PurePath", "PosixPath", "PurePosixPath", "with_name", "open"}
_ITER_CALLS = {"tuple", "list", "set", "sorted", "dict", "zip", "enumerate", "reversed", "items",
               "keys", "values", "chain"}
_ARGV_WRITERS = {"ln", "cp", "install", "mv", "tee"}
_MAKERS = {"install": "rule", "install_inert": "inert"}
# Re-check M7e A-M1/runtime: who writes the registry beside the maker makes a
# fake the narrow point admits. In a test file that is a bypass.
_BYPASS = re.compile(r"MECLAW_FAKE_SSH_REGISTRY|\bfake_ssh\s*\.\s*_(?:register|put)\b")


def _text_sites(text):
    """The 1-based lines of a shell text that may write an `ssh`."""
    lines = set()
    for rx in (_TEXT_SITE, _BARE_TARGET):
        lines |= {text.count("\n", 0, m.start()) + 1 for m in rx.finditer(text)}
    for m in _SH_BIND.finditer(text):
        name = m.group(1) or m.group(2)
        if re.search(r"/\$\{?%s\b" % re.escape(name), text):
            lines.add(text.count("\n", 0, m.start()) + 1)
    return sorted(lines)


# The narrow point itself READS the `ssh` of each PATH and system directory
# (re-check tv-bn M7j A-M1/A-M3: it resolves the path by its own walk with
# `[[ -f $d/ssh ]]` instead of `command -v`, which an exported function
# replaces). Inside `lane_sync.sh ssh_bin`, a line with a path on `ssh` and
# without a redirect or a writing command is such a read; a write there stays
# red (`test_a_write_inside_the_narrow_point_stays_red`).
NARROW_POINT = {"scripts/lane_sync.sh": "ssh_bin"}
_WRITE_FORM = re.compile(r">|\b(?:ln|cp|install|mv|tee|dd|printf|echo|cat)\b")


def _narrow_point_reads(rel, text):
    """The lines of `rel`'s narrow-point function that only read an `ssh`."""
    func = NARROW_POINT.get(rel)
    if func is None:
        return set()
    lines = text.splitlines()
    try:
        first = lines.index("%s() {" % func)
        last = lines.index("}", first)
    except ValueError:
        return set()
    return {i + 1 for i in range(first + 1, last) if not _WRITE_FORM.search(lines[i])}


def tracked(root):
    """The scan set of `root`: `git ls-files` with the files not yet added
    (re-check M7e H1), or -- not a git tree (an unpacked export) -- every
    file below it, which is more, never less."""
    root = pathlib.Path(root)
    res = subprocess.run(["git", "-C", str(root), "ls-files", "-z", "--cached", "--others", "--exclude-standard"],
                         capture_output=True, timeout=kit_time.HANG)
    if res.returncode == 0:
        rels = [r for r in res.stdout.decode("utf-8", "surrogateescape").split("\0") if r]
    else:
        rels = [p.relative_to(root).as_posix() for p in root.rglob("*") if (p.is_file() or p.is_symlink())
                and not {".git", "target", "node_modules", "__pycache__"} & set(p.parts)]
    return sorted({r for r in rels if r.split("/", 1)[0] not in PROSE_ROOTS and r not in OWN
                   and not pathlib.PurePosixPath(r).name.startswith(".env")
                   and pathlib.PurePosixPath(r).name not in ("vault.env", "vault-pass.txt")})


def _call_name(call):
    f = call.func
    return f.id if isinstance(f, ast.Name) else f.attr if isinstance(f, ast.Attribute) else None


def _is_rule_call(n, names=("script", "install")):
    """`fake_ssh.script(...)` / `fake_ssh.install(...)` -- the one rule."""
    return isinstance(n, ast.Call) and isinstance(n.func, ast.Attribute) and n.func.attr in names \
        and isinstance(n.func.value, ast.Name) and n.func.value.id == "fake_ssh"


def _ids(node):
    return {n.id if isinstance(n, ast.Name) else n.attr for n in ast.walk(node)
            if isinstance(n, (ast.Name, ast.Attribute))}


def _bound(t):
    """The identifiers a binding target binds (`self.x` binds `x`)."""
    if isinstance(t, ast.Name):
        return {t.id}
    if isinstance(t, ast.Attribute):
        return {t.attr}
    if isinstance(t, (ast.Tuple, ast.List)):
        return set().union(*map(_bound, t.elts)) if t.elts else set()
    if isinstance(t, ast.Starred):
        return _bound(t.value)
    return set()


def _bindings(tree):
    """(target, value) of every binding: assignments (element-wise for a
    tuple on both sides), loop and comprehension targets (to their
    iterable), `with ... as`, `:=`, and parameter defaults."""
    out = []
    for n in ast.walk(tree):
        if isinstance(n, ast.Assign):
            pairs = [(t, n.value) for t in n.targets]
        elif isinstance(n, (ast.AnnAssign, ast.AugAssign)) and n.value is not None:
            pairs = [(n.target, n.value)]
        elif isinstance(n, (ast.For, ast.AsyncFor, ast.comprehension)):
            pairs = [(n.target, n.iter)]
        elif isinstance(n, ast.withitem) and n.optional_vars is not None:
            pairs = [(n.optional_vars, n.context_expr)]
        elif isinstance(n, ast.NamedExpr):
            pairs = [(n.target, n.value)]
        elif isinstance(n, ast.arguments):
            pos = n.posonlyargs + n.args
            pairs = [(ast.Name(id=a.arg), d) for a, d in zip(pos[len(pos) - len(n.defaults):], n.defaults)]
            pairs += [(ast.Name(id=a.arg), d) for a, d in zip(n.kwonlyargs, n.kw_defaults) if d is not None]
        else:
            continue
        loop = isinstance(n, (ast.For, ast.AsyncFor, ast.comprehension))
        for t, v in pairs:
            if not loop and isinstance(t, (ast.Tuple, ast.List)) and isinstance(v, (ast.Tuple, ast.List)) \
                    and len(t.elts) == len(v.elts):
                out.extend(zip(t.elts, v.elts))
            else:
                out.append((t, v))
    return out


class _PyModule:
    """One Python file of the sweep: what may hold `ssh`, what is a body,
    which templates the rule takes -- the facts its sites are judged by."""

    def __init__(self, rel, text, imported_ssh=frozenset()):
        self.rel, self.lines = rel, text.splitlines()
        self.tree = ast.parse(text)
        self.parent = {c: n for n in ast.walk(self.tree) for c in ast.iter_child_nodes(n)}
        binds = _bindings(self.tree)
        self.imported = {a.asname or a.name: n.module for n in ast.walk(self.tree)
                         if isinstance(n, ast.ImportFrom) and n.module for a in n.names}
        # `scal`: may be `ssh` or a path on it; `coll`: a collection that may hold one;
        # `params`: per function, its parameters a call hands `ssh` (re-check M7e A-M1);
        # `sfuncs`: functions that return `ssh`
        self.scal, self.coll = set(), set(imported_ssh) & _ids(self.tree)
        self.params, self.sfuncs, self.bodies = {}, set(), set()
        defs = {n.name: n for n in ast.walk(self.tree) if isinstance(n, (ast.FunctionDef, ast.AsyncFunctionDef))}
        loops = {id(v) for n in ast.walk(self.tree) if isinstance(n, (ast.For, ast.AsyncFor, ast.comprehension))
                 for v in (n.iter,)}
        changed = True
        while changed:   # to a fixed point: names that may hold `ssh`, bodies
            changed = False
            for t, v in binds:
                names, k = _bound(t), self.kind(v)
                if id(v) in loops:   # a loop's target: an element of what it walks
                    k = "s" if k == "c" else None
                for flag, into in ((k == "s", self.scal), (k == "c", self.coll), (self.has_body(v), self.bodies)):
                    if flag and not names <= into:
                        into |= names
                        changed = True
            for n in ast.walk(self.tree):
                if isinstance(n, ast.Call) and _call_name(n) in defs:
                    fn = defs[_call_name(n)]
                    pos = [a.arg for a in fn.args.posonlyargs + fn.args.args]
                    if pos and pos[0] in ("self", "cls") and isinstance(n.func, ast.Attribute):
                        pos = pos[1:]
                    got = {p for p, a in zip(pos, n.args) if self.kind(a) == "s"}
                    got |= {k.arg for k in n.keywords if k.arg and self.kind(k.value) == "s"}
                    into = self.params.setdefault(id(fn), set())
                    if not got <= into:
                        into |= got
                        changed = True
                elif isinstance(n, (ast.FunctionDef, ast.AsyncFunctionDef)) and n.name not in self.sfuncs:
                    if any(isinstance(r, ast.Return) and r.value is not None and self.kind(r.value) == "s"
                           for r in ast.walk(n)):
                        self.sfuncs.add(n.name)
                        changed = True
        self.templates = {a.id for n in ast.walk(self.tree) if _is_rule_call(n)
                          for a in n.args[1 if _call_name(n) == "install" else 0:][:1] if isinstance(a, ast.Name)}

    def _func(self, n):
        n = self.parent.get(n)
        while n is not None and not isinstance(n, (ast.FunctionDef, ast.AsyncFunctionDef)):
            n = self.parent.get(n)
        return n

    def kind(self, e):
        """"s": `e` may be `ssh` or a path on it; "c": a collection that may hold one."""
        if isinstance(e, ast.Constant):
            return "s" if isinstance(e.value, str) and (e.value == "ssh" or e.value.endswith("/ssh")) else None
        if isinstance(e, ast.JoinedStr):
            return self.kind(e.values[-1]) if e.values else None
        if isinstance(e, ast.Name) and any(e.id in self.params.get(id(f), ()) for f in self._funcs(e)):
            return "s"
        if isinstance(e, (ast.Name, ast.Attribute)):
            i = e.id if isinstance(e, ast.Name) else e.attr
            return "s" if i in self.scal else "c" if i in self.coll else None
        if isinstance(e, (ast.Tuple, ast.List, ast.Set)):
            return "c" if any(self.kind(x) for x in e.elts) else None
        if isinstance(e, ast.Dict):
            return "c" if any(k is not None and self.kind(k) for k in e.keys) else None
        if isinstance(e, ast.Starred):
            return self.kind(e.value)
        if isinstance(e, ast.Subscript):
            return "s" if self.kind(e.value) else None
        if isinstance(e, ast.IfExp):
            return self.kind(e.body) or self.kind(e.orelse)
        if isinstance(e, ast.BoolOp):
            return next((k for k in map(self.kind, e.values) if k), None)
        if isinstance(e, ast.BinOp) and isinstance(e.op, (ast.Div, ast.Add)):
            right, left = self.kind(e.right), self.kind(e.left)
            if right == "s":
                return "s"
            return "c" if isinstance(e.op, ast.Add) and "c" in (left, right) else None
        if isinstance(e, ast.Call) and _call_name(e) in self.sfuncs:
            return "s"
        if isinstance(e, ast.Call) and _call_name(e) in _PATH_CALLS - {"open"}:
            return "s" if any(self.kind(a) == "s" for a in e.args) else None
        if isinstance(e, ast.Call) and _call_name(e) in _ITER_CALLS:
            parts = list(e.args) + ([e.func.value] if isinstance(e.func, ast.Attribute) else [])
            return "c" if any(self.kind(a) for a in parts) else None
        if isinstance(e, (ast.ListComp, ast.SetComp, ast.GeneratorExp)):
            return "c" if self.kind(e.elt) else None
        return None

    def _funcs(self, n):
        f = self._func(n)
        while f is not None:
            yield f
            f = self._func(f)

    def has_body(self, e):
        """`e` holds a script: a `#!` text, a name bound to one, an imported
        constant (upper case: `PASS_SSH`; a module or a function is none)."""
        for x in ast.walk(e):
            if isinstance(x, ast.Constant) and isinstance(x.value, str) and x.value.startswith("#!"):
                return True
            if isinstance(x, ast.Name) and (x.id in self.bodies or x.id in self.imported and x.id.isupper()):
                return True
        return False

    def _harmless_context(self, n):
        up = self.parent.get(n)
        return isinstance(up, ast.Compare) or isinstance(up, ast.Call) and n in up.args \
            and _call_name(up) in ("startswith", "endswith", "get", "which")

    def _path_part(self, e):
        return self.kind(e) == "s"

    def makers(self):
        """[(line, 'rule'|'inert')]: the calls of the one maker."""
        return [(n.lineno, _MAKERS[n.func.attr]) for n in ast.walk(self.tree)
                if _is_rule_call(n, tuple(_MAKERS))]

    def sites(self):
        """[(line, kind, node)]: every place that may write an `ssh`."""
        out = []
        for n in ast.walk(self.tree):
            if isinstance(n, ast.BinOp) and self._path_part(n.right) and (
                    isinstance(n.op, ast.Div) or isinstance(n.op, ast.Add)
                    and isinstance(n.right, (ast.Constant, ast.JoinedStr))):
                out.append((n.lineno, "path", n))
            elif isinstance(n, ast.Call) and _call_name(n) in _PATH_CALLS \
                    and any(self._path_part(a) for a in n.args):
                out.append((n.lineno, "path", n))
            elif isinstance(n, ast.Constant) and isinstance(n.value, str):
                if n.value.endswith("/ssh") and not self._harmless_context(n):
                    out.append((n.lineno, "path", n))
                # a shell text in a Python string (re-check M7e A-M1: `sh -c 'cat >"$1/ssh"'`)
                elif "\n" in n.value or "$" in n.value:
                    out += [(n.lineno + i - 1, "text", n) for i in _text_sites(n.value)]
            elif isinstance(n, (ast.List, ast.Tuple)) and n.elts and isinstance(n.elts[0], ast.Constant) \
                    and n.elts[0].value in _ARGV_WRITERS and any(self.kind(x) == "s" for x in n.elts[1:]):
                out.append((n.lineno, "argv", n))   # `["ln", "-s", stub, "ssh"]`
            if isinstance(n, ast.Call) and _call_name(n) not in _PATH_CALLS:
                args = list(n.args) + [k.value for k in n.keywords]
                if any(self.kind(a) == "s" for a in args) and any(self.has_body(a) for a in args):
                    out.append((n.lineno, "named", n))
            elif isinstance(n, ast.Dict):
                if any(k is not None and self.kind(k) == "s" and self.has_body(v)
                       for k, v in zip(n.keys, n.values)):
                    out.append((n.lineno, "named", n))
            elif isinstance(n, (ast.Assign, ast.AnnAssign)) and n.value is not None:
                targets = n.targets if isinstance(n, ast.Assign) else [n.target]
                if any(_SSH_NAME.search(i) for t in targets for i in _ids(t)) and any(
                        isinstance(x, ast.Constant) and isinstance(x.value, str) and x.value.startswith("#!")
                        for x in ast.walk(n.value)):
                    out.append((n.lineno, "body", n))
        return out

    def verdict(self, kind, node):
        """'rule' (a template the maker takes, the marker once in it) or 'red'.
        Re-check M7e A-M3: no marker and no credit over a function any more
        -- every other place that writes an `ssh` is red, the maker writes."""
        if kind == "body":
            texts = [x.value for x in ast.walk(node.value) if isinstance(x, ast.Constant)
                     and isinstance(x.value, str)]
            targets = node.targets if isinstance(node, ast.Assign) else [node.target]
            if set().union(*map(_ids, targets)) & self.templates \
                    and sum(t.count(fake_ssh.MARKER) for t in texts) == 1:
                return "rule"
        return "red"


def sweep(root, files=None):
    """`[(rel, line, kind, verdict)]` of every place in the files of `root`
    that may write a fake `ssh`, and of every call of the maker (kind
    'rule'/'inert'). The early warning beside the run-time guarantee (the
    narrow point refuses an `ssh` the maker did not register): a writer is
    red whatever its body or place; green are only the templates the maker
    takes. Re-check M7e A-M2: a file that does not decode is scanned as
    bytes (`surrogateescape`), a file that cannot be read is red."""
    root = pathlib.Path(root)
    texts, mods, out = {}, {}, []
    for rel in (tracked(root) if files is None else files):
        try:
            texts[rel] = (root / rel).read_bytes().decode("utf-8", "surrogateescape")
        except OSError:
            out.append((rel, 0, "unreadable", "red"))
    for rel, text in texts.items():
        if rel.endswith(".py") and "ssh" in text.lower():
            try:
                mods[rel] = _PyModule(rel, text)
            except (SyntaxError, ValueError, UnicodeError):
                pass   # unparsable: read as text below
    # names a module exports holding `ssh` (`side_effect_guard.BLOCKED`) reach the files that name them
    exported = set().union(*(m.coll & {n for s in m.tree.body if not isinstance(
        s, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)) for t, _ in _bindings(s) for n in _bound(t)}
        for m in mods.values()))
    if exported:
        word = re.compile(r"\b(?:%s)\b" % "|".join(map(re.escape, sorted(exported))))
        for rel, text in texts.items():
            if rel.endswith(".py") and word.search(text):
                try:
                    mods[rel] = _PyModule(rel, text, exported)
                except (SyntaxError, ValueError, UnicodeError):
                    pass
    for rel, text in sorted(texts.items()):
        if pathlib.PurePosixPath(rel).name == "ssh":
            out.append((rel, 1, "file", "red"))
        if rel.startswith("scripts/tests/") and _BYPASS.search(text):
            out += [(rel, i, "bypass", "red") for i, ln in enumerate(text.splitlines(), 1) if _BYPASS.search(ln)]
        m = mods.get(rel)
        if m is not None:
            out += [(rel, line, kind, kind) for line, kind in m.makers()]
            out += [(rel, line, kind, m.verdict(kind, node))
                    for line, kind, node in m.sites()]
            continue
        reads = _narrow_point_reads(rel, text)
        out += [(rel, i, "text", "red") for i in _text_sites(text) if i not in reads]
    return sorted(set(out))


def without_the_rule(root, files=None):
    """`rel:line` of every red place of `sweep(root)`."""
    return sorted({"%s:%d" % (rel, line) for rel, line, _, v in sweep(root, files) if v == "red"})


def makers(found):
    """{'rule': modules, 'inert': modules} of the maker's calls in `found`."""
    out = {"rule": set(), "inert": set()}
    for rel, _, kind, _ in found:
        if kind in out:
            out[kind].add(rel[:-3].replace("/", "."))
    return out


def maker_mismatch(found):
    """The tree's makers against the lists: {kind: the modules in one but not the other}."""
    got = makers(found)
    want = {"rule": {u[0] for u in USERS} | {i[0] for i in IMPORTERS}, "inert": set(INERT)}
    return {kind: sorted(got[kind] ^ want[kind]) for kind in ("rule", "inert") if got[kind] ^ want[kind]}


# Re-check M7e A-M1: eleven writers of a running `ssh` without a path form
# the sweep knew; each runs for real in the run-time pin.
_RUN = '#!/bin/sh\nshift\nexec bash -c "$*"\n'
_PRINTF = "printf '#!/bin/sh\\nshift\\nexec sh -c \"$*\"\\n'"
WRITERS = {
    "py_param_gap": ("scripts/tests/test_w1.py", 'RUN = %r\ndef put(d, name):\n    (d / name).write_text(RUN)\n'
                     'def f(d):\n    put(d, "ssh")\n' % _RUN),
    "py_return_gap": ("scripts/tests/test_w2.py", 'RUN = %r\ndef tool():\n    return "ssh"\n'
                      'def f(d):\n    (d / tool()).write_text(RUN)\n' % _RUN),
    "py_open_rel": ("scripts/tests/test_w3.py", 'import os\nRUN = %r\ndef f(d):\n    os.chdir(d)\n'
                    '    with open("ssh", "w") as fh:\n        fh.write(RUN)\n' % _RUN),
    "py_ln_argv": ("scripts/tests/test_w4.py", 'import subprocess\nRUN = %r\ndef f(d):\n'
                   '    (d / "stub").write_text(RUN)\n'
                   '    subprocess.run(["ln", "-s", str(d / "stub"), "ssh"], cwd=d, check=True)\n' % _RUN),
    "py_cp_argv": ("scripts/tests/test_w5.py", 'import subprocess\nRUN = %r\ndef f(d):\n'
                   '    (d / "stub").write_text(RUN)\n'
                   '    subprocess.run(["cp", str(d / "stub"), "ssh"], cwd=d, check=True)\n' % _RUN),
    "py_sh_in_str": ("scripts/tests/test_w6.py", 'import subprocess\nRUN = %r\ndef f(d):\n'
                     '    subprocess.run(["sh", "-c", \'cat >"$1/ssh"; chmod +x "$1/ssh"\', "sh", str(d)],\n'
                     '                   input=RUN, text=True, check=True)\n' % _RUN),
    "py_heredoc_str": ("scripts/tests/test_w7.py", 'import subprocess\nSETUP = %r\ndef f(d):\n'
                       '    subprocess.run(["sh", "-c", SETUP], env={"BIN": str(d), "PATH": "/usr/bin:/bin"},'
                       ' check=True)\n' % ('cat >"$BIN/ssh" <<"EOF"\n' + _RUN + 'EOF\n')),
    "sh_bare_ln": ("scripts/tests/fixtures/w8.sh", 'd=$1\n%s > "$d/stub"\ncd "$d" && ln -s "$d/stub" ssh\n' % _PRINTF),
    "sh_bare_cp": ("scripts/tests/fixtures/w9.sh", 'd=$1\n%s > "$d/stub"\ncd "$d" && cp stub ssh && chmod +x ssh\n'
                   % _PRINTF),
    "sh_for": ("scripts/tests/fixtures/w10.sh", 'd=$1\nfor t in ssh scp; do %s > "$d/$t"; done\n' % _PRINTF),
    "sh_var": ("scripts/tests/fixtures/w11.sh", 'd=$1\nn=ssh\n%s > "$d/$n"\n' % _PRINTF),
}


# TV-Z3 (VORGABEN 3.5), re-check M7e A-M5: a deadline of a kit test is the
# hang marker `kit_time.HANG` (scaled by MECLAW_TEST_TIME_FACTOR), never a
# number -- in every Python file of `scripts/tests/` (git ls-files), not a
# hand list. A deadline is a keyword `timeout*`/`deadline*`/`wait*` (in a call
# or as a parameter default) and a positional argument of a wait
# (`wait`/`join`/`get`/`acquire`/`communicate`/`result`/`select`/`wait_for`,
# `kit_time.until`'s third); a number is a literal, its sign or literal
# arithmetic (`10 * 2`, `float(30)`).
#
# Rest boundary (named, may only shrink): a number of the PLAN of the unit
# under test is data, no wait of the kit -- a station's `timeout_min`
# (minutes the executor or the scheduler reads), the scheduler's station
# `timeout`, a lease's `deadline_min`, and a deadline on an injected clock
# (`self.fc`: virtual seconds). They stand in DEADLINE_DATA by
# `file:callee:keyword` with their count; the pin holds the list EQUAL to
# the tree, and a RATCHET against the base of the branch (re-check tv-bn
# M7h B-M2, orchestrator OR-T10-2): no count above the same scan over the
# tree at `git merge-base HEAD master`, no key the base lacks -- unless the
# entry carries a ruling `(n, "OR-...")` whose id stands in .config/testing-rulings.toml (OR-T10-12). No base = red. Beyond the
# AST: a number through a variable (`T = 10; wait(T)`) and a deadline in a
# string are not seen -- this pin is an early warning, not the guarantee.
_WAITS = {"wait", "join", "get", "result", "acquire", "communicate", "select", "wait_for"}
_DEADLINE_KW = re.compile(r"^(?:timeout|deadline|wait)(?:_\w+)?$")
DEADLINE_DATA = {
    "test_pipeline_executor.py:def station:timeout_min": 1,
    "test_pipeline_executor.py:def _browser:timeout_min": 1,
    "test_pipeline_executor.py:def _browser_emptied:timeout_min": 1,
    "test_pipeline_executor.py:station:timeout_min": 11,
    "test_pipeline_register.py:station:timeout_min": 2,
    "test_pipeline_schedule.py:def station:timeout": 1,
    "test_pipeline_schedule.py:station:timeout": 4,
    "test_pipeline_ir_path.py:sched_station:timeout": 2,
    "test_pipeline_two_phase_build.py:sched_station:timeout": 1,
    "test_pipeline_two_phase_build.py:ex_station:timeout_min": 6,
    "test_pipeline_lanes_cli.py:def hold:deadline_min": 1,
    "test_pipeline_lanes_cli.py:hold:deadline_min": 1,
    "test_pipeline_pool.py:get:timeout_s": 2,
}


def _number(e):
    """A literal number other than zero (a zero waits for nothing), its sign, literal arithmetic."""
    if isinstance(e, ast.Constant):
        return type(e.value) in (int, float) and e.value != 0
    if isinstance(e, ast.UnaryOp) and isinstance(e.op, (ast.USub, ast.UAdd)):
        return _number(e.operand)
    if isinstance(e, ast.BinOp):
        lit = [x for x in (e.left, e.right) if not (isinstance(x, ast.Constant) and type(x.value) in (int, float))]
        return all(_number(x) for x in lit) and (_number(e.left) or _number(e.right))
    if isinstance(e, ast.Call) and isinstance(e.func, ast.Name) and e.func.id in ("float", "int") \
            and len(e.args) == 1 and not e.keywords:
        return _number(e.args[0])
    return False


def literal_deadlines(text):
    """`[(line, 'callee:keyword')]` of the deadlines of `text` given as a number."""
    out = set()
    for n in ast.walk(ast.parse(text)):
        if isinstance(n, ast.Call):
            name = _call_name(n)
            args = n.args
            if name == "get" and args and isinstance(args[0], ast.Constant) and isinstance(args[0].value, str):
                args = []   # `d.get("k", 30)`: a mapping's default, no wait
            if name in _WAITS and any(_number(a) for a in args):
                out.add((n.lineno, "%s:" % name))
            if name == "until" and len(args) >= 3 and _number(args[2]):
                out.add((n.lineno, "until:"))
            out |= {(k.value.lineno, "%s:%s" % (name, k.arg)) for k in n.keywords
                    if k.arg and _DEADLINE_KW.match(k.arg) and _number(k.value)}
        elif isinstance(n, (ast.FunctionDef, ast.AsyncFunctionDef, ast.Lambda)):
            a = n.args
            pos = a.posonlyargs + a.args
            pairs = list(zip(pos[len(pos) - len(a.defaults):], a.defaults)) + list(zip(a.kwonlyargs, a.kw_defaults))
            fname = getattr(n, "name", "lambda")
            out |= {(d.lineno, "def %s:%s" % (fname, p.arg)) for p, d in pairs
                    if d is not None and _DEADLINE_KW.match(p.arg) and _number(d)}
    return sorted(out)


def deadline_files(root):
    """Every Python file of `scripts/tests/` (git ls-files) but `kit_time.py`, which defines the marker."""
    res = subprocess.run(["git", "-C", str(root), "ls-files", "-z", "--cached", "--others", "--exclude-standard",
                          "--", "scripts/tests"], capture_output=True, timeout=kit_time.HANG, check=True)
    rels = [r for r in res.stdout.decode().split("\0") if r.endswith(".py") and r != "scripts/tests/kit_time.py"]
    if not rels:
        raise AssertionError("no Python file under scripts/tests -- the deadline pin reads nothing")
    return sorted(rels)


DEADLINE_BASE_REFS = ("master", "origin/master")


def _data_count(value):
    """The count of a DEADLINE_DATA entry: `n`, or `(n, "OR-...")`."""
    return value[0] if isinstance(value, tuple) else value


def _data_ruling(value):
    """The ruling id of a DEADLINE_DATA entry, "" without one."""
    if isinstance(value, tuple) and len(value) == 2:
        return str(value[1])
    return ""


# The file a ruling for DEADLINE_DATA must name under `loosens` in the
# register (orchestrator OR-T10-26, re-check tv-bn M7l B-H1: any valid id --
# OR-TV-2, scoped to flaky-legacy.toml -- opened every key).
DEADLINE_LOOSENS = "scripts/tests/test_fake_ssh.py"


def register_at(root, rev):
    """The register `.config/testing-rulings.toml` as it stands at `rev`
    ({id: entry}), None when it is missing or no TOML there (fail-closed:
    `ruling_problem` then says it does not exist)."""
    from scripts import testing_bans
    res = subprocess.run(["git", "-C", str(root), "show", "%s:%s" % (rev, testing_bans.REGISTER_FILE)],
                         capture_output=True, timeout=kit_time.HANG)
    if res.returncode != 0:
        return None
    try:
        doc = tomllib.loads(res.stdout.decode("utf-8"))
    except (UnicodeDecodeError, tomllib.TOMLDecodeError):
        return None
    return {e.get("id"): e for e in doc.get("ruling", []) if isinstance(e, dict) and e.get("id")}


def _ruling_problem(root, value, base):
    """Why the ruling of a DEADLINE_DATA entry does not hold, None when it
    does: its id stands in the register `.config/testing-rulings.toml` AT
    THE BASE of the branch (re-check tv-bn M7m B-H1: the worktree's register
    let the strand's own diff carry the ruling past this ratchet) -- owner or
    coordinator, not expired, no strand ruling, the form of the entry -- the
    check of `testing_bans` (orchestrator OR-T10-12, re-check tv-bn M7j B-M3:
    a regex `OR-...` let the diff the ratchet bounds grant its own exception;
    the pin named a non-existent OR-TV-99 green), and it names this file
    under `loosens` (DEADLINE_LOOSENS, OR-T10-26)."""
    from scripts import testing_bans
    return testing_bans.ruling_problem(register_at(root, base), _data_ruling(value),
                                       loosens=DEADLINE_LOOSENS)


def deadline_base(root):
    """The commit the branch grew from (`git merge-base HEAD <ref>`, the
    first of DEADLINE_BASE_REFS that resolves), None without one."""
    for ref in DEADLINE_BASE_REFS:
        res = subprocess.run(["git", "-C", str(root), "merge-base", "HEAD", ref],
                             capture_output=True, text=True, timeout=kit_time.HANG)
        if res.returncode == 0 and res.stdout.strip():
            return res.stdout.strip()
    return None


def deadline_counts_at(root, rev, keys):
    """The counts of `keys` (`file:callee:keyword`) in the Python files of
    `scripts/tests/` at `rev` -- the scan of `deadline_hits`, over git's
    objects (one `cat-file --batch`)."""
    ls = subprocess.run(["git", "-C", str(root), "ls-tree", "-r", "-z", "--name-only", rev, "--", "scripts/tests"],
                        capture_output=True, timeout=kit_time.HANG, check=True)
    rels = [r for r in ls.stdout.decode().split("\0") if r.endswith(".py") and r != "scripts/tests/kit_time.py"]
    blob = subprocess.run(["git", "-C", str(root), "cat-file", "--batch"],
                          input="".join("%s:%s\n" % (rev, r) for r in rels).encode(),
                          capture_output=True, timeout=kit_time.HANG, check=True).stdout
    counts, pos = {}, 0
    for rel in rels:
        head_end = blob.index(b"\n", pos)
        size = int(blob[pos:head_end].split()[2])
        text = blob[head_end + 1:head_end + 1 + size].decode("utf-8", "replace")
        pos = head_end + 1 + size + 1
        try:
            found = literal_deadlines(text)
        except SyntaxError:
            continue
        for _, what in found:
            key = "%s:%s" % (rel[len("scripts/tests/"):], what)
            if key in keys:
                counts[key] = counts.get(key, 0) + 1
    return counts


def deadline_ratchet(root, data=None):
    """The breaches of the ratchet: `key: <n> > base <m>` for every entry of
    `data` (default DEADLINE_DATA) above its count at the base of the
    branch and without a ruling the register holds (`_ruling_problem`);
    `no base` when there is none."""
    data = DEADLINE_DATA if data is None else data
    base = deadline_base(root)
    if base is None:
        return ["no base: git merge-base HEAD %s resolves nothing" % "|".join(DEADLINE_BASE_REFS)]
    at_base = deadline_counts_at(root, base, set(data))
    return ["%s: %d > base %d (%s) without a ruling of the register: %s"
            % (k, _data_count(v), at_base.get(k, 0), base[:9], _ruling_problem(root, v, base))
            for k, v in sorted(data.items())
            if _data_count(v) > at_base.get(k, 0) and _ruling_problem(root, v, base)]


def deadline_hits(root):
    """(red `rel:line callee:keyword`, data counts by `file:callee:keyword`)."""
    red, data = [], {}
    for rel in deadline_files(root):
        name = rel[len("scripts/tests/"):]
        for line, what in literal_deadlines((pathlib.Path(root) / rel).read_text(encoding="utf-8")):
            key = "%s:%s" % (name, what)
            if key in DEADLINE_DATA:
                data[key] = data.get(key, 0) + 1
            else:
                red.append("%s:%d %s" % (name, line, what))
    return red, data


def setUpModule():
    store_isolation.isolate()


def tearDownModule():
    store_isolation.restore()


class FakeSshRule(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory(prefix="fake-ssh-")
        self.addCleanup(self._tmp.cleanup)
        self.tmp = pathlib.Path(self._tmp.name)

    def arrived(self, text, host, names):
        """The names of `names` the remote side of the fake `text` still sees."""
        # not named `ssh`: side_effect_guard blocks a real ssh by that name
        ssh = self.tmp / "fake-ssh"
        ssh.write_text(text)
        ssh.chmod(0o755)
        env = dict(os.environ, FAKE_SSH_LOG=str(self.tmp / "ssh.log"))
        env.update({n: "leaked-%s" % n for n in names})
        probe = "; ".join('[ -z "${%s+x}" ] || echo "arrived %s"' % (n, n) for n in names)
        res = subprocess.run([str(ssh), "-o", "BatchMode=yes", host, probe], env=env,
                             capture_output=True, text=True, timeout=kit_time.HANG)
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        return [ln.split()[1] for ln in res.stdout.splitlines() if ln.startswith("arrived ")]

    def test_the_list_is_read_from_the_transport(self):
        names = fake_ssh.carried_names()
        for name in lane.verdict_env() + lane.lane_words() + tuple(lane.GOLDEN_SWITCHES):
            self.assertIn(name, names)
        self.assertEqual(len(names), len(set(names)))
        # Re-check M7c M3: the brake and the lane's name are words of the
        # transport's own list now, never a hand copy beside it.
        for name in ("MECLAW_BROWSER_CPU_THROTTLE", "MECLAW_LANE_NAME", "CARGO_BUILD_JOBS",
                     "NEXTEST_TEST_THREADS", "RUSTFLAGS"):
            self.assertIn(name, lane.lane_words())
        self.assertFalse(hasattr(fake_ssh, "BRAKE"))
        self.assertEqual(set(names), set(lane.verdict_env() + lane.lane_words()
                                         + tuple(lane.GOLDEN_SWITCHES) + fake_ssh.TEST_SECRET))

    def test_every_user_is_built_from_the_one_rule(self):
        for mod, template, built, _ in USERS:
            m = importlib.import_module(mod)
            with self.subTest(mod):
                self.assertEqual(fake_ssh.script(getattr(m, template)), getattr(m, built))

    def test_no_user_strips_a_carried_name_of_its_own(self):
        names = fake_ssh.carried_names()
        for mod, template, _, _ in USERS:
            text = getattr(importlib.import_module(mod), template)
            with self.subTest(mod):
                own = [ln for ln in text.splitlines() if ln.lstrip().startswith("unset ")
                       and set(ln.split()[1:]) & set(names)]
                self.assertEqual([], own)

    def test_every_writer_of_an_ssh_is_the_maker(self):
        """Re-check M7e: no place of the files writes an `ssh` but the maker
        (`fake_ssh.install`/`install_inert`), and the makers of the tree are
        EQUAL to USERS + IMPORTERS (the rule) and INERT (A-M4: the pin had
        become containment -- a seventh user escaped the USERS pins)."""
        found = sweep(REPO)
        self.assertEqual([], without_the_rule(REPO))
        self.assertEqual(maker_mismatch(found), {})

    def test_a_write_inside_the_narrow_point_stays_red(self):
        """Mutants on the narrow point's reads: a redirect into, a copy onto
        an `ssh` inside `ssh_bin` and a read form outside it are red; the
        real function is not."""
        text = pathlib.Path(lane.LANE_SYNC).read_text(encoding="utf-8")
        rel = "scripts/lane_sync.sh"
        self.assertTrue(_narrow_point_reads(rel, text))
        self.assertEqual([], [i for i in _text_sites(text) if i not in _narrow_point_reads(rel, text)])
        head = "ssh_bin() {\n"
        self.assertIn(head, text)
        for bad in ('    : > "$d/ssh"\n', '    cp /bin/true "$d/ssh"\n', '    printf x >>"$d"/ssh\n'):
            with self.subTest(bad=bad):
                mut = text.replace(head, head + bad, 1)
                line = text[:text.index(head)].count("\n") + 2
                self.assertIn(line, [i for i in _text_sites(mut) if i not in _narrow_point_reads(rel, mut)])
        outside = text.replace("remote() {", 'remote() {\n    [[ -x $d/ssh ]]', 1)
        self.assertTrue([i for i in _text_sites(outside) if i not in _narrow_point_reads(rel, outside)])
        self.assertEqual(set(), _narrow_point_reads("scripts/other.sh", text))

    def test_an_importer_takes_a_template_of_a_user(self):
        for mod, source, name in IMPORTERS:
            with self.subTest(mod):
                self.assertIn(source, [u[0] for u in USERS])
                self.assertIs(getattr(importlib.import_module(source), name),
                              getattr(importlib.import_module(mod), name))

    def test_a_maker_beside_the_lists_is_red(self):
        """A-M4 (mutant): one maker more than the lists name is a mismatch
        (under the old containment it was green), one less as well."""
        found = sweep(REPO)
        extra = found + [("scripts/tests/test_seventh.py", 9, "rule", "rule"),
                         ("scripts/tests/test_eighth.py", 9, "inert", "inert")]
        self.assertEqual({"rule": ["scripts.tests.test_seventh"], "inert": ["scripts.tests.test_eighth"]},
                         maker_mismatch(extra))
        fewer = [f for f in found if not f[0].endswith("test_pipeline_hygiene.py")]
        self.assertEqual({"inert": ["scripts.tests.test_pipeline_hygiene"]}, maker_mismatch(fewer))

    def git_tree(self, files):
        root = self.tmp / "tree"
        for rel, text in files.items():
            (root / rel).parent.mkdir(parents=True, exist_ok=True)
            if text is None:   # a link to nowhere: a file the sweep cannot read
                os.symlink("nowhere", root / rel)
            else:
                (root / rel).write_bytes(text if isinstance(text, bytes) else text.encode())
        for argv in (["init", "-q"], ["add", "-A"]):
            subprocess.run(["git", "-C", str(root)] + argv, check=True, capture_output=True,
                           timeout=kit_time.HANG)
        return root

    def red_files(self, files):
        root = self.git_tree(files)
        try:
            return sorted({r.rsplit(":", 1)[0] for r in without_the_rule(root)})
        finally:
            shutil.rmtree(root)

    def test_a_writer_of_an_ssh_is_red_whatever_its_form_or_place(self):
        """The early warning (mutants): every form of the re-check probes,
        each ALONE, is red -- p1-p10 and m1-m9 (M7d), the eleven writers
        without a path form (M7e A-M1), the files that do not decode or
        cannot be read (A-M2), the marker forms (A-M3: there is no marker
        any more; a write beside the maker or under a comment stays red), a
        file not yet added (H1) and a test that writes the registry itself."""
        run = '#!/bin/sh\nshift\nexec bash -c "$*"\n'
        mutants = {
            # the re-check probe of M7d, verbatim (rc-m7db/probe_m2.py)
            "p1": {"scripts/tests/test_p1.py": 'BODY = %r\ndef f(d):\n    p = d / "ssh"\n    p.write_text(BODY)\n' % run},
            "p2": {"scripts/tests/test_p2.py": 'FAKE_SSH = %r\n' % '#!/bin/sh\nshift\ncmd="$*"\nbash -c "$cmd"\n'},
            "p3": {"scripts/tests/test_p3.py": 'FAKE_SSH = %r\n' % '#!/bin/sh\nshift\nexec env -u FOO "$@"\n'},
            "p4": {"scripts/tests/test_p4.py": 'FAKE_SSH = %r\n'
                   % "#!/usr/bin/env python3\nimport os, sys\ncmd = ' '.join(sys.argv[2:])\nos.system(cmd)\n"},
            "p5": {"scripts/tests/test_p5.py": 'FAKE_SSH = %r\n' % '#!/bin/sh\nexec bash -c "$2"\n'},
            "p6": {"scripts/tests/helper_p6.py": 'BODY = %r\n' % run,
                   "scripts/tests/test_p6.py": 'from scripts.tests.helper_p6 import BODY\n'
                                               'def f(d):\n    (d / "ssh").write_text(BODY)\n'},
            "p7": {"scripts/tests/fixtures/remote.sh": run,
                   "scripts/tests/test_p7.py": 'import shutil\nFIX = None\n'
                                               'def f(d):\n    shutil.copy(FIX / "remote.sh", d / "ssh")\n'},
            "p8": {"scripts/selftest_p8.sh": '#!/bin/sh\ncat >"$1/ssh" <<\'EOF\'\n%sEOF\n' % run},
            "p9": {"tests/kit/test_p9.py": 'FAKE_SSH = %r\n' % run},
            "p10": {"scripts/tests/fixtures/lane_p10.sh": '#!/bin/sh\ncat <<\'EOF\' >"$1/ssh"\n%sEOF\n' % run},
            # the mutants of re-check M7c M2, all nine (re-check M7e H3)
            "m1": {"scripts/tests/test_m1.py": 'B = "#!/bin/sh\\nshift\\nbash -c \\"$@\\"\\n"\n'
                                               'def f(d):\n    (d / "ssh").write_text(B)\n'},
            "m2": {"scripts/tests/test_m2.py": 'FAKE_SSH = """#!/bin/sh\nshift\nsh -c "$*"\n"""\n'},
            "m3": {"scripts/tests/test_m3.py": 'def f(w, t):\n    w(t, "bin/ssh", "#!/bin/sh\\nshift\\neval \\"$*\\"\\n")\n'},
            "m4": {"scripts/tests/test_m4.py": 'X = """#!/bin/sh\nwhile [ $# -gt 1 ]; do shift; done\nexec "$@"\n"""\n'
                                               'def f(b):\n    for n, s in (("ssh", X), ("sudo", "")):\n'
                                               '        (b / n).write_text(s)\n'},
            "m5": {"scripts/tests/sub/test_m5.py": 'RUN = {"ssh": "#!/usr/bin/env python3\\nimport subprocess, sys\\n'
                                                   'subprocess.run(\' \'.join(sys.argv[2:]), shell=True)\\n"}\n'},
            "m6": {"scripts/tests/fixtures/lane.sh": '#!/bin/sh\ncat >"$1/ssh" <<\'EOF\'\n#!/bin/sh\nshift\n"$@"\nEOF\n'},
            "m7": {"scripts/tests/fixtures/ssh": '#!/bin/sh\nshift\nexec $*\n'},
            "m8": {"workshop/evals/demo/tests/test_m8.py": 'SSH_FAKE = "#!/bin/sh\\nshift\\nexec bash -c \\"$*\\"\\n"\n'},
            "m9": {"workshop/tools/demo/tests/run.sh": 'printf \'#!/bin/sh\\nshift\\nexec sh -c "$*"\\n\' > "$d/ssh"\n'},
            "m9web": {"web/tools/demo/tests/run.sh": 'printf \'#!/bin/sh\\nshift\\nexec sh -c "$*"\\n\' > "$d/ssh"\n'},
            # further forms and places
            "join": {"scripts/x/fake.py": 'import os\ndef f(d, body):\n'
                                          '    with open(os.path.join(d, "bin", "ssh"), "w") as fh:\n'
                                          '        fh.write(body)\n'},
            "fstring": {"scripts/tests/test_f.py": 'def f(d, b):\n    open(f"{d}/ssh", "w").write(b)\n'},
            "name": {"scripts/tests/test_n.py": 'TOOL = "ssh"\ndef f(d, b):\n    (d / TOOL).write_text(b)\n'},
            "list": {"scripts/tests/test_l.py": 'TOOLS = ["ssh", "rsync"]\ndef f(d, b):\n'
                                                '    for t in TOOLS:\n        (d / t).symlink_to(b)\n'},
            "rust": {"crates/x/tests/fake.rs": 'fn f(d: &Path) { std::fs::write(d.join("ssh"), BODY).unwrap(); }\n'},
            "node": {"web/x/fake.test.mjs": "fs.writeFileSync(path.join(dir, 'ssh'), body);\n"},
            "unreachable": {"scripts/tests/test_u.py": 'FAKE_SSH = "#!/bin/sh\\necho unreachable >&2\\nexit 255\\n"\n'},
        }
        mutants.update({name: {rel: text} for name, (rel, text) in WRITERS.items()})
        mutants.update({
            # A-M2: what does not decode is read as bytes, what cannot be read is red
            "latin1_sh": {"scripts/tests/fixtures/l1.sh": b'#!/bin/sh\n# \xe9\ncat >"$1/ssh" <<EOF\n'
                                                          + run.encode() + b'EOF\n'},
            "latin1_py": {"scripts/tests/test_l1.py": b'# \xe9\nRUN = %r\ndef f(d):\n    (d / "ssh").write_text(RUN)\n'
                                                      % run.encode()},
            "binary_ssh": {"scripts/tests/fixtures/blob/ssh": b"\x7fELF\x02\x01\x01\x00\xff\xfe"},
            "unreadable": {"scripts/tests/fixtures/gone": None},
            # A-M3: the forms the marker let pass (L1-L6b) -- no marker any more
            "L1": {"scripts/tests/test_l1m.py": 'RUN = %r\ndef f(d):\n    # @fake-ssh: runs nothing@ -- says it\n'
                                                '    (d / "ssh").write_text(RUN)\n' % run},
            "L2": {"scripts/tests/fixtures/l2.sh": '# @fake-ssh: runs nothing@ -- says it\ncat >"$1/ssh" <<EOF\n%sEOF\n'
                                                   % run},
            "L4": {"scripts/tests/test_l4.py": 'from scripts.tests import fake_ssh\nT = "x"\nRUN = %r\n'
                                               'def f(d, e):\n    fake_ssh.install(d, T)\n'
                                               '    (e / "ssh").write_text(RUN)\n' % run},
            "L4b": {"scripts/tests/test_l4b.py": 'from scripts.tests import fake_ssh\nRUN = %r\ndef f(d):\n'
                                                 '    if False:\n        fake_ssh.script(None)\n'
                                                 '    (d / "ssh").write_text(RUN)\n' % run},
            "L5": {"scripts/tests/fixtures/x/ssh": '#!/bin/sh\n# @fake-ssh: runs nothing@\nprintf %s "$*" | sh\n'},
            "L6": {"scripts/tests/test_l6.py": '# @fake-ssh: runs nothing@ -- says it\n'
                                               'FAKE_SSH = "#!/bin/sh\\nbash <<<\\"$*\\"\\n"\n'},
            "template_twice": {"scripts/tests/test_tt.py": 'from scripts.tests import fake_ssh\n'
                                                           'FAKE_SSH_T = %r\nS = fake_ssh.install(None, FAKE_SSH_T)\n'
                                                           % ("#!/bin/sh\n%s\n%s\n" % ((fake_ssh.MARKER,) * 2))},
            # the registry beside the maker
            "bypass": {"scripts/tests/test_by.py": 'import os\ndef f(p):\n'
                                                   '    os.environ["MECLAW_FAKE_SSH_REGISTRY"] = str(p)\n'},
            "bypass2": {"scripts/tests/test_by2.py": 'from scripts.tests import fake_ssh\n'
                                                     'def f(p):\n    fake_ssh._register(str(p), None)\n'},
        })
        for name, files in mutants.items():
            writer = sorted(files)[-1] if name not in ("p6", "p7") else \
                [r for r in files if "/test_" in r][0]
            with self.subTest(name):
                self.assertEqual([writer], self.red_files(files))
        with self.subTest("not yet added (H1)"):
            root = self.git_tree({"scripts/tests/test_ok.py": "X = 1\n"})
            (root / "scripts/tests/test_new.py").write_text('RUN = %r\ndef f(d):\n    (d / "ssh").write_text(RUN)\n'
                                                            % run)
            self.assertEqual(["scripts/tests/test_new.py:3"], without_the_rule(root))
            shutil.rmtree(root)

    def test_what_the_sweep_lets_pass(self):
        """The controls: the maker's calls (also with a template imported, as
        `test_pipeline_ir_path` takes `PASS_SSH_TEMPLATE`), a caller of the
        real ssh, a reader of an ssh path, and the prose (PROSE_ROOTS)."""
        tmpl = '#!/bin/sh\n%s\nshift\nexec bash -c "$*"\n' % fake_ssh.MARKER
        files = {
            "scripts/tests/test_r.py": 'from scripts.tests import fake_ssh\nFAKE_SSH_TEMPLATE = %r\n'
                                       'S = fake_ssh.script(FAKE_SSH_TEMPLATE)\n'
                                       'def f(d):\n    fake_ssh.install(d, FAKE_SSH_TEMPLATE)\n' % tmpl,
            "scripts/tests/test_ri.py": 'from scripts.tests import fake_ssh\n'
                                        'from scripts.tests.test_r import FAKE_SSH_TEMPLATE\n'
                                        'def f(d):\n    fake_ssh.install(d, FAKE_SSH_TEMPLATE)\n',
            "scripts/tests/test_in.py": 'from scripts.tests import fake_ssh\ndef f(d):\n'
                                        '    fake_ssh.install_inert(d, "unreachable", stderr="no", rc=255)\n',
            "scripts/pipeline/caller.py": 'import shutil, subprocess\n'
                                          'def f(lane, host, cmd):\n'
                                          '    if lane.transport != "ssh" or not shutil.which("ssh"):\n'
                                          '        return None\n'
                                          '    return subprocess.run(["ssh", "-o", "BatchMode=yes", host, cmd])\n',
            "scripts/pipeline/engstelle.py": 'import os\ndef f():\n'
                                             '    return os.environ.get("MECLAW_FAKE_SSH_REGISTRY", "")\n',
            "scripts/lane.sh": 'transport=ssh\nexec ssh -o BatchMode=yes "$host" "$cmd"\n',
            "docs/kit.md": 'cat >"$d/ssh" <<EOF\nexec bash -c "$*"\nEOF\n',
            "/".join(("plans", "x", "probe.py")): 'FAKE_SSH = %r\n' % tmpl.replace(fake_ssh.MARKER, ""),
        }
        root = self.git_tree(files)
        found = sweep(root)
        self.assertEqual([], without_the_rule(root))
        self.assertEqual({"scripts.tests.test_r", "scripts.tests.test_ri"}, makers(found)["rule"])
        self.assertEqual({"scripts.tests.test_in"}, makers(found)["inert"])
        self.assertEqual({"scripts/tests/test_r.py"}, {rel for rel, _, k, v in found if k == "body" and v == "rule"})

    def run_writer(self, rel, text, d):
        """Run the writer `text` (a mutant of WRITERS) for real into `d`."""
        src = self.tmp / pathlib.PurePosixPath(rel).name
        src.write_text(text)
        if rel.endswith(".py"):
            argv = [sys.executable, "-I", "-c", "import pathlib, runpy, sys\n"
                    "runpy.run_path(sys.argv[1])['f'](pathlib.Path(sys.argv[2]))", str(src), str(d)]
        else:
            argv = ["sh", str(src), str(d)]
        res = subprocess.run(argv, capture_output=True, text=True, timeout=kit_time.HANG,
                             env=dict(os.environ, PATH=store_isolation.trapped_path()))
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)

    @staticmethod
    def lane_sync(snippet, env):
        """`snippet` with lane_sync.sh's functions loaded, its dispatch cut
        off (the same cut as test_lane_engstelle, which does not travel)."""
        return subprocess.run(["bash", "-c", 'source "$0"; ' + snippet, _fns()], capture_output=True, text=True,
                              env=env, timeout=kit_time.HANG)

    def narrow_point(self, d):
        """What the narrow point says to the `ssh` of `d` on the front of
        PATH, with this process's registry: (Python `lane.ssh_program`,
        Bash `lane_sync.sh ssh_bin` exit, its stderr)."""
        env = dict(os.environ, PATH=str(d) + os.pathsep + store_isolation.trapped_path())
        try:
            py = lane.ssh_program(env)
        except lane.LaneError as exc:
            py = exc
        res = self.lane_sync("ssh_bin", env=env)
        return py, res.returncode, res.stderr

    def test_a_writer_beside_the_maker_is_refused_at_run_time(self):
        """The guarantee (orchestrator decision M7f/M7h), end to end: the
        eleven writers of re-check M7e A-M1 -- the sweep did not see them --
        each write a running `ssh`, for real; the narrow point (Python and
        Bash) refuses each of them with its path. The maker's fake in the
        same place passes it."""
        self.assertIn(fake_ssh.REGISTRY, os.environ)  # the guard's stub registered it: test mode
        for name, (rel, text) in WRITERS.items():
            d = self.tmp / ("w-" + name)
            d.mkdir()
            with self.subTest(name):
                self.run_writer(rel, text, d)
                ssh = d / "ssh"
                self.assertTrue(os.path.lexists(ssh), name)
                self.assertFalse(fake_ssh.admits(ssh))
                # the forms differ in how they write, not in the mode: as a
                # program on PATH (what the narrow point resolves) it is 0755
                os.chmod(ssh, 0o755)
                py, rc, err = self.narrow_point(d)
                self.assertIsInstance(py, lane.LaneError, name)
                self.assertIn("unregistered ssh on PATH: %s" % ssh, str(py))
                self.assertEqual(2, rc, err)
                self.assertIn("unregistered ssh on PATH: %s" % ssh, err)
                fake_ssh.install(d, "#!/bin/sh\n%s\nexit 0\n" % fake_ssh.MARKER)
                self.assertTrue(fake_ssh.admits(ssh))
                self.assertEqual((os.path.realpath(ssh), 0), self.narrow_point(d)[:2])

    # a lane played here: options and the target off, the command run by sh
    LANE_AS_HERE = ('#!/bin/sh\n%s\nwhile [ $# -gt 0 ]; do case "$1" in -o) shift 2 ;; -*) shift ;; '
                    '*) break ;; esac; done\nshift\nexec sh -c "$*"\n' % fake_ssh.MARKER)

    def test_the_makers_fake_reaches_the_lane_through_the_narrow_point(self):
        """M7h end to end: a fake from `install` is admitted by the narrow
        point and runs a command as the lane -- `lane.ssh_argv` (Python) and
        `remote` (Bash) -- the guard's stub stays registered beside it."""
        d = self.tmp / "maker"
        d.mkdir()
        before = [p for p in fake_ssh.paths() if os.path.exists(p)]
        stub = os.path.realpath(os.path.join(side_effect_guard.current_dir(), "ssh"))
        self.assertIn(stub, before)
        path = fake_ssh.install(d, self.LANE_AS_HERE)
        self.assertEqual(before + [str(path)], list(fake_ssh.paths()))  # appended, never overwritten
        self.assertEqual(fake_ssh.entry(path), fake_ssh.registry()[-1])  # path AND hash of the body
        env = dict(os.environ, PATH=str(d) + os.pathsep + store_isolation.trapped_path())
        with unittest.mock.patch.dict(os.environ, {"PATH": env["PATH"]}):
            argv = lane.ssh_argv("lane-x", "echo through")
        res = subprocess.run(argv, capture_output=True, text=True, timeout=kit_time.HANG, env=env)
        self.assertEqual((0, "through\n"), (res.returncode, res.stdout), res.stderr)
        res = self.lane_sync('remote lane-x "echo through"', env=env)
        self.assertEqual((0, "through\n"), (res.returncode, res.stdout), res.stderr)


    # Setting forms the word scan does not see (re-check tv-bn M7h A-M2),
    # here on THIS side: each sets and exports a probe name in the caller.
    PROBE_FORMS_SH = (
        'declare -n r=PROBE_NAMEREF; r=1; export PROBE_NAMEREF',
        'set -- -a; getopts a PROBE_GETOPTS; export PROBE_GETOPTS',
        'let PROBE_LET=1; export PROBE_LET',
        '((PROBE_ARITH=1)); export PROBE_ARITH',
        'exec {PROBE_FD}>/dev/null; export PROBE_FD',
        'coproc PROBE_CO { cat; }; export PROBE_CO_PID',
        'n=PROBE_COMPUTED; export "$n=1"',
        'n=probe_upper; export "${n^^}=1"',
        'read -r PROBE_READ <<<1; export PROBE_READ',
    )
    PROBE_NAMES = ("PROBE_NAMEREF", "PROBE_GETOPTS", "PROBE_LET", "PROBE_ARITH", "PROBE_FD",
                   "PROBE_CO_PID", "PROBE_COMPUTED", "PROBE_UPPER", "PROBE_READ")

    def test_the_remote_environment_is_built_from_data_alone(self):
        """Orchestrator OR-T10-2 (re-check tv-bn M7h A-M2, the lock): the
        narrow point builds the lane's environment from data -- an allowlist
        of the host's own names (`lane_sync.sh REMOTE_BASE`) and the words the
        command carries -- with `env -i` semantics, so it does not matter how a
        name was set on this side. Each probe form sets and exports a name
        here; none reaches the lane, through `remote` (Bash) and `ssh_argv`
        (Python). Control: a verdict word and the host's HOME do."""
        d = self.tmp / "envi"
        d.mkdir()
        fake_ssh.install(d, self.LANE_AS_HERE)
        env = dict(os.environ, PATH=str(d) + os.pathsep + store_isolation.trapped_path(),
                   MECLAW_GATE_MODE="strand")
        res = self.lane_sync("; ".join(self.PROBE_FORMS_SH) + '; remote lane-x "$(verdict_words)env"', env=env)
        self.assertEqual(0, res.returncode, res.stderr)
        seen = {line.split("=", 1)[0] for line in res.stdout.splitlines() if "=" in line}
        self.assertEqual(set(), seen & set(self.PROBE_NAMES), "Bash: set here, reached the lane")
        self.assertLessEqual({"MECLAW_GATE_MODE", "HOME", "PATH"}, seen)
        probes = {name: "1" for name in self.PROBE_NAMES}
        with unittest.mock.patch.dict(os.environ, dict(probes, PATH=env["PATH"])):
            argv = lane.ssh_argv("lane-x", "env", env={"MECLAW_GATE_MODE": "strand"})
            res = subprocess.run(argv, capture_output=True, text=True, timeout=kit_time.HANG,
                                 env=dict(os.environ))
        self.assertEqual(0, res.returncode, res.stderr)
        seen = {line.split("=", 1)[0] for line in res.stdout.splitlines() if "=" in line}
        self.assertEqual(set(), seen & set(self.PROBE_NAMES), "Python: set here, reached the lane")
        self.assertLessEqual({"MECLAW_GATE_MODE", "HOME", "PATH"}, seen)

    def test_the_maker_writes_registers_and_applies_the_rule(self):
        env = {"PATH": store_isolation.trapped_path()}
        tmpl = '#!/bin/sh\n%s\nshift\nexec bash -c "$*"\n' % fake_ssh.MARKER
        (self.tmp / "bin").mkdir()
        path = fake_ssh.install(self.tmp / "bin", tmpl, env=env)
        self.assertEqual(pathlib.Path(os.path.realpath(self.tmp / "bin" / "ssh")), path)
        self.assertEqual(fake_ssh.script(tmpl), path.read_text())
        self.assertTrue(os.access(path, os.X_OK))
        self.assertIn(str(path), fake_ssh.paths())
        self.assertEqual((fake_ssh.entry(path),), fake_ssh.registry(env))
        self.assertTrue(fake_ssh.admits(path) and fake_ssh.admits(path, env))
        self.assertFalse(fake_ssh.admits(path, {}))
        # through a link the resolved path is what counts
        os.symlink(path, self.tmp / "link")
        self.assertTrue(fake_ssh.admits(self.tmp / "link", env))
        with self.assertRaisesRegex(AssertionError, "names the marker exactly once"):
            fake_ssh.install(self.tmp / "bin", "#!/bin/sh\nexec bash -c \"$*\"\n")
        # a body rewritten after the maker wrote it is no fake of the maker
        path.write_text(path.read_text() + "touch /nowhere\n")
        self.assertFalse(fake_ssh.admits(path, env))

    def test_a_link_named_ssh_to_another_system_program_is_refused(self):
        """Re-check tv-bn M7h A-M1.2: admission by directory let any system
        program pass under the name `ssh` (`ssh -> /usr/bin/true`, a fake
        "green without a transport"); the narrow point admits the system's
        own ssh only, on both sides, with or without a registry."""
        true = shutil.which("true", path=store_isolation.trapped_path())
        d = self.tmp / "alias"
        d.mkdir()
        os.symlink(true, d / "ssh")
        for registry in ("", None):
            with self.subTest(registry=registry):
                env = dict(os.environ, PATH=str(d) + os.pathsep + store_isolation.trapped_path())
                env.pop(fake_ssh.REGISTRY, None)
                if registry is not None:
                    env[fake_ssh.REGISTRY] = registry
                with self.assertRaisesRegex(lane.LaneError, "unregistered ssh on PATH: %s" % (d / "ssh")):
                    lane.ssh_program(env)
                res = self.lane_sync("ssh_bin", env=env)
                self.assertEqual(2, res.returncode, res.stderr)
                self.assertIn("unregistered ssh on PATH: %s" % (d / "ssh"), res.stderr)

    def test_the_registry_drops_vanished_fakes_and_refuses_its_separator(self):
        env = {}
        a, b = self.tmp / "a", self.tmp / "b:c"
        a.mkdir()
        b.mkdir()
        first = fake_ssh.install_inert(a, "gone soon", env=env)
        shutil.rmtree(a)
        a.mkdir()
        second = fake_ssh.install_inert(a, "again", env=env)
        self.assertEqual(first, second)
        self.assertEqual((str(second),), fake_ssh.paths(env))
        (self.tmp / "c").mkdir()
        third = fake_ssh.install_inert(self.tmp / "c", "third", env=env)
        shutil.rmtree(a)
        fourth = fake_ssh.install_inert(self.tmp / "c", "third", env=env)
        self.assertEqual((str(third),), fake_ssh.paths(env))
        self.assertEqual(third, fourth)
        with self.assertRaisesRegex(AssertionError, "separator"):
            fake_ssh.install_inert(b, "a colon", env=env)
        self.assertNotIn(":", "".join(fake_ssh.registry(env)))

    def test_an_inert_fake_runs_nothing_of_its_arguments(self):
        """`install_inert` generates its body: whatever the arguments hold, it
        logs them, answers from its files and exits -- nothing of them runs."""
        d = self.tmp / "bin"
        d.mkdir()
        log, reply, rc_file, ran = self.tmp / "calls.log", self.tmp / "reply", self.tmp / "rc", self.tmp / "ran"
        reply.write_text("canned\n")
        ssh = fake_ssh.install_inert(d, "a logger", log=log, reply=reply, rc_file=rc_file,
                                     stderr='refused "x" $HOME `y` (argv: {argv})', rc=7)
        for cmd in ("touch %s" % ran, "; touch %s" % ran, "$(touch %s)" % ran, "`touch %s`" % ran,
                    "' ; touch %s ; '" % ran):
            with self.subTest(cmd):
                # through `sh`: side_effect_guard sends an absolute `.../ssh` to its own stub
                res = subprocess.run(["sh", str(ssh), "-o", "BatchMode=yes", "u@h", cmd], capture_output=True,
                                     text=True, timeout=kit_time.HANG)
                self.assertEqual(7, res.returncode)
                self.assertEqual("canned\n", res.stdout)
                self.assertEqual('refused "x" $HOME `y` (argv: -o BatchMode=yes u@h %s)\n' % cmd, res.stderr)
                self.assertFalse(ran.exists())
                self.assertEqual("ssh -o BatchMode=yes u@h %s" % cmd, log.read_text().splitlines()[-1])
        rc_file.write_text("3\n")
        res = subprocess.run(["sh", str(ssh), "h", "true"], capture_output=True, text=True, timeout=kit_time.HANG)
        self.assertEqual(3, res.returncode)
        with self.assertRaisesRegex(AssertionError, "one line"):
            fake_ssh.inert_body("two\nlines")

    def test_no_carried_name_reaches_the_far_side_of_any_user(self):
        names = fake_ssh.carried_names()
        for mod, _, built, host in USERS:
            text = getattr(importlib.import_module(mod), built)
            with self.subTest(mod):
                self.assertEqual([], self.arrived(text, host, names))

    def test_a_new_name_in_the_one_list_reaches_every_user(self):
        new = "MECLAW_A_FIFTH_VERDICT_NAME"
        copy = self.tmp / "lane_sync.sh"
        shutil.copy2(lane.LANE_SYNC, copy)
        kit_tree.seed(copy.parent, "lane_sync.sh")
        text = copy.read_text()
        line = re.search(r"^VERDICT_ENV=\(([A-Z0-9_ ]*)\)$", text, re.M)
        copy.write_text(text.replace(line.group(0), "VERDICT_ENV=(%s %s)" % (line.group(1), new)))
        names = fake_ssh.carried_names(lane_sync=copy)
        self.assertIn(new, names)
        for mod, template, _, host in USERS:
            m = importlib.import_module(mod)
            with self.subTest(mod):
                self.assertEqual([], self.arrived(fake_ssh.script(getattr(m, template), names),
                                                  host, names))

    def lane_env_copy(self, listed):
        """A copy of `lane_sync.sh` whose `lane_env` carries one more word, in
        `LANE_WORDS` when `listed`; returns (copy, the word's name)."""
        new = "MECLAW_A_NEW_LANE_WORD"
        copy = self.tmp / ("listed" if listed else "unlisted") / "lane_sync.sh"
        copy.parent.mkdir()
        shutil.copy2(lane.LANE_SYNC, copy)
        kit_tree.seed(copy.parent, "lane_sync.sh")
        text = copy.read_text()
        hook = "    w+=$(verdict_words)\n"
        self.assertEqual(1, text.count(hook))
        text = text.replace(hook, hook + '    w+="%s=1 "\n' % new)
        if listed:
            line = re.search(r"^LANE_WORDS=\(([A-Z0-9_ ]*)\)$", text, re.M)
            text = text.replace(line.group(0), "LANE_WORDS=(%s %s)" % (line.group(1), new))
        copy.write_text(text)
        return copy, new

    def lane_env_of(self, copy):
        env = dict(os.environ, MECLAW_LANE_NAME="lane-a", MECLAW_LANE_THREADS="")
        return subprocess.run([str(copy), "env", "strand"], env=env, capture_output=True,
                              text=True, timeout=kit_time.HANG)

    def test_a_new_lane_word_reaches_lane_env_and_every_user(self):
        """Re-check M7c M3: a word `lane_env` carries reaches the host only on
        the command line; one list (`LANE_WORDS`) names them, `lane_env`
        refuses a word it lacks, and every fake strips what it names -- a new
        word acts in both without touching a fake."""
        copy, new = self.lane_env_copy(listed=True)
        res = self.lane_env_of(copy)
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertIn("%s=1" % new, res.stdout.split())
        names = fake_ssh.carried_names(lane_sync=copy)
        self.assertIn(new, names)
        for mod, template, _, host in USERS:
            m = importlib.import_module(mod)
            with self.subTest(mod):
                self.assertEqual([], self.arrived(fake_ssh.script(getattr(m, template), names),
                                                  host, names))

    def test_lane_env_refuses_a_word_its_list_lacks(self):
        copy, new = self.lane_env_copy(listed=False)
        res = self.lane_env_of(copy)
        self.assertEqual(2, res.returncode, res.stdout + res.stderr)
        self.assertIn("%s is not in LANE_WORDS" % new, res.stderr)
        self.assertNotIn(new, fake_ssh.carried_names(lane_sync=copy))

    def producer_copy(self, word, listed):
        """A copy of `lane_sync.sh` with a NEW producer of lane words that
        checks nothing itself (re-check M7d B-H1): the verbs `probe` and
        `probe-long` send `<word>=1` through `remote` / `remote_long`, plainly,
        behind an escaped blank and in a `q`-quoted multi-line script."""
        copy = self.tmp / ("producer-%s-%s" % (word, listed)) / "lane_sync.sh"
        copy.parent.mkdir()
        text = pathlib.Path(lane.LANE_SYNC).read_text()
        verb = "    env)    [ $# -eq 1 ] || die \"env <mode>\"; lane_env \"$1\"; echo ;;\n"
        self.assertEqual(1, text.count(verb))
        text = text.replace(verb, verb + (
            '    probe)  remote "$1" "export PATH=\\"\\$HOME/bin:\\$PATH\\" && IFS= read -r x </dev/null;'
            ' $2${3:-%(w)s=1} touch $(q "$4")" ;;\n'
            '    probe-long) nl=$\'\\n\'; remote_long "$1" "bash -c $(q "true${nl}$2${3:-%(w)s=1} touch $4")" ;;\n')
            % {"w": word})
        if listed:
            line = re.search(r"^LANE_WORDS=\(([A-Z0-9_ ]*)\)$", text, re.M)
            text = text.replace(line.group(0), "LANE_WORDS=(%s %s)" % (line.group(1), word))
        copy.write_text(text)
        copy.chmod(0o755)
        kit_tree.seed(copy.parent, "lane_sync.sh")
        return copy

    def test_a_new_producer_of_lane_words_is_checked_at_the_narrow_point(self):
        """Re-check M7d B-H1: `lane_words_known` stood in `lane_env` and
        `cmd_test` only -- `cmd_check` and `cmd_coverage` wrote their words
        unchecked, and so would any new producer. The check sits in `remote`
        and `remote_long` now: a producer without a check of its own sends no
        unlisted word, on a host or `local`; PATH and IFS (HOST_OWN) pass."""
        word = "MECLAW_A_NEW_PRODUCER_WORD"
        for listed in (False, True):
            copy = self.producer_copy(word, listed)
            for verb, target, prefix in (("probe", "local", ""), ("probe", "local", "\\ "),
                                         ("probe-long", "local", ""), ("probe", "u@lane-z.invalid", ""),
                                         ("probe-long", "u@lane-z.invalid", "")):
                if listed and prefix:
                    continue   # `\ W=1` is a refused word, never a command that runs
                ran = self.tmp / ("ran-%s-%s-%s" % (listed, verb, target))
                res = subprocess.run([str(copy), verb, target, prefix, "", str(ran)],
                                     capture_output=True, text=True, timeout=kit_time.HANG)
                with self.subTest(listed=listed, verb=verb, target=target, prefix=prefix):
                    if not listed:
                        self.assertEqual(2, res.returncode, res.stdout + res.stderr)
                        self.assertIn("the lane word %s is not in LANE_WORDS" % word, res.stderr)
                        self.assertNotIn("is blocked in tests", res.stderr)   # no ssh started
                        self.assertFalse(ran.exists())
                    elif target == "local":
                        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
                        self.assertTrue(ran.exists())
                    else:   # the command left: the tests' guard stands in for ssh
                        self.assertIn("is blocked in tests", res.stderr)
                        self.assertNotIn("is not in LANE_WORDS", res.stderr)

    def test_the_narrow_point_lets_only_the_host_own_names_beside_the_lists(self):
        copy = self.producer_copy("MECLAW_UNUSED_WORD", listed=False)
        for name, rc in (("PATH", 0), ("IFS", 0), ("TMPDIR", 0), ("LC_ALL", 0), ("HOME", 2),
                         ("MECLAW_LANE_NAME", 0)):
            ran = self.tmp / ("own-%s" % name)
            res = subprocess.run([str(copy), "probe", "local", "", "%s=/usr/bin:/bin " % name, str(ran)],
                                 capture_output=True, text=True, timeout=kit_time.HANG)
            with self.subTest(name=name):
                self.assertEqual(rc, res.returncode, res.stdout + res.stderr)
                self.assertEqual(rc == 0, ran.exists())
        self.assertEqual(("PATH", "IFS", "TMPDIR", "LC_ALL"), lane._shell_names("HOST_OWN"))

    def test_the_golden_hash_passes_the_narrow_point(self):
        """M7e2: `cmd_test` with a golden switch sends `bash -c $(q
        "$GOLDEN_HASH")` through `remote`; its `LC_ALL=C sort` is the host's
        own sort order. Without LC_ALL in HOST_OWN the narrow point refused
        every blessing run (exit 2). Same script on the Python way."""
        copy = self.producer_copy("MECLAW_UNUSED_WORD", listed=False)
        ran = self.tmp / "golden-hash"
        res = subprocess.run([str(copy), "probe", "local", "", "LC_ALL=C ", str(ran)],
                             capture_output=True, text=True, timeout=kit_time.HANG)
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        lane.remote_words_known("cd /t && bash -c %s" % lane.q(lane.GOLDEN_HASH))

    def test_the_python_ways_check_the_words_at_their_narrow_point(self):
        """OR-TV.tv-bn.M7e-1 (orchestrator: yes): `lane._remote_argv` and
        `measure.ssh_runner` send scripts to a lane outside `lane_sync.sh`;
        the same word check stands in front of both. An unlisted word is
        refused before any process starts (plain, behind a blank, after a
        newline, behind `;`), on a host and `local`; listed words pass, and
        measure's own script (TMPDIR, HOST_OWN) passes."""
        from scripts.pipeline import measure
        word = "MECLAW_A_NEW_PRODUCER_WORD"
        hosts = (lane.Lane(name="lane-z", ssh="u@lane-z.invalid"),
                 lane.Lane(name="lane-l", ssh="", transport="local"))
        started = []

        def no_start(argv, *a, **kw):
            started.append(argv)
            return subprocess.CompletedProcess(argv, 0, "", "")
        for script in ("%s=1 true" % word, "cd /t && %s=1 true" % word, "true\n%s=1 true" % word,
                       "true;%s=1 true" % word):
            for host in hosts:
                with self.subTest(script=script, transport=host.transport):
                    with self.assertRaisesRegex(lane.LaneError, "the lane word %s is not in LANE_WORDS" % word):
                        lane._remote_argv(host, script)
            with self.subTest(script=script, way="measure"):
                with unittest.mock.patch.object(measure.subprocess, "run", no_start):
                    with self.assertRaisesRegex(SystemExit, "the lane word %s is not in LANE_WORDS" % word):
                        measure.ssh_runner("u@lane-z.invalid")(script, 1)
        self.assertEqual([], started)
        for host in hosts:
            self.assertTrue(lane._remote_argv(host, "MECLAW_LANE_NAME=x PATH=/bin:$PATH x=1 true"))
        lane.remote_words_known(measure.timed_script("/t", "/tg", "", "true", "/l/x.log"))

    def test_the_kit_tests_take_their_deadlines_from_kit_time(self):
        """Re-check M7c M4 / M7d M3 / M7e A-M5 (TV-Z3, VORGABEN 3.5): a
        deadline of a kit test is the hang marker `kit_time.HANG`, never a
        number -- in EVERY Python file of `scripts/tests/` (git ls-files),
        not a hand list of modules. The plan data of the unit under test
        (DEADLINE_DATA) is the named rest, held EQUAL to the tree."""
        red, data = deadline_hits(REPO)
        self.assertEqual([], red)
        self.assertEqual({k: _data_count(v) for k, v in DEADLINE_DATA.items()}, data)

    @private_tree.private()   # GH #1199: the base is the private master
    def test_the_named_rest_only_shrinks_against_the_base_of_the_branch(self):
        """Re-check tv-bn M7h B-M2 (orchestrator OR-T10-2): `may only shrink`
        was pinned as equality with the tree -- a new key or a higher count
        in the same diff stayed green. Now a ratchet against the base."""
        self.assertEqual([], deadline_ratchet(REPO))

    def test_the_ratchet_refuses_growth_without_an_owner_ruling(self):
        """Mutants on a tree with a base: a count above the base, a key the
        base lacks are red; with `(n, "OR-...")` green; no base is red."""
        call = "def f(station):\n    station(timeout_min=5)\n"
        root = self.git_tree({"scripts/tests/test_x.py": call})
        git = lambda *a: subprocess.run(["git", "-C", str(root), "-c", "user.name=t", "-c", "user.email=t@t"]
                                        + list(a), check=True, capture_output=True, timeout=kit_time.HANG)
        self.assertEqual(["no base: git merge-base HEAD master|origin/master resolves nothing"],
                         deadline_ratchet(root, {"test_x.py:station:timeout_min": 1}))
        git("commit", "-q", "-m", "base")
        git("branch", "-M", "master")
        git("checkout", "-q", "-b", "strand")
        (root / "scripts/tests/test_x.py").write_text(call + "    station(timeout_min=6)\n")
        (root / "scripts/tests/test_y.py").write_text(call)
        git("add", "-A")
        git("commit", "-q", "-m", "grow")
        grown = {"test_x.py:station:timeout_min": 2, "test_y.py:station:timeout_min": 1}
        breaches = deadline_ratchet(root, grown)
        self.assertEqual(2, len(breaches), breaches)
        self.assertTrue(breaches[0].startswith("test_x.py:station:timeout_min: 2 > base 1"), breaches)
        self.assertTrue(breaches[1].startswith("test_y.py:station:timeout_min: 1 > base 0"), breaches)
        # the id must stand in the register (orchestrator OR-T10-12, re-check
        # tv-bn M7j B-M3: any `OR-...` string passed, the pin named a
        # non-existent OR-TV-99 green) -- the register of `testing_bans`
        unknown = {k: (n, "OR-TV-99") for k, n in grown.items()}
        self.assertEqual(2, len(deadline_ratchet(root, unknown)), "no register: an id holds nothing")
        reg = root / ".config" / "testing-rulings.toml"
        reg.parent.mkdir()
        scope = 'loosens = ["%s"]\n' % DEADLINE_LOOSENS
        own = '[[ruling]]\nid = "OR-T-1"\ndate = "2026-10-12"\nby = "coordinator"\ntext = "t"\n' + scope
        # re-check tv-bn M7m B-H1: the register of the strand's own tree -- in
        # the worktree or committed on the strand -- carries nothing; the
        # ratchet reads it at the base
        reg.write_text(own)
        own_ruled = {k: (n, "OR-T-1") for k, n in grown.items()}
        breaches = deadline_ratchet(root, own_ruled)
        self.assertEqual(2, len(breaches), breaches)
        self.assertIn("OR-T-1: the register .config/testing-rulings.toml does not exist", breaches[0])
        git("add", "-A")
        git("commit", "-q", "-m", "own ruling")
        self.assertEqual(2, len(deadline_ratchet(root, own_ruled)))
        git("reset", "-q", "--hard", "HEAD~1")
        # the ruling on master, the strand rebased onto it
        git("checkout", "-q", "master")
        reg.parent.mkdir(exist_ok=True)
        reg.write_text(own
                       + '[[ruling]]\nid = "OR-T-2"\ndate = "2026-10-12"\nby = "someone"\ntext = "t"\n' + scope
                       + '[[ruling]]\nid = "OR-T-3"\ndate = "2026-01-01"\nby = "owner"\ntext = "t"\n'
                       'expires = "2026-01-02"\n' + scope
                       # OR-T10-26 (re-check tv-bn M7l B-H1): valid, but scoped elsewhere or nowhere
                       + '[[ruling]]\nid = "OR-T-4"\ndate = "2026-10-12"\nby = "owner"\ntext = "t"\n'
                       'loosens = ["scripts/testing_bans.py"]\n'
                       '[[ruling]]\nid = "OR-T-5"\ndate = "2026-10-12"\nby = "owner"\ntext = "t"\n'
                       # re-check tv-bn M7m B-H3: a string under `loosens` (a substring
                       # matched), a malformed `expires` (never expired), a past TOML date
                       + '[[ruling]]\nid = "OR-T-6"\ndate = "2026-10-12"\nby = "owner"\ntext = "t"\n'
                       'loosens = "x %s y"\n' % DEADLINE_LOOSENS
                       + '[[ruling]]\nid = "OR-T-7"\ndate = "2026-10-12"\nby = "owner"\ntext = "t"\n'
                       'expires = "2026-1-2"\n' + scope
                       + '[[ruling]]\nid = "OR-T-8"\ndate = "2026-01-01"\nby = "owner"\ntext = "t"\n'
                       'expires = 2026-01-02\n' + scope
                       + '[[ruling]]\nid = "OR-T-9"\ndate = "2026-10-12"\nby = "owner"\ntext = "t"\n'
                       'expires = 2999-01-01\n' + scope
                       + '[[ruling]]\nid = "OR-T-10"\ndate = "2026-10-12"\nby = "owner"\ntext = "t"\n'
                       'paths = "scripts/tests/*"\n' + scope)
        git("add", "-A")
        git("commit", "-q", "-m", "register")
        git("checkout", "-q", "strand")
        git("rebase", "-q", "master")
        breaches = deadline_ratchet(root, unknown)
        self.assertEqual(2, len(breaches), breaches)
        self.assertIn("OR-TV-99 is not in the register", breaches[0])
        breaches = deadline_ratchet(root, {k: (n, "OR-T-4") for k, n in grown.items()})
        self.assertEqual(2, len(breaches), breaches)
        self.assertIn("OR-T-4 does not name %s under `loosens`" % DEADLINE_LOOSENS, breaches[0])
        for rid in ("OR-T-2", "OR-T-3", "OR-T-5", "OR-TV.tv-bn.M7e-1",   # wrong `by`, expired, no scope, strand
                    "OR-T-6", "OR-T-7", "OR-T-8", "OR-T-10"):               # the form of the entry (B-H3)
            with self.subTest(rid=rid):
                self.assertEqual(2, len(deadline_ratchet(root, {k: (n, rid) for k, n in grown.items()})))
        why = {rid: deadline_ratchet(root, {k: (n, rid) for k, n in grown.items()})[0]
               for rid in ("OR-T-6", "OR-T-7", "OR-T-8", "OR-T-10")}
        self.assertIn("`loosens` is not a list of strings", why["OR-T-6"])
        self.assertIn("`expires` is '2026-1-2', not a date", why["OR-T-7"])
        self.assertIn("OR-T-8 expired on 2026-01-02", why["OR-T-8"])
        self.assertIn("`paths` is not a list of strings", why["OR-T-10"])
        self.assertEqual([], deadline_ratchet(root, {k: (n, "OR-T-9") for k, n in grown.items()}))
        ruled = {k: (n, "OR-T-1") for k, n in grown.items()}
        self.assertEqual([], deadline_ratchet(root, ruled))
        self.assertEqual(1, len(deadline_ratchet(root, dict(ruled, **{"test_x.py:station:timeout_min": (2, "no")}))))
        self.assertEqual([], deadline_ratchet(root, {"test_x.py:station:timeout_min": 1}))

    def test_the_deadline_pin_reads_every_file_of_the_kit_tests(self):
        files = deadline_files(REPO)
        self.assertIn("scripts/tests/test_display_lab.py", files)
        self.assertIn("scripts/tests/test_pipeline_hygiene.py", files)
        self.assertNotIn("scripts/tests/kit_time.py", files)
        root = self.git_tree({"scripts/tests/test_new.py": "def f(ev):\n    ev.wait(10)\n",
                              "scripts/tests/sub/test_deep.py": "import subprocess\n"
                                                                "subprocess.run(['x'], timeout=2 * 30)\n",
                              "scripts/other/test_x.py": "def f(ev):\n    ev.wait(10)\n"})
        self.assertEqual(["sub/test_deep.py:2 run:timeout", "test_new.py:2 wait:"], deadline_hits(root)[0])

    def test_the_deadline_pin_sees_every_form(self):
        """Re-check M7d M3 / M7e A-M5(b) (mutants): each form of a literal
        deadline alone is red; the hang marker, its multiples, a zero (a
        probe, no wait) and a mapping's default pass."""
        red = ("p.wait(10)", "th.join(30)", "q.get(5)", "fut.result(2.5)", "lock.acquire(timeout=3)",
               "subprocess.run(a, timeout=60)", "proc.communicate(timeout=9)",
               "kit_time.until('x', ok, 5)", "kit_time.until('x', ok, deadline=5)",
               "def f(timeout_s=30):\n    pass", "def f(*, deadline=1.5):\n    pass", "ev.wait(timeout=10)",
               # A-M5(b): literal arithmetic, sign, conversion; further positions, names, keywords
               "subprocess.run(a, timeout=10 * 2)", "subprocess.run(a, timeout=float(30))",
               "subprocess.run(a, timeout=-1)", "q.get(True, 5)", "futures.wait(fs, 10)",
               "sel.select(5)", "select.select(r, [], [], 5)", "asyncio.wait_for(c, 10)",
               "cond.wait_for(pred, 10)", "x.run(timeout_secs=5)", "x.run(timeout_ms=500)",
               "x.run(deadline_s=5)", "x.run(wait_s=5)", "x.poll(wait=3)",
               "f = lambda timeout=5: None")
        green = ("p.wait(kit_time.HANG)", "th.join(2 * kit_time.HANG)", "subprocess.run(a, timeout=kit_time.HANG)",
                 "def f(timeout_s=4 * kit_time.HANG):\n    pass", "ev.wait(0)", "', '.join(parts)",
                 "kit_time.until('x', ok)", "d.get('k')", "doc.get('threshold_s', 30)", "x.run(timeout=None)")
        for text in red:
            with self.subTest(text):
                self.assertEqual([1], sorted({line for line, _ in literal_deadlines(text)}))
        for text in green:
            with self.subTest(text):
                self.assertEqual([], literal_deadlines(text))

    def test_a_template_names_the_marker_once(self):
        with self.assertRaisesRegex(AssertionError, "names the marker exactly once, found 0"):
            fake_ssh.script("#!/bin/sh\nexec bash -c \"$*\"\n")



class SameDecision(unittest.TestCase):
    """Re-check tv-bn M7j A-M2/A-M3: one set of probes, Python
    `lane.ssh_program` and Bash `ssh_bin` decide the same, and each as
    expected -- admission (a) by owner root and no group/other write bit up
    to `/`, not by momentary writability (an own 0555 file passed); the Bash
    check's tools neither from PATH nor from an exported function (P6/P6b),
    an entry's hash 64 hex digits (P6c `@x`)."""

    def setUp(self):
        self.dir = pathlib.Path(tempfile.mkdtemp(prefix="same-"))
        self.addCleanup(self._cleanup)
        self.mark = self.dir / "RAN"

    def _cleanup(self):
        for root, dirs, _ in os.walk(self.dir):
            for d in dirs:
                os.chmod(os.path.join(root, d), 0o755)
        shutil.rmtree(self.dir, True)

    def fake(self, name, registered=True):
        d = self.dir / name
        d.mkdir()
        env = {}
        path = os.path.realpath(fake_ssh.install_inert(d, "same-decision probe " + name, log=self.mark, rc=0,
                                                       env=env))
        return d, path, (env[lane.FAKE_SSH_REGISTRY] if registered else "")

    def probes(self):
        """name -> (environment, SYSTEM_SSH_DIRS or None, expected path or None)."""
        trapped = store_isolation.trapped_path()
        out = {}
        sysdirs = [d for d in lane.SYSTEM_SSH_DIRS if shutil.which("ssh", path=d)]
        if sysdirs:
            own = os.path.realpath(shutil.which("ssh", path=sysdirs[0]))
            out["system ssh"] = ({"PATH": sysdirs[0]}, None, own)
        d, path, reg = self.fake("registered")
        out["registered fake"] = ({"PATH": "%s:%s" % (d, trapped), lane.FAKE_SSH_REGISTRY: reg}, None, path)
        out["registered fake behind a relative entry"] = (
            {"PATH": "registered:%s:%s" % (d, trapped), lane.FAKE_SSH_REGISTRY: reg}, None, path)
        d, path, _ = self.fake("unregistered")
        out["unregistered fake"] = ({"PATH": "%s:%s" % (d, trapped), lane.FAKE_SSH_REGISTRY: ""}, None, None)
        out["no registry"] = ({"PATH": "%s:%s" % (d, trapped)}, None, None)
        out["empty PATH"] = ({"PATH": "", lane.FAKE_SSH_REGISTRY: reg}, None, None)
        out["relative PATH only"] = ({"PATH": "registered", lane.FAKE_SSH_REGISTRY: reg}, None, None)
        true = shutil.which("true", path=trapped)
        link = self.dir / "link"
        link.mkdir()
        os.symlink(true, link / "ssh")
        out["link to another system program"] = ({"PATH": "%s:%s" % (link, trapped)}, None, None)
        d, path, reg = self.fake("rewritten")
        pathlib.Path(path).write_text("#!/bin/sh\ntouch %s\n" % shlex.quote(str(self.mark)), encoding="utf-8")
        out["rewritten body"] = ({"PATH": "%s:%s" % (d, trapped), lane.FAKE_SSH_REGISTRY: reg}, None, None)
        d, path, _ = self.fake("ro0555")
        os.chmod(path, 0o555)
        os.chmod(d, 0o555)
        out["own 0555 file in an own 0555 system dir"] = ({"PATH": "%s:%s" % (d, trapped)}, (str(d),), None)
        d, path, _ = self.fake("writable")
        out["own file in a writable system dir"] = ({"PATH": "%s:%s" % (d, trapped)}, (str(d),), None)
        d, path, _ = self.fake("unreadable")
        os.chmod(path, 0o111)
        out["unreadable body, entry @x"] = (
            {"PATH": "%s:%s" % (d, trapped), lane.FAKE_SSH_REGISTRY: path + "@x"}, None, None)
        out["unreadable body, entry @<64 x>"] = (
            {"PATH": "%s:%s" % (d, trapped), lane.FAKE_SSH_REGISTRY: path + "@" + "x" * 64}, None, None)
        d, path, _ = self.fake("shadowed")
        shadow = d / "realpath"
        # the lie names the system's ssh by the kit's own list, a datum the
        # shadow prints, not a tool the test calls
        lie = shlex.quote(os.path.join(lane.SYSTEM_SSH_DIRS[0], "ssh"))
        shadow.write_text('#!/bin/sh\nfor a; do case "$a" in */ssh) echo %s; exit 0;; esac; done\n'
                          'exec /usr/bin/realpath "$@"\n' % lie, encoding="utf-8")
        shadow.chmod(0o755)
        out["realpath shadowed on PATH (P6)"] = ({"PATH": "%s:%s" % (d, trapped)}, None, None)
        d, path, _ = self.fake("plain")
        for fn, body in (("realpath", "() { echo %s; }" % lie), ("command", "() { echo %s; }" % lie),
                         ("sha256sum", "() { echo x; }"), ("stat", "() { echo 0:755; }"),
                         ("continue", "() { :; }"), ("read", "() { :; }"), ("test", "() { return 0; }"),
                         ("[", "() { return 0; }"), ("echo", "() { builtin echo %s; }" % lie)):
            out["exported function %s (P6b)" % fn] = (
                {"PATH": "%s:%s" % (d, trapped), "BASH_FUNC_%s%%%%" % fn: body}, None, None)
        out["exported stat, own 0555 file in a system dir"] = (
            {"PATH": "%s:%s" % (self.dir / "ro0555", trapped), "BASH_FUNC_stat%%": "() { echo 0:755; }"},
            (str(self.dir / "ro0555"),), None)
        # Re-check tv-bn M7l A-M1: the caller's code in the checking shell
        # (P11-P14) -- `die` that returns, a `local` that initialises
        # nothing, a `printf` that names the lie, BASH_ENV/ENV read before the
        # script, the check's globals readonly beforehand. The Bash column
        # runs the executed script (`#!/bin/bash -p`), as every caller does.
        d, path, _ = self.fake("caller-code")
        hand = {"PATH": "%s:%s" % (d, trapped)}
        for fn, body in (("exit", "() { :; }"), ("local", "() { :; }"), ("exec", "() { :; }"),
                         ("printf", "() { builtin printf '%%s\\n' %s; }" % lie)):
            out["exported function %s (P11)" % fn] = (dict(hand, **{"BASH_FUNC_%s%%%%" % fn: body, "ok": "1"}),
                                                      None, None)
        exit_rc = self.dir / "env-exit"
        exit_rc.write_text("exit() { :; }\n", encoding="utf-8")
        out["BASH_ENV defines exit() (P13)"] = (dict(hand, BASH_ENV=str(exit_rc)), None, None)
        out["ENV defines exit()"] = (dict(hand, ENV=str(exit_rc)), None, None)
        liar = self.dir / "liar"
        liar.mkdir()
        (liar / "stat").write_text('#!/bin/sh\nshift 3\nfor a; do echo 0:755; done\n', encoding="utf-8")
        (liar / "stat").chmod(0o755)
        ro = self.dir / "env-readonly"
        ro.write_text("readonly SSH_STAT=%s\nreadonly -a SYSTEM_SSH_DIRS=(%s)\n"
                      % (shlex.quote(str(liar / "stat")), shlex.quote(str(d))), encoding="utf-8")
        out["BASH_ENV sets the check's globals readonly (P14)"] = (dict(hand, BASH_ENV=str(ro)), None, None)
        out["SHELLOPTS/BASHOPTS from the caller"] = (dict(hand, SHELLOPTS="xtrace", BASHOPTS="extdebug"),
                                                     None, None)
        # Re-check tv-bn M7l A-M2: a FIFO named `ssh` -- Bash skipped it
        # (`-f`), Python took it (`which`); a candidate is a regular
        # executable file on both sides.
        fifo = self.dir / "fifo"
        fifo.mkdir()
        os.mkfifo(fifo / "ssh")
        os.chmod(fifo / "ssh", 0o755)
        if sysdirs:
            out["FIFO named ssh first, the system's behind (P16)"] = (
                {"PATH": "%s:%s" % (fifo, sysdirs[0])}, None, own)
        out["FIFO named ssh the registry names (P16b)"] = (
            {"PATH": str(fifo), lane.FAKE_SSH_REGISTRY: "%s@%s" % (fifo / "ssh", "0" * 64)}, None, None)
        return out

    def py(self, env, sysdirs):
        with unittest.mock.patch.object(lane, "SYSTEM_SSH_DIRS", sysdirs or lane.SYSTEM_SSH_DIRS):
            try:
                return lane.ssh_program(env)
            except lane.LaneError:
                return None

    def sh(self, env, sysdirs):
        """The Bash decision: the EXECUTED script (`lane_sync.sh checked
        ssh`, its shebang `#!/bin/bash -p` -- re-check tv-bn M7l A-M1: the
        probes sourced the functions into a `bash -c`, where the caller's
        functions and BASH_ENV act); only a probe that moves SYSTEM_SSH_DIRS
        sources them behind that assignment."""
        if sysdirs:
            pre = "SYSTEM_SSH_DIRS=(%s); " % " ".join(shlex.quote(s) for s in sysdirs)
            argv = ["/bin/bash", "-c", 'source "$0"; ' + pre + 'ssh_bin; printf "%s\\n" "$SSH_CHECKED"',
                    _fns()]
        else:
            argv = [lane.LANE_SYNC, "checked", "ssh"]
        res = subprocess.run(argv, env=env, capture_output=True, text=True, timeout=kit_time.HANG, cwd=self.dir)
        if res.returncode == 0 and res.stdout.endswith("\n") and res.stdout.count("\n") == 1:
            return res.stdout[:-1] or None
        self.assertEqual(2, res.returncode, res.stdout + res.stderr)
        return None

    def test_python_and_bash_decide_the_same_on_every_probe(self):
        probes = self.probes()
        self.assertGreaterEqual(len(probes), 20)
        got = {name: (self.py(env, dirs), self.sh(env, dirs)) for name, (env, dirs, _) in probes.items()}
        want = {name: (exp, exp) for name, (_, _, exp) in probes.items()}
        self.assertEqual(want, got)
        self.assertFalse(self.mark.exists())

    def test_a_root_mode_is_read_as_a_number_on_either_side(self):
        """Re-check tv-bn M7l A-M2: Bash read the mode by a digit regex that
        wanted two digits (a root mode below 0o10 was refused there, admitted
        in Python). Both sides see the same lie of owner and mode over the
        whole chain; only the group/other write bits decide."""
        d, path, _ = self.fake("modes")
        liar = self.dir / "modes-stat"
        liar.mkdir()
        real_stat = os.stat
        for mode, admitted in ((0o005, True), (0o0, True), (0o1755, True), (0o755, True),
                               (0o020, False), (0o002, False), (0o775, False)):
            with self.subTest(mode=oct(mode)):
                def lie(p, *a, **kw):
                    st = real_stat(p, *a, **kw)
                    v = list(st)
                    v[0], v[4] = stat.S_IFMT(st.st_mode) | mode, 0
                    return os.stat_result(v)
                with unittest.mock.patch.object(lane.os, "stat", lie):
                    py = self.py({"PATH": str(d)}, (str(d),))
                (liar / "stat").write_text('#!/bin/sh\nshift 3\nfor a; do echo 0:%o; done\n' % mode,
                                           encoding="utf-8")
                (liar / "stat").chmod(0o755)
                res = subprocess.run(["/bin/bash", "-c", 'source "$0"; ' + "SSH_STAT=%s; SYSTEM_SSH_DIRS=(%s); ssh_bin; "
                                      'printf "%%s\\n" "$SSH_CHECKED"' % (shlex.quote(str(liar / "stat")),
                                                                        shlex.quote(str(d))), _fns()],
                                     env={"PATH": str(d)}, capture_output=True, text=True,
                                     timeout=kit_time.HANG, cwd=self.dir)
                sh = res.stdout.strip() if res.returncode == 0 else None
                self.assertEqual((path if admitted else None,) * 2, (py, sh), res.stderr)

    def test_a_registered_fifo_never_blocks_the_hash(self):
        """Re-check tv-bn M7l P16b: the hash read a FIFO the registry named
        and blocked without a HANG line; it reads only a regular file."""
        fifo = self.dir / "ssh"
        os.mkfifo(fifo)
        res = subprocess.run(["/usr/bin/python3", "-I", "-c",
                              "import sys; sys.path.insert(0, sys.argv[1]); from scripts.pipeline import lane; "
                              "print(lane._registered_fake(sys.argv[2], sys.argv[2] + '@' + '0' * 64))",
                              str(REPO), str(fifo)], capture_output=True, text=True, timeout=kit_time.HANG)
        self.assertEqual(("False\n", 0), (res.stdout, res.returncode), res.stderr)


if __name__ == "__main__":
    unittest.main()
