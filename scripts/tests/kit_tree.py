"""The kit libraries a test tree needs next to a copied kit shell entry (M7w),
and the one reading of how a kit shell file sources and locates itself.

A kit shell file sources its libraries itself, fail-closed and from its own
directory: the whole line `. "$<dir>/<library>" || exit 2`, where `<dir>` is
set in the same file with builtins only (`case "${BASH_SOURCE[0]}" in */*)
<dir>=${BASH_SOURCE[0]%/*}/ ...`, then `<dir>=$(cd -- "$<dir>..." && pwd) ||
exit 2`). test_lane_engstelle KitShellClass pins that over every kit shell
file -- every `.`/`source` wherever it stands, in the code and in double
quotes (re-check bundle N N-K4; delta M1: no list of command positions), the
words that are no source named with their reason (`NOT_SOURCES`). A test that copies such an entry into a
throw-away tree seeds the tree from here: the entry's own libraries and theirs
(M7y: `test-tier.sh` sources `cargo-target.sh`, which sources
`pipeline/kit_python.sh`). The list is read from the files' own source lines
-- no second inventory of kit files to keep in step, no hand copy.
"""
import ast
import pathlib
import re
import shutil
import subprocess

SCRIPTS = pathlib.Path(__file__).resolve().parent.parent

# A kit shell file: `*.sh`, or a file whose first line is a shell shebang
# (`env`, `env -S`, bash/sh/dash/ksh; re-check bundle N H1).
SHEBANG = re.compile(r"^#!\s*(?:/usr/bin/env\s+(?:-S\s+)?)?(?:/\S*/)?(?:ba|da|k)?sh\b")


def shell_files(repo=None):
    """Every kit shell file under scripts/ -- found, not listed: tracked, `*.sh`
    or a shell shebang, at any depth (OR-T12-18, re-check bundle N N-K6)."""
    repo = pathlib.Path(repo) if repo else SCRIPTS.parent
    out = []
    for rel in subprocess.run(["git", "-C", str(repo), "ls-files", "-z", "scripts"],
                              capture_output=True, check=True).stdout.decode().split("\0"):
        path = repo / rel
        if not rel or not path.is_file():
            continue
        if rel.endswith(".sh"):
            out.append(rel)
            continue
        with open(path, "rb") as fh:
            head = fh.readline(200).decode("utf-8", "replace")
        if SHEBANG.match(head):
            out.append(rel)
    return sorted(out)


