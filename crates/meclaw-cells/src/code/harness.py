"""meclaw warm/resident runner harness.

Boots once, compiles the cell's script once, then answers exactly one framed
line per request line. It is not a sandbox, not a scheduler and not a protocol:
the only thing it adds over `python3 -c <script>` is that the interpreter and
the compiled code object survive between messages.

Wire (line-JSON, both directions):
  line 1  {"script": "..."} | {"script_path": "..."}  plus "persistent": bool
  line n  the stdin document the body reads, or a job frame
          {"meclaw_job":1,"env":{NAME: value},"document": "<the document>"}
          whose env entries exist for exactly this one job (GH #1060)
  answer  {"exit_code": int, "stdout": str, "stderr": str}
"""
import sys

# GH #1026: a script may restrict its own imports -- a finder on sys.meta_path
# that refuses a name, or a sys.path it cleans before its first import (a gate
# script that drops '' so a stray json.py in the working directory cannot
# win). Both only see an import that reaches the import system. This harness
# needs io/json/traceback for itself, which pulls in 21 modules a cold
# `python3 -c <script>` does not have at its start (json, re, enum, tokenize,
# ...; measured with python 3.12.3), and `-c` puts '' first on sys.path while
# it does. Left alone, every such module sat in sys.modules, an `import json` in
# the script was answered from that cache, and the script's restriction never
# ran: cold denied, warm allowed. So the harness (1) imports its own modules
# without '' on the path, (2) remembers which modules a cold run starts with
# and drops everything else from sys.modules before the script's first run,
# keeping its own references, and (3) hands the script the sys.path a cold run
# would have. The script then starts where `python3 -c <script>` (or `python3
# <path>`) starts.
COLD_MODULES = frozenset(sys.modules)
COLD_PATH = list(sys.path)
if sys.path and sys.path[0] == "":
    del sys.path[0]

import builtins  # noqa: E402  (loaded at every start; never purged)
import io  # noqa: E402  (after the path guard on purpose, see above)
import json  # noqa: E402
import os  # noqa: E402  (in sys.modules at every start, like builtins; GH #1060 sets env per job)
import collections.abc  # noqa: E402  (traceback brings it; named here for _private_collections)

# GH #1026, re-review of Z-M2: `traceback` imports more modules while it
# formats, from inside its functions. Python 3.12 `traceback.py` runs
# `import ast` for the caret anchors of every frame whose source line it can
# read (line 590; a raise inside `json.loads` is enough) and `import
# unicodedata` for a non-ASCII line (line 647); `ast` brings `_ast` and
# `contextlib`. 3.13/3.14 add `import _suggestions` and (3.14) `import difflib`,
# and `linecache` there imports `tokenize` inside `updatecache`. Each such
# import went through the import system after the purge: the module landed in
# the script's `sys.modules` (cold `[]`, warm `['_ast', 'ast', 'contextlib']`
# after one raise) and the script's own finder on `sys.meta_path` was asked
# for it. An `import` statement calls `__import__` from the builtins its
# function was created with (CPython keeps them per function, taken from the
# module's `__builtins__` at definition time), so the harness executes its own
# `linecache` and `traceback` with a builtins dict whose `__import__` hands out
# these modules from references loaded here, before the purge -- the import
# system is never asked, and `sys.modules` is never touched after the purge.
# Every other name goes to the real `__import__`.
_LAZY_FOR_TRACEBACK = ("ast", "unicodedata", "tokenize", "_suggestions", "difflib")
REAL_IMPORT = builtins.__import__
SERVED = {}


def _served_import(name, globals=None, locals=None, fromlist=(), level=0):
    if level == 0:
        module = SERVED.get(name)
        if module is not None:
            return module
    return REAL_IMPORT(name, globals, locals, fromlist, level)


SERVED_BUILTINS = dict(vars(builtins))
SERVED_BUILTINS["__import__"] = _served_import


def _load_served(name):
    """Execute module `name` afresh with `SERVED_BUILTINS` (GH #1026).

    Setting `__builtins__` on an already imported module changes nothing for
    its functions (measured with python 3.12.3: a function defined before the
    swap still calls the real `__import__`), so the module is built from its
    spec here. It sits in `sys.modules` only while it executes, as a normal
    import would have it, and whatever was there before comes back.
    """
    import importlib.util

    spec = importlib.util.find_spec(name)
    module = importlib.util.module_from_spec(spec)
    module.__builtins__ = SERVED_BUILTINS
    previous = sys.modules.get(name)
    sys.modules[name] = module
    try:
        spec.loader.exec_module(module)
    finally:
        if previous is None:
            sys.modules.pop(name, None)
        else:
            sys.modules[name] = previous
    return module


for _name in _LAZY_FOR_TRACEBACK:
    try:
        SERVED[_name] = __import__(_name)
    except ImportError:  # _suggestions exists from python 3.13 on
        pass
SERVED["linecache"] = _load_served("linecache")
traceback = _load_served("traceback")

REAL_STDIN = sys.stdin
REAL_STDOUT = sys.stdout
REAL_STDERR = sys.stderr
JOB_MARK = '{"meclaw_job":'


