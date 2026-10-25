#!/usr/bin/env python3
"""meclaw -- the unit runner: one `unittest` process per file, dir or module, in a pool.

WHY THIS EXISTS
===============
The satellites run their Python tests one file per process, serially:
soul2me's `unit` station spends 1 159-1 647 s on 201 files, orga-hive's 758 s.
Each file is independent, so the time is a pool away from a fraction of it.
A satellite station with `parallel.by` in file|dir|module calls this runner
through the core (W10, `pipeline/stations.json`):

    {python} {pipeline}/unit_runner.py --root {root} --by file --workers {workers}
        --include 'tests/test_*.py' --exclude 'tests/lab/**' --slots {slots}
        [--run unittest|script] [--top root|unit] [--slot-env NAME] [--python EXE]
        [--timeout-s N] [--jsonl FILE --run-id ID --station NAME] [--list]

`--include` and `--exclude` repeat. `--list` prints the units (`UNIT <unit>`
each, then `UNIT-LIST <n>`) and runs nothing; exit 1 when none matched.

UNITS
=====
`--include` / `--exclude` are fnmatch patterns over paths relative to
`--root` (`*` crosses `/`, as in the station register). by=file and
by=module: one unit per matching file, run as `<python> -m unittest
<dotted.module>` from the root. by=dir: one unit per directory that holds a
matching file, run as `<python> -m unittest <its matching modules>` -- the
directory's files, never a recursive `discover`, so an excluded or nested
directory is not run twice.

`--run script` (by=file|module): each file runs as `<python> <file>` from the
root, as soul2me's old gate did -- the tree mixes unittest modules and case
scripts; the file's exit code is the verdict. `--top unit` (by=dir): the
units are the DIRECTORIES that match an include (not descended into), each
run as `<python> -m unittest discover -s <dir> -t <dir>` -- a dir without
`__init__.py` whose modules import their neighbours (`import _setup`).

Every unit runs with the root at the head of PYTHONPATH, so a script finds
`tests.support...` like a module run from the root does.

SLOTS
=====
Every worker holds exactly ONE slot for its whole life: worker i gets
`slots[i]` and passes it to each unit it runs as `MECLAW_SLOT`, its index as
`MECLAW_WORKER`, and with `--slot-env NAME` also as `NAME` (soul2me's
`SOUL2ME_LAB_SLOT`). Fewer slots than workers is a usage error (exit 2),
never two workers on one slot. Without `--slots` the slots are `0..k-1`.

OUTPUT
======
After the pool is done, sorted by unit -- the same order in every run:

    UNIT <unit> <secs>s <GREEN|RED> tests=<n|n/a> fail=<n> [-- <reason>]
    UNIT-SUMMARY <green>/<total> <secs>s <GREEN|RED>

The tail of each RED unit's output goes to stderr. A unit that runs longer
than `--timeout-s` is killed with its process group and is RED with the
reason `timeout after <n>s`. `tests` comes from the unit's unittest or
pytest summary line; without one it is `n/a` (null in the jsonl), never 0.
Exit 5 ("no tests ran") is GREEN only without failures and errors -- a
failed set-up exits 5 too (GH #1313). No unit at all is RED. Exit 0 GREEN, 1 RED,
2 usage. `--jsonl FILE` writes one `test_result` stream line per unit
(`scripts/pipeline/events.py`: `{at, run, kind: note, event: test_result,
text, data: {run, station, part, verdict, secs, tests, fail[, reason]}}`).

Standard library only: the runner travels in the kit to a lane
(`/srv/target/kit/<sha8>/scripts/pipeline/`) and runs there as a script.
"""

import argparse
import concurrent.futures
import datetime
import fnmatch
import json
import os
import queue
import re
import signal
import subprocess
import sys
import threading
import time

try:  # as part of the package (tests, `pipeline.py`) ...
    from . import events as _events
    from . import hang_dump
except ImportError:  # ... or as a script from the kit
    sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
    from pipeline import events as _events  # noqa: E402
    from pipeline import hang_dump  # noqa: E402

BY = ("file", "dir", "module")
RUN = ("unittest", "script")
TOP = ("root", "unit")
SKIP_DIRS = frozenset({".git", "__pycache__", "node_modules", ".venv", "venv", "target",
                       "build", ".mypy_cache", ".pytest_cache"})