def split(text):
    """`text` (shell) as (code, snippets): `code` keeps every byte position and
    line of the file, with comments and the bodies of single-quoted strings
    and here-documents blanked; `snippets` are those bodies (code run
    elsewhere: `bash -c '...'`, a string for the lane host, a heredoc), each
    as (line, text). A `$(...)` or a backtick inside double quotes is code
    again, its quotes nest (re-check bundle N delta kit-shell M1 b: an
    apostrophe in `"$(printf "%s" "can't")"` opened a phantom snippet). A
    quoted or split word `.`/`source` is the bare word first (`bare_source_words`)."""
    text = bare_source_words(text)
    out, snippets, i, n = list(text), [], 0, len(text)
    word_start, heredocs = True, []
    # the open contexts: "dq" (double quotes), "bt" (a backtick), [depth] (a `$(`)
    stack = []

    def blank(a, b):
        for k in range(a, b):
            if out[k] != "\n":
                out[k] = " "

    while i < n:
        c = text[i]
        top = stack[-1] if stack else None
        if c == "\n":
            word_start = True
            i += 1
            for strip, delim in heredocs:  # bodies start on the next line
                start, end = i, i
                while end < n:
                    eol = text.find("\n", end)
                    eol = n if eol < 0 else eol
                    line = text[end:eol]
                    if (line.lstrip("\t") if strip else line) == delim:
                        break
                    end = eol + 1
                snippets.append((text.count("\n", 0, start) + 1, text[start:end]))
                blank(start, end)
                i = min(n, text.find("\n", end) + 1 if text.find("\n", end) >= 0 else n)
            heredocs = []
            continue
        if top == "dq":
            if c == "\\":
                i += 2
                continue
            if c == '"':
                stack.pop()
            elif text.startswith("$(", i):
                stack.append([0])
                i += 2
                word_start = True
                continue
            elif c == "`":
                stack.append("bt")
                word_start = True
            i += 1
            continue
        if c == "\\":
            i += 2
            word_start = False
            continue
        if c == "#" and word_start:
            end = text.find("\n", i)
            end = n if end < 0 else end
            blank(i, end)
            i = end
            continue
        if c == "'":
            ansi = i > 0 and text[i - 1] == "$"
            j = i + 1
            while j < n and text[j] != "'":
                j += 2 if ansi and text[j] == "\\" else 1
            snippets.append((text.count("\n", 0, i) + 1, text[i + 1:j]))
            blank(i + 1, j)
            i = j + 1
            word_start = False
            continue
        if c == '"':
            stack.append("dq")
            i += 1
            word_start = False
            continue
        if top == "bt" and c == "`":
            stack.pop()
            i += 1
            word_start = False
            continue
        if isinstance(top, list):
            if c == "(":
                top[0] += 1
            elif c == ")":
                if top[0] == 0:
                    stack.pop()
                    i += 1
                    word_start = False
                    continue
                top[0] -= 1
        m = re.compile(r"<<(-?)[ \t]*(['\"]?)([A-Za-z_]\w*)\2").match(text, i)
        if m and not text.startswith("<<<", i):
            heredocs.append((m.group(1) == "-", m.group(3)))
            i = m.end()
            word_start = False
            continue
        word_start = c in " \t;&|(){}<>`"
        i += 1
    return "".join(out), snippets


# `.` or `source` anywhere (re-check bundle N delta kit-shell M1 a, the
# re-checker's decision: no list of command positions -- `if`, a case arm,
# `command`/`builtin`, a backtick, the head of a double-quoted `bash -c`/
# `ssh`/`eval` string all escaped the last one): every word `.`/`source` in
# the code and in double quotes is judged, `\.`/`\source` too (the builtin,
# the backslash only skips an alias; rest list N-R1). A `.` inside a word
# (`a.b`, `./x`, `$.`, `*.` of a glob) is none. Its target is a word; a
# target that is prose ("nothing else. The run", "the source of truth") is
# not a source (`_is_path`); `after` is the rest of its line. `--` ends the
# options, the target is the word after it; `(` starts no word.
SOURCE_AT = re.compile(r"(?<![\w./$*-])(?P<cmd>\.|source)[ \t]+(?:--[ \t]+)?"
                       r"(?P<target>\\?\"[^\"\n]*\"|'[^'\n]*'|[^\s;&|()`]+)(?=(?P<after>[^\n]*))")
# The one form at file level: a whole line, own-directory variable, a library
# below scripts/ (the variable and a `scripts/` step dropped), fail-closed.
SOURCE_LINE = re.compile(r'^[ \t]*\. "\$\{?(?P<var>\w+)\}?/?(?:scripts/)?(?P<lib>[\w./-]+\.sh)" \|\| exit 2[ \t]*$')
# The one form inside a snippet (its location is handed in by whoever runs
# it -- a `cd`, an argument): a quoted variable path, fail-closed, the
# command ends there.
SNIPPET_SOURCE = re.compile(r'\. "\$\{?\w+\}?[\w./-]*" \|\| exit 2(?=[ \t]*(?:;|\n|\}|$))')


# A word of a shell file the shell reads as `.`/`source` although it is
# quoted or split (`"." "$1"`, `sour""ce`, `'.'`, `\source`; rest list N-R2):
# the word at a word start, quotes and backslashes only, then a blank.
_QUOTED_WORD = re.compile(r"""(?:(?<=[\s;&|(){}<>`])|^)"""
                          r"""(?P<w>(?:'[^'\n]*'|"(?:[^"\\\n]|\\.)*"|\\[^\n]|[^\s;&|(){}<>`'"\\])+)(?=[ \t])""")