def _private_collections():
    """Give the harness's `traceback` a `collections` of its own (GH #1026).

    `traceback` reads `collections.abc.Sequence` for every exception it
    formats (the `isinstance(self.__notes__, ...)` check in
    `format_exception_only`, python 3.12 `traceback.py` line 898, whether the
    exception has notes or not), `collections.deque` for a negative limit and
    `collections.namedtuple` at import -- the only three uses in CPython 3.11,
    3.12, 3.13 and 3.14 `Lib/traceback.py`. The purge in `_cold_start` unbinds
    `abc` from the shared `collections`, so a script never reaches it without
    an import. Without `abc` the child died on the first script that raised;
    binding it back on the shared package around the harness's own traceback
    output opened a window a thread of the script saw (re-review of Z-M2: 2496
    hits in 200 raises). This module carries the three names itself and asks the real
    `collections` for anything a later python may read, so the shared package
    is never touched again after the purge.
    """
    real = vars(traceback).get("collections")
    if real is None:
        return
    private = type(sys)(real.__name__)
    private.abc = collections.abc
    private.deque = real.deque
    private.namedtuple = real.namedtuple

    def __getattr__(name):
        return getattr(real, name)

    private.__getattr__ = __getattr__
    vars(traceback)["collections"] = private


_private_collections()


def _load(cfg):
    """Compile once. Returns (code, globals_seed, boot_error)."""
    seed = {}
    src = cfg.get("script")
    filename = "<string>"
    if src is None:
        path = cfg["script_path"]
        with open(path, "r", encoding="utf-8") as fh:
            src = fh.read()
        filename = path
        seed["__file__"] = path
    try:
        return compile(src, filename, "exec"), seed, None
    except BaseException:
        return None, seed, traceback.format_exc()


def _cold_start(cfg):
    """Make the process look to the script as a cold run's would (GH #1026).

    sys.path: `-c` starts with '' first, `python3 <path>` with the script's own
    directory (symlinks resolved) in that place instead; with '' absent (`-I`,
    PYTHONSAFEPATH) neither adds anything. sys.modules: everything the harness
    imported for itself leaves; its module objects stay alive through the
    harness's own globals, so framing keeps working whatever the script later
    imports under the same names. A dropped submodule is also unbound from a
    parent package that stays, so it is not reachable as an attribute either.
    """
    path = list(COLD_PATH)
    if cfg.get("script") is None and path and path[0] == "":
        import os
        path[0] = os.path.dirname(os.path.realpath(cfg["script_path"]))
    sys.path[:] = path
    purged = {}
    for name in list(sys.modules):
        if name not in COLD_MODULES:
            purged[name] = sys.modules.pop(name)
    # Importing `a.b` also binds `b` on `a`. Where the parent stays loaded, that
    # binding outlives the purge: with python 3.12.3 `collections` is there at a
    # cold start, `collections.abc` only comes with `traceback`, so
    # `hasattr(collections, "abc")` was False cold and True warm, and a script
    # reached the submodule without an import its filter could see. Unbind it,
    # but only where the attribute is still the very module object we dropped.
    for name, module in purged.items():
        parent, _, child = name.rpartition(".")
        if not parent or parent in purged:
            continue
        owner = sys.modules.get(parent)
        if owner is not None and vars(owner).get(child) is module:
            delattr(owner, child)


def _job(line):
    """(document, env) of one request line. A job frame carries the credential
    of THIS job as an environment entry (GH #1060); every other line is the
    document itself. Nothing of the frame is ever printed: a frame that does not
    parse becomes an empty document, and the body says what it makes of that."""
    if not line.startswith(JOB_MARK):
        return line, {}
    try:
        frame = json.loads(line)
        env = frame.get("env") or {}
        # The plain line ends in its newline; so does the framed document.
        document = str(frame.get("document", "")) + "\n"
        return document, {str(k): str(v) for k, v in env.items()}
    except (ValueError, AttributeError):
        return "", {}


def _run(code, glb, document, env=None):
    """Execute the body once against `document` and frame what it wrote.

    `env` is set for the body and removed (or restored) after it, in the
    `finally`: the child serves job after job, and the next job -- one without
    a credential included -- must not inherit what this one was given."""
    out, err = io.StringIO(), io.StringIO()
    exit_code = 0
    saved = {}
    sys.stdin, sys.stdout, sys.stderr = io.StringIO(document), out, err
    try:
        for name, value in (env or {}).items():
            saved[name] = os.environ.get(name)
            os.environ[name] = value
        exec(code, glb)
    except SystemExit as exc:
        if exc.code is None:
            exit_code = 0
        elif isinstance(exc.code, int):
            exit_code = exc.code
        else:
            exit_code = 1
            print(exc.code, file=err)
    except BaseException:
        exit_code = 1
        traceback.print_exc(file=err)
    finally:
        sys.stdin, sys.stdout, sys.stderr = REAL_STDIN, REAL_STDOUT, REAL_STDERR
        for name, old in saved.items():
            if old is None:
                os.environ.pop(name, None)
            else:
                os.environ[name] = old
    return {"exit_code": exit_code, "stdout": out.getvalue(), "stderr": err.getvalue()}


def main():
    boot = REAL_STDIN.readline()
    if not boot:
        return
    cfg = json.loads(boot)
    persistent = bool(cfg.get("persistent"))
    code, seed, boot_error = _load(cfg)
    # Last thing before the first message: from here on the process is the one
    # a cold run of the script would start in (GH #1026, see the top).
    _cold_start(cfg)
    # The resident namespace. Untouched in warm mode, where every message gets a
    # fresh dict built from the same seed -- which is what makes accumulation
    # impossible rather than merely discouraged.
    resident = dict(seed)
    resident["__name__"] = "__main__"
    for line in REAL_STDIN:
        if not line.strip():
            continue
        document, env = _job(line)
        if boot_error is not None:
            frame = {"exit_code": 1, "stdout": "", "stderr": boot_error}
        elif persistent:
            frame = _run(code, resident, document, env)
        else:
            glb = dict(seed)
            glb["__name__"] = "__main__"
            frame = _run(code, glb, document, env)
        REAL_STDOUT.write(json.dumps(frame) + "\n")
        REAL_STDOUT.flush()


main()