TAIL_LINES = 30
FAILURE_LINE = re.compile(r"^(FAIL:|ERROR:|not ok\b|FAILED\b|Traceback )")


def keep_failures(lines, n):
    """The last `n` lines of a RED unit plus every failure line above them
    (`FAIL:`/`ERROR:`/`not ok`/`FAILED`/`Traceback`, W10 kit finding K2)."""
    if len(lines) <= n:
        return list(lines)
    head = [l for l in lines[:-n] if FAILURE_LINE.match(l.lstrip())]
    if len(head) > FAILURE_HEAD_MAX:  # review Zb R2 M4: the head is capped too
        more = len(head) - FAILURE_HEAD_MAX
        head = head[:FAILURE_HEAD_MAX] + ["... %d more failure lines in the unit log" % more]
    return head + list(lines[-n:])


FAILURE_HEAD_MAX = 200
UNIT_LOG_MAX_B = 8 << 20  # the last 8 MiB of a unit's output


def _write_unit_logs(dirpath, results):
    try:
        os.makedirs(dirpath, exist_ok=True)
        taken = set()
        for res in results:
            name = re.sub(r"[^A-Za-z0-9_.-]+", "_", str(res["unit"])).strip("._") or "unit"
            stem, i = name, 1
            while name in taken:  # `a/b` and `a_b` sanitize alike (R2 M4)
                i += 1
                name = "%s-%d" % (stem, i)
            taken.add(name)
            text = res.get("output") or ""
            if len(text) > UNIT_LOG_MAX_B:
                text = "... %d characters cut\n" % (len(text) - UNIT_LOG_MAX_B) + text[-UNIT_LOG_MAX_B:]
            with open(os.path.join(dirpath, name + ".log"), "w", encoding="utf-8") as fh:
                fh.write(text)
    except OSError as exc:
        print("unit_runner: unit logs not written to %s (%s)" % (dirpath, exc), file=sys.stderr)
DEFAULT_TIMEOUT_S = 1800

_RAN = re.compile(r"^Ran (\d+) tests? in ", re.MULTILINE)
_FAILED = re.compile(r"^FAILED \(([^)]*)\)", re.MULTILINE)
# pytest's closing line, decorated (`=== 4 passed, 1 failed in 0.31s ===`) or
# quiet (`4 passed in 0.31s`): the script mode's second readable count (#1313).
_PYTEST = re.compile(r"^=*\s*(\d+ [a-z]+(?:, \d+ [a-z]+)*) in [\d.]+s\b", re.MULTILINE)
_PYTEST_RAN = ("passed", "failed", "error", "errors", "skipped", "xfailed", "xpassed")
_PYTEST_FAIL = ("failed", "error", "errors")


# --- the units ------------------------------------------------------------------

def _matches(rel, patterns):
    return any(fnmatch.fnmatch(rel, p) for p in patterns)


def _walk_roots(patterns):
    """The literal directory prefix of each include pattern -- where to walk."""
    out = set()
    for p in patterns:
        parts = []
        for part in p.split("/")[:-1]:
            if any(c in part for c in "*?["):
                break
            parts.append(part)
        out.add("/".join(parts))
    # a prefix inside another one is walked with it
    return sorted(r for r in out if not any(o != r and (o == "" or r.startswith(o + "/"))
                                            for o in out))


def find_files(root, include, exclude=()):
    """Sorted relative paths under `root` that match `include` and not `exclude`."""
    found = set()
    for top in _walk_roots(include):
        start = os.path.join(root, top) if top else root
        for cur, dirs, files in os.walk(start):
            dirs[:] = sorted(d for d in dirs if d not in SKIP_DIRS)
            for f in files:
                rel = os.path.relpath(os.path.join(cur, f), root).replace(os.sep, "/")
                if _matches(rel, include) and not _matches(rel, exclude):
                    found.add(rel)
    return sorted(found)


def module_of(rel):
    return rel[:-3].replace("/", ".") if rel.endswith(".py") else rel.replace("/", ".")


def find_dirs(root, include, exclude=()):
    """Sorted relative directories under `root` that match `include` and not
    `exclude`; a matching directory is not descended into."""
    found = set()
    for top in _walk_roots(include):
        start = os.path.join(root, top) if top else root
        for cur, dirs, _files in os.walk(start):
            keep = []
            for d in sorted(dirs):
                if d in SKIP_DIRS:
                    continue
                rel = os.path.relpath(os.path.join(cur, d), root).replace(os.sep, "/")
                if _matches(rel, include):
                    if not _matches(rel, exclude):
                        found.add(rel)
                    continue
                keep.append(d)
            dirs[:] = keep
    return sorted(found)


