"""meclaw warm/resident runner harness.

Boots once, compiles the cell's script once, then answers exactly one framed
line per request line. It is not a sandbox, not a scheduler and not a protocol:
the only thing it adds over `python3 -c <script>` is that the interpreter and
the compiled code object survive between messages.

Wire (line-JSON, both directions):
  line 1  {"script": "..."} | {"script_path": "..."}  plus "persistent": bool
  line n  the stdin document the body reads
  answer  {"exit_code": int, "stdout": str, "stderr": str}
"""
import io
import json
import sys
import traceback

REAL_STDIN = sys.stdin
REAL_STDOUT = sys.stdout
REAL_STDERR = sys.stderr


def _load(cfg):
    """Compile once. Returns (code, globals_seed, boot_error)."""
    seed = {}
    src = cfg.get("script")
    filename = "<string>"
    if src is None:
        import os
        path = cfg["script_path"]
        with open(path, "r", encoding="utf-8") as fh:
            src = fh.read()
        # `python3 <path>` puts the script's own directory first on sys.path and
        # defines __file__; the harness itself runs as `-c`, so a path script's
        # search order is restored here rather than silently changed.
        filename = path
        seed["__file__"] = path
        sys.path.insert(0, os.path.dirname(os.path.abspath(path)))
    try:
        return compile(src, filename, "exec"), seed, None
    except BaseException:
        return None, seed, traceback.format_exc()


def _run(code, glb, document):
    """Execute the body once against `document` and frame what it wrote."""
    out, err = io.StringIO(), io.StringIO()
    exit_code = 0
    sys.stdin, sys.stdout, sys.stderr = io.StringIO(document), out, err
    try:
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
    return {"exit_code": exit_code, "stdout": out.getvalue(), "stderr": err.getvalue()}


def main():
    boot = REAL_STDIN.readline()
    if not boot:
        return
    cfg = json.loads(boot)
    persistent = bool(cfg.get("persistent"))
    code, seed, boot_error = _load(cfg)
    # The resident namespace. Untouched in warm mode, where every message gets a
    # fresh dict built from the same seed -- which is what makes accumulation
    # impossible rather than merely discouraged.
    resident = dict(seed)
    resident["__name__"] = "__main__"
    for line in REAL_STDIN:
        if not line.strip():
            continue
        if boot_error is not None:
            frame = {"exit_code": 1, "stdout": "", "stderr": boot_error}
        elif persistent:
            frame = _run(code, resident, line)
        else:
            glb = dict(seed)
            glb["__name__"] = "__main__"
            frame = _run(code, glb, line)
        REAL_STDOUT.write(json.dumps(frame) + "\n")
        REAL_STDOUT.flush()


main()