def _unquoted(word):
    return re.sub(r"""'([^']*)'|"((?:[^"\\]|\\.)*)"|\\(.)""",
                  lambda m: m.group(1) if m.group(1) is not None else
                  re.sub(r"\\(.)", r"\1", m.group(2)) if m.group(2) is not None else m.group(3), word)


def bare_source_words(text):
    """`text` with every quoted or split word `.`/`source` written bare,
    blank-padded to its length (every position and line stays)."""
    def bare(m):
        w = m.group("w")
        if not re.search(r"""['"\\]""", w) or _unquoted(w) not in (".", "source"):
            return w
        return _unquoted(w).ljust(len(w))
    return _QUOTED_WORD.sub(bare, text)


# A target that is prose or punctuation, not a path (rest list N-R2: every
# other target is judged -- `/etc/os-release`, `./env`, `~/.bashrc`, `x.conf`,
# a quoted word). A plain word (letters, digits, `_`, `'`/`-` inside) is a
# file name only where its command ends with it: an operator follows
# (`. env; x`, `$(. env)`, `. env >/dev/null`), or the command stands alone
# (`. env` as the whole line, after `&&`/`;`/`(`/a backtick, at the head of a
# quoted string for another shell). Before a sentence mark or more words it
# is prose ("connect point. An", "the source of truth"); a target without a
# letter, digit or path sign (`=`, `]]`, `->`) is none.
_PROSE_WORD = re.compile(r"[\w'-]+(?P<mark>[.,:!?]?)")
_COMMAND_END = re.compile(r"[ \t]*[;&|)}<>]|`")
_ALONE_BEFORE = re.compile(r"(?:^|[;&|({`\"'])[ \t]*$")
_ALONE_AFTER = re.compile(r"[ \t]*[\"']?[ \t]*$")


def _is_path(target, after="", before=""):
    if not re.search(r"[\w/.$~%{\"'\\]", target):
        return False
    word = _PROSE_WORD.fullmatch(target)
    if word is None:
        return True
    if word.group("mark"):
        return False
    return (_COMMAND_END.match(after) is not None
            or (_ALONE_BEFORE.search(before) is not None and _ALONE_AFTER.match(after) is not None))


# A placeholder of a Python string the shell sees as a path (`%s`, `%(lib)s`,
# `{lib}`/`{}` of `.format`; re-check bundle N delta kit-shell M3, g15).
PY_PLACEHOLDER = re.compile(r"%[(s]|\{")


def source_hits(code, is_path=_is_path):
    """Every `.`/`source` of a path in `code`, wherever it stands, as
    (line, match)."""
    code = bare_source_words(code)
    return [(code.count("\n", 0, m.start("cmd")) + 1, m) for m in SOURCE_AT.finditer(code)
            if is_path(m.group("target"), m.group("after"),
                       code[code.rfind("\n", 0, m.start("cmd")) + 1:m.start("cmd")])]


def _line(code, pos):
    a = code.rfind("\n", 0, pos) + 1
    b = code.find("\n", pos)
    return code[a:len(code) if b < 0 else b]


# `<v>` set by builtins only from the file's own path (`$0` for a file that
# is run, never sourced), then resolved by `cd`/`pwd`, fail-closed.
SELF_CASE = (r'(?m)^[ \t]*case "\$(?:\{BASH_SOURCE\[0\]\}|0)" in \*/\*\) (?P<var>\w+)='
             r'\$\{(?:BASH_SOURCE\[0\]|0)%/\*\}/ ;; \*\) (?P=var)=\./ ;; esac\b')
_ASSIGN = r"(?<![\w$.{-])%s\+?="