def units(root, by, include, exclude=(), top="root"):
    """`[(unit, [modules])]`, sorted by unit (by=dir with top=unit: `[(dir, [])]`)."""
    if by == "dir" and top == "unit":
        return [(d, []) for d in find_dirs(root, include, exclude)]
    files = find_files(root, include, exclude)
    if by in ("file", "module"):
        return [(rel, [module_of(rel)]) for rel in files]
    groups = {}
    for rel in files:
        groups.setdefault(os.path.dirname(rel) or ".", []).append(module_of(rel))
    return sorted((d, mods) for d, mods in groups.items())


# --- one unit -------------------------------------------------------------------

def counts(text):
    """`(tests, fail)` from unittest's closing lines, else from pytest's;
    `tests` is None when the output carries neither (GH #1313: soul2me's case
    scripts ran 85-870 s and reported `tests=0` -- `0` claimed that nothing
    ran, the runner only could not read a count). `tests == 0` is only ever
    a summary that says so."""
    ran = _RAN.findall(text)
    if ran:
        fail = 0
        failed = _FAILED.findall(text)
        if failed:
            for item in failed[-1].split(","):
                key, _, value = item.strip().partition("=")
                if key in ("failures", "errors", "unexpected successes") and value.isdigit():
                    fail += int(value)
        return int(ran[-1]), fail
    pytest = _PYTEST.findall(text)
    if pytest:
        tests = fail = 0
        seen = False  # review KX I1: `12 cases in 85.1s` has the form, not the keys
        for item in pytest[-1].split(","):
            value, _, key = item.strip().partition(" ")
            if key in _PYTEST_RAN:
                tests += int(value)
                seen = True
            if key in _PYTEST_FAIL:
                fail += int(value)
        if seen:
            return tests, fail
    return None, 0


def _last_line(text):
    lines = [l for l in text.strip().splitlines() if l.strip()]
    return lines[-1].strip() if lines else ""


def unit_argv(unit, modules, root, python, run="unittest", top="root"):
    """The command of one unit (run from `root`)."""
    if run == "script":
        return [python, unit]
    if top == "unit" and not modules:
        where = os.path.join(root, unit)
        return hang_dump.argv(python, "unittest") + ["discover", "-s", where, "-t", where]
    # Through the hang dump: a unit stuck past the cap shows its stacks (TV-Z5).
    return hang_dump.argv(python, "unittest") + list(modules)


def run_unit(unit, modules, root, python, env, timeout_s, run="unittest", top="root"):
    """Run one unit; `{unit, verdict, secs, tests, fail, reason, output}`."""
    argv = unit_argv(unit, modules, root, python, run, top)
    t0 = time.monotonic()
    try:
        proc = subprocess.Popen(argv, cwd=root, env=env, stdout=subprocess.PIPE,
                                stderr=subprocess.STDOUT, text=True, start_new_session=True)
    except OSError as e:
        reason = "cannot start %s: %s" % (argv[0], e.strerror or e)
        return {"unit": unit, "verdict": "RED", "secs": round(time.monotonic() - t0, 3),
                "tests": 0, "fail": 1, "reason": reason, "output": reason}
    timed_out = False
    try:
        output, _ = proc.communicate(timeout=timeout_s)
    except subprocess.TimeoutExpired:
        timed_out = True
        try:
            os.killpg(proc.pid, signal.SIGKILL)
        except (ProcessLookupError, PermissionError):
            proc.kill()
        output, _ = proc.communicate()
    secs = round(time.monotonic() - t0, 3)
    output = output or ""
    tests, fail = counts(output)
    if timed_out:
        verdict, reason = "RED", "timeout after %ds" % timeout_s
    elif proc.returncode == 0:
        verdict, reason = "GREEN", ""
    elif run == "unittest" and proc.returncode == 5 and tests == 0 and fail == 0:
        # Python 3.12: a module without tests exits 5 ("NO TESTS RAN"). Only
        # without failures and errors: a failed setUpClass exits 5 as well,
        # with `FAILED (errors=1)` -- that unit was GREEN (GH #1313). And only
        # beside a summary that says 0: exit 5 without one is no statement.
        verdict, reason = "GREEN", ""
    elif run == "unittest" and proc.returncode == 5:
        verdict = "RED"
        if fail:
            reason = "set-up failed, no test ran"
        else:
            reason, fail = "exit 5 without a readable test summary", 1
    else:
        verdict = "RED"
        reason = _last_line(output) or "exit %d" % proc.returncode
        if fail == 0:
            fail = 1
    return {"unit": unit, "verdict": verdict, "secs": secs, "tests": tests, "fail": fail,
            "reason": reason, "output": output}