# Every other way a builtin sets `<v>` (re-check bundle N delta kit-shell M2,
# g11): `read`, `printf -v`, `mapfile`/`readarray`, `getopts`, a `for` loop,
# `${v:=...}`/`${v=...}`, `unset`, a declaration without a value, `(( v = ))`.
_OTHER_SET = (r"\b(?:read|mapfile|readarray|getopts)\b[^\n;&|]*?(?<![\w$])%(v)s\b(?![\w=])"
              r"|\bprintf[ \t]+-v[ \t]*%(v)s\b"
              r"|\bfor[ \t]+%(v)s[ \t]+in\b"
              r"|\$\{%(v)s:?="
              r"|\bunset\b[^\n;&|]*?(?<![\w$])%(v)s\b"
              r"|\b(?:declare|local|typeset|export|readonly)\b(?:[ \t]+-\w+)*[^\n;&|=]*?"
              r"(?<![\w$])%(v)s\b(?![\w=\[])"
              r"|\(\([ \t]*%(v)s[ \t]*[-+*/%%]?=")


def self_located(code, var, pos=None):
    """Is `$<var>` this file's own directory at `pos` (a source there; None:
    the end of the file) -- set before `pos` with builtins only, resolved
    before it, and never set otherwise (re-check bundle N M1 b; delta M2: the
    order, g10, and every other setter, g11)? `$PWD`, `$HOME` and a name
    inherited from the caller are not."""
    pos = len(code) if pos is None else pos
    own = [m for m in re.finditer(SELF_CASE, code) if m.group("var") == var]
    if len(own) != 1 or own[0].start() >= pos:
        return False
    resolve = re.compile(r'^[ \t]*%s=\$\(cd -- "\$\{?%s\}?[\w./]*" && pwd\) \|\| exit 2[ \t]*$' % (var, var))
    for m in re.finditer(_ASSIGN % re.escape(var), code):
        if own[0].start() <= m.start() < own[0].end():
            continue
        if not resolve.match(_line(code, m.start())) or m.start() >= pos:
            return False
    return re.search(_OTHER_SET % {"v": re.escape(var)}, code) is None


def source_lines(text):
    """The file-level source lines of a kit shell file's text, in source order:
    (line, the line, var, lib) -- `var`/`lib` None where the line is not the
    one form."""
    code, _ = split(text)
    out = []
    for line, m in source_hits(code):
        whole = _line(code, m.start("cmd"))
        form = SOURCE_LINE.match(whole)
        out.append((line, whole.strip(), form and form.group("var"), form and form.group("lib")))
    return out


def source_positions(text):
    """`source_lines` with the position of each source in the blanked code:
    (line, the line, var, lib, pos) -- `self_located(code, var, pos)`."""
    code, _ = split(text)
    return [(line, w, v, lib, m.start("cmd"))
            for (line, w, v, lib), (_, m) in zip(source_lines(text), source_hits(code))]


# Every `.`/`source` of a path the scan judges that is no source of a kit
# library, each named with its file, its text (the line from the word on; at
# file level the whole line) and its reason (OR-T12-18; re-check bundle N
# delta kit-shell M3: a generic rule -- "a target that names no kit library"
# -- let every `. "$1"` through, and the scan judges every word now, M1).
# Everything else is the file-level or the snippet form; an entry no file
# has any more is stale (test_lane_engstelle KitShellClass). The level is part
# of the text (re-check bundle N delta FN2-d, [NS iii]): a snippet's command
# is named with `SNIPPET_MARK`, a comment at file level -- a snippet entry
# never names a file line (an open `. "$1"` straight in the file stays red),
# and a file-level entry never a snippet.
SNIPPET_MARK = "  # in a snippet"
_PROSE = "prose of a message, no command: the word `source` before a value"
NOT_SOURCES = {
    ("scripts/pipeline/lane_fingerprint.sh", '. "$1" >/dev/null 2>&1; shift; exec node -e "$@"  # in a snippet'):
        "the lane's browser env (bp_wkenv), not a kit library: sourced on purpose without a stop",
    ("scripts/publish_docs.sh",
     '|| die "source \'$SOURCE\' is neither a directory with docs/ nor a revision in $REPO_ROOT"'): _PROSE,
    ("scripts/publish_docs.sh", 'say "source   $SOURCE_DESC"'): _PROSE,
    ("scripts/test-tier.sh", 'echo "test-tier: LISTED $(grep -c . "$seen")"'):
        "the pattern `.` of grep (any line), no command",
    ("scripts/pipeline/grafana_push.py", "source $X: HTTP $X"): _PROSE,
    ("scripts/pipeline/locks.py", "source %s is not an object"): _PROSE,
    ("scripts/pipeline/locks.py", "source %s names neither tag nor commit"): _PROSE,
    ("scripts/pipeline/locks.py", "source %s: repository %s is not here, pins unchecked"): _PROSE,
    ("scripts/pipeline/locks.py", "source %s: %s %s unknown in %s (or git failed), pins unchecked"): _PROSE,
    ("scripts/pipeline/locks.py", "source %s: %s %s has no %s"): _PROSE,
}