# --- the pool -------------------------------------------------------------------

def unit_env(root, base_env=None):
    """`base_env` (default: ours) with `root` at the head of PYTHONPATH and
    `MECLAW_UNIT_RUNNER` naming this runner: a satellite's negative test asks
    it for its own unit list (`--list`, GH #1147), and on a lane the runner
    lives in the kit, not in a meclaw tree the test could find."""
    env = dict(base_env if base_env is not None else os.environ)
    rest = [p for p in (env.get("PYTHONPATH") or "").split(os.pathsep) if p and p != root]
    env["PYTHONPATH"] = os.pathsep.join([root] + rest)
    env["MECLAW_UNIT_RUNNER"] = os.path.realpath(os.path.abspath(__file__))
    return env


def run_pool(todo, root, python, workers, slots, slot_env=None, timeout_s=DEFAULT_TIMEOUT_S,
             base_env=None, run="unittest", top="root"):
    """Run `todo` = `[(unit, modules)]` on `workers` threads; results sorted by unit.

    Each pool thread takes one `(index, slot)` when it starts and keeps it:
    worker i always runs with `slots[i]`."""
    free = queue.Queue()
    for i in range(workers):
        free.put((i, slots[i]))
    local = threading.local()

    def claim():
        local.worker, local.slot = free.get_nowait()

    def job(unit, modules):
        env = unit_env(root, base_env)
        env["MECLAW_SLOT"] = str(local.slot)
        env["MECLAW_WORKER"] = str(local.worker)
        if slot_env:
            env[slot_env] = str(local.slot)
        res = run_unit(unit, modules, root, python, env, timeout_s, run, top)
        res["worker"], res["slot"] = local.worker, local.slot
        return res

    with concurrent.futures.ThreadPoolExecutor(max_workers=workers, initializer=claim) as pool:
        futures = [pool.submit(job, unit, modules) for unit, modules in todo]
        results = [f.result() for f in futures]
    return sorted(results, key=lambda r: r["unit"])


def line(res):
    tests = "n/a" if res["tests"] is None else "%d" % res["tests"]
    text = "UNIT %s %.1fs %s tests=%s fail=%d" % (res["unit"], res["secs"], res["verdict"],
                                                 tests, res["fail"])
    return "%s -- %s" % (text, res["reason"]) if res["verdict"] == "RED" and res["reason"] else text


def summary(results, secs, run="unittest"):
    """The closing line and the verdict. A unittest run where no test ran at all
    is RED (GH #997, re-check W10 I2): a station glob that only hits test-less
    modules would otherwise report a GREEN gate without one test. A single
    test-less unit stays GREEN. Only counted units decide it: a unit without a
    readable count (`tests=n/a`, mostly script mode) is not "no test ran", and
    its exit code rules (GH #1313)."""
    green = sum(1 for r in results if r["verdict"] == "GREEN")
    verdict = "GREEN" if results and green == len(results) else "RED"
    text = "UNIT-SUMMARY %d/%d %.1fs %s" % (green, len(results), secs, verdict)
    counted = [r["tests"] for r in results]
    if verdict == "GREEN" and None not in counted and sum(counted) == 0:
        verdict = "RED"
        text = "UNIT-SUMMARY %d/%d %.1fs RED -- no test ran in %d units" % (
            green, len(results), secs, len(results))
    return text, verdict


def _now():
    return datetime.datetime.now(datetime.timezone.utc).isoformat(
        timespec="milliseconds").replace("+00:00", "Z")


def test_result_rows(results, run, station):
    """One `test_result` stream line per unit, validated against events.py."""
    rows = []
    for res in results:
        data = {"run": run, "station": station, "part": res["unit"], "verdict": res["verdict"],
                "secs": float(res["secs"]), "tests": res["tests"], "fail": res["fail"]}
        if res["reason"]:
            data["reason"] = res["reason"]
        _events.validate("test_result", "note", data)
        rows.append({"at": _now(), "run": run, "kind": "note", "event": "test_result",
                     "text": _events.summary_text("test_result", data), "data": data})
    return rows


# --- command line ---------------------------------------------------------------

def _parser():
    ap = argparse.ArgumentParser(prog="unit_runner", description=__doc__.splitlines()[0])
    ap.add_argument("--root", required=True)
    ap.add_argument("--by", choices=BY, default="file")
    ap.add_argument("--run", choices=RUN, default="unittest",
                    help="script: each file as `<python> <file>`, its exit code the verdict")
    ap.add_argument("--top", choices=TOP, default="root",
                    help="unit (by=dir): discover each matching dir with itself as top level")
    ap.add_argument("--workers", type=int, default=1)
    ap.add_argument("--include", action="append", default=[])
    ap.add_argument("--exclude", action="append", default=[])
    ap.add_argument("--slots", default="", help="comma-separated, one per worker")
    ap.add_argument("--slot-env", default=None, help="also pass the slot as this variable")
    ap.add_argument("--python", default=sys.executable)
    ap.add_argument("--timeout-s", type=int, default=DEFAULT_TIMEOUT_S)
    ap.add_argument("--jsonl", default=None, help="write one test_result line per unit")
    ap.add_argument("--run-id", default="local", help="the run id of the jsonl lines")
    ap.add_argument("--station", default="unit", help="the station of the jsonl lines")
    ap.add_argument("--list", action="store_true", help="print the units, run nothing")
    return ap


def main(argv=None):
    args = _parser().parse_args(argv)
    root = os.path.abspath(args.root)
    if args.workers < 1:
        print("unit_runner: --workers must be >= 1", file=sys.stderr)
        return 2
    if not args.include:
        print("unit_runner: at least one --include", file=sys.stderr)
        return 2
    if args.run == "script" and args.by == "dir":
        print("unit_runner: --run script runs files (--by file|module), not dirs", file=sys.stderr)
        return 2
    if args.top == "unit" and args.by != "dir":
        print("unit_runner: --top unit is a form of --by dir", file=sys.stderr)
        return 2
    if args.list:
        todo = units(root, args.by, args.include, args.exclude, args.top)
        for unit, _mods in todo:
            print("UNIT %s" % unit)
        print("UNIT-LIST %d" % len(todo), flush=True)
        return 0 if todo else 1
    slots = [s.strip() for s in args.slots.split(",") if s.strip()] if args.slots else \
        [str(i) for i in range(args.workers)]
    if len(slots) < args.workers:
        print("unit_runner: %d slots for %d workers -- every worker needs its own slot"
              % (len(slots), args.workers), file=sys.stderr)
        return 2
    if len(set(slots[:args.workers])) < args.workers:
        print("unit_runner: the slots of the workers must be distinct", file=sys.stderr)
        return 2
    t0 = time.monotonic()
    todo = units(root, args.by, args.include, args.exclude, args.top)
    results = run_pool(todo, root, args.python, min(args.workers, max(1, len(todo))),
                       slots, args.slot_env, args.timeout_s, run=args.run,
                       top=args.top) if todo else []
    secs = time.monotonic() - t0
    for res in results:
        print(line(res))
    text, verdict = summary(results, secs, args.run)
    if not results:
        text += " -- no unit matched %s" % " ".join(args.include)
    print(text, flush=True)
    for res in results:
        if res["verdict"] == "RED":
            tail = keep_failures(res["output"].rstrip().splitlines(), TAIL_LINES)
            print("--- %s (worker %s, slot %s)\n%s" % (res["unit"], res["worker"], res["slot"],
                                                      "\n".join(tail)), file=sys.stderr)
    if args.jsonl:
        # K2: the full output of every unit stays on the lane next to the
        # jsonl (`<jsonl>.d/<unit>.log`) -- the cut above is for the stream.
        _write_unit_logs(args.jsonl + ".d", results)
    if args.jsonl:
        with open(args.jsonl, "w", encoding="utf-8") as fh:
            for row in test_result_rows(results, args.run_id, args.station):
                fh.write(json.dumps(row, sort_keys=True) + "\n")
    return 0 if verdict == "GREEN" else 1


if __name__ == "__main__":
    sys.exit(main())