def snippet_sources(text, rel=None):
    """Every source inside the snippets of a shell file's text (the strings
    for another shell, the heredocs): (line, the command text, is it the
    snippet form or a named exclusion of scripts/<rel>); the command text
    carries `SNIPPET_MARK`."""
    out = []
    for first, body in split(text)[1]:
        for line, m in source_hits(body):
            cmd = body[m.start("cmd"):].split("\n", 1)[0] + SNIPPET_MARK
            ok = SNIPPET_SOURCE.match(body, m.start("cmd")) is not None or (rel, cmd) in NOT_SOURCES
            out.append((first + line - 1, cmd, ok))
    return out


def _py_strings(tree):
    """(line, text) of every string of a Python tree as the shell gets it: an
    f-string with `$X` for each value it formats (g16), its parts not twice.
    A string that is a statement of its own (a docstring) reaches nothing --
    its value is dropped -- and is prose, not a string (rest list N-R2)."""
    parts = {id(v) for n in ast.walk(tree) if isinstance(n, ast.JoinedStr) for v in n.values}
    parts |= {id(n.value) for n in ast.walk(tree)
              if isinstance(n, ast.Expr) and isinstance(n.value, ast.Constant) and isinstance(n.value.value, str)}
    for node in ast.walk(tree):
        if isinstance(node, ast.JoinedStr):
            yield node.lineno, "".join(v.value if isinstance(v, ast.Constant) else "$X"
                                       for v in node.values)
        elif isinstance(node, ast.Constant) and isinstance(node.value, str) and id(node) not in parts:
            yield node.lineno, node.value


def python_sources(text):
    """The sources inside the strings of a Python file's text (the `bash -c`
    strings of the kit's Python side): (line, the command text, is it the
    snippet form) -- `.` or `source`, any quoting (N-K4 c); a `%s`/`{...}`
    placeholder or an f-string value is a path (delta M3, g15/g16)."""
    out = []

    def is_path(target, after="", before=""):
        return _is_path(target, after, before) or PY_PLACEHOLDER.search(target) is not None

    for lineno, value in _py_strings(ast.parse(text)):
        for line, m in source_hits(value, is_path):
            ok = SNIPPET_SOURCE.match(value, m.start("cmd")) is not None
            out.append((lineno + line - 1, value[m.start("cmd"):].split("\n", 1)[0], ok))
    return out


# Self-location by an external tool (re-check bundle N M2): a variable that
# holds this file's own path -- `$0`, BASH_SOURCE, or one assigned a word that
# names them in any expansion (delta M4: a taint, `${BASH_SOURCE[0]:-$0}`,
# `${self#./}`) -- given to `dirname`, `readlink` or `realpath`. Off PATH (`dirname`) the answer was
# the caller's cwd (M7w); the kit's form is the builtin `${v%/*}`.
_REF = re.compile(r"\$\{?(\w+)|\b(BASH_SOURCE)\b")
_TOOL = re.compile(r"\b(dirname|readlink|realpath)\b")


def _names(s):
    return {a or b for a, b in _REF.findall(s)}


def _word(s):
    """The shell word at the start of `s`: up to a blank, `;`, `&`, `|` or a
    `)` outside quotes, `$(...)` and `${...}`."""
    depth, q, j = 0, None, 0
    while j < len(s):
        c = s[j]
        if c == "\\":
            j += 2
            continue
        if q:
            if c == q:
                q = None
            elif q == '"' and s.startswith("$(", j):
                depth += 1
                j += 1
            elif q == '"' and c == ")" and depth:
                depth -= 1
        elif c in "\"'":
            q = c
        elif s.startswith(("$(", "${"), j):
            depth += 1
            j += 1
        elif c in ")}" and depth:
            depth -= 1
        elif c in " \t\n;&|)" and not depth:
            break
        j += 1
    return s[:j]


def self_names(code):
    """The names holding this file's own path (see above), `0` and
    `BASH_SOURCE` included: every name assigned a word that names one of them
    in any expansion (re-check bundle N delta kit-shell M4: a taint, not a list
    of forms -- `${BASH_SOURCE[0]:-$0}`, `${self#./}` broke the chain)."""
    names = {"0", "BASH_SOURCE"}
    assigns = [(m.group(1), _names(_word(code[m.end():])))
               for m in re.finditer(r"(?<![\w$.{-])(\w+)=", code)]
    grew = True
    while grew:
        grew = False
        for var, refs in assigns:
            if var not in names and refs & names:
                names.add(var)
                grew = True
    return names


def self_by_tool(text):
    """The lines of a shell file's text that hand its own path to
    `dirname`/`readlink`/`realpath`: (line, the line)."""
    code, _ = split(text)
    names = self_names(code)
    out = []
    for m in _TOOL.finditer(code):
        arg = re.match(r"[^)\n;&|]*", code[m.end():]).group(0)
        if _names(arg) & names:
            out.append((code.count("\n", 0, m.start()) + 1, _line(code, m.start()).strip()))
    return out


def sourced(rel):
    """The kit libraries (relative to scripts/) that scripts/<rel> sources, in
    source order."""
    text = (SCRIPTS / rel).read_text(encoding="utf-8")
    out = []
    for _, _, var, lib in source_lines(text):
        if lib and lib not in out:
            out.append(lib)
    return out


def closure(*rels):
    """Every kit library the entries scripts/<rels> source, transitively
    (the libraries of the libraries)."""
    seen, todo = set(), [lib for rel in rels for lib in sourced(rel)]
    while todo:
        lib = todo.pop()
        if lib not in seen:
            seen.add(lib)
            todo.extend(sourced(lib))
    return sorted(seen)


def seed(scripts_dir, *rels):
    """Copy into `scripts_dir` every kit library the entries scripts/<rels>
    source, transitively; returns the libraries (relative to scripts/)."""
    libs = closure(*rels)
    for lib in libs:
        dst = pathlib.Path(scripts_dir) / lib
        dst.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy(SCRIPTS / lib, dst)
    return libs


def library(scripts_dir, *libs):
    """Copy into `scripts_dir` the kit libraries scripts/<libs> themselves and
    every library they source, transitively -- for a tree whose Python side
    sources a library (`executor.test_features`, `gate_plan`, the tree sync);
    returns the copied files (relative to scripts/)."""
    out = sorted(set(libs) | set(closure(*libs)))
    for lib in out:
        dst = pathlib.Path(scripts_dir) / lib
        dst.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy(SCRIPTS / lib, dst)
    return out


# Where lane_sync.sh's functions end and its dispatch begins.
LANE_SYNC_DISPATCH = '\n[ $# -ge 1 ] || die "expected '


def functions(dst_dir, rel="lane_sync.sh", cut=LANE_SYNC_DISPATCH):
    """scripts/<rel> up to its dispatch, written to `dst_dir`/<rel> beside the
    kit libraries it sources -- a file to `source "$0"`, so its own location
    (BASH_SOURCE) finds them; a `bash -c "<text>"` has no location and stops
    at the fail-closed source. Returns the path."""
    text = (SCRIPTS / rel).read_text(encoding="utf-8")
    dst = pathlib.Path(dst_dir) / rel
    dst.parent.mkdir(parents=True, exist_ok=True)
    dst.write_text(text[:text.index(cut)] + "\n", encoding="utf-8")
    seed(dst_dir, rel)
    return str(dst)
