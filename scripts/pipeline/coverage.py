"""`pipeline coverage` -- the coverage map: which test part executes which file (W6, GH #1120).

WHY THIS EXISTS
===============
On crate level a change in a lib crate plans every binary that depends on it
(`rdeps`). The map makes the strand gate file-level without a model: a nightly
profile run (`cargo llvm-cov nextest --no-report`) on a lane, one profile per
test (`coverage_runner.sh` names it after NEXTEST_BINARY_ID/NEXTEST_TEST_NAME),
merged per test PART -- a module of a harness or lib binary, else the whole
binary, the unit the cache runs (V0 section 2 `cache`) -- and exported per part
to the source files it executed (`llvm-cov export -format=lcov -summary-only`).

    {"schema": "meclaw.pipeline.coverage/1", "rev", "built_at", "runner",
     "granularity": "module"|"binary", "secs", "tests": <profiles>,
     "parts": [{"binary", "module"|null, "files": {path: lines_hit}}]}

The map counts red tests too (their path ran), but red tests make its verdict
RED (welle-tb S1 C-Beleg, KR-TB-1 a): the build on the end tip of a wave is the
full suite of that wave. `aggregate --nextest-log` reads the names of the red
tests from the nextest log; `build` writes `process_finding coverage_red` with
them and exits `RED_EXIT`; `wave close` refuses while the end tip has no GREEN
map. A map is GREEN only when complete, RED on nextest exit 100 or a named red
test, else INCOMPLETE. Older than 7 days, it is stale:
`process_finding coverage_stale`, and `impact` falls back to `rdeps`. With no
lane up the run switches none on (rulings.md 2): `process_finding
coverage_skipped_no_lane`. A lane without llvm-tools is `TOOL_EXIT` and
`process_finding coverage_tool_missing`, the lane named.
"""

import argparse
import concurrent.futures
import datetime
import json
import os
import pathlib
import re
import shlex
import subprocess
import sys
import time

_SCRIPTS = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
if _SCRIPTS not in sys.path:
    sys.path.insert(0, _SCRIPTS)

from pipeline import browser_cover, cpus, spawn_closure  # noqa: E402

SCHEMA = "meclaw.pipeline.coverage/1"
STALE_DAYS = 7
# `build` exit for a map with red tests: never 0 (green), 1 (incomplete), 2 (broken
# map), 3 (lane not up) or 5 (refused lane) -- the night and the wave tell it apart.
RED_EXIT = 6
# `build` exit for a lane without the coverage tools (llvm-tools): the host part
# `coverage_host.sh` ends with it, `build` journals `process_finding
# coverage_tool_missing` with the lane and passes it on -- never a silent 0
# (V2-A2, welle-val2: build02 without llvm-tools read as a finished build).
TOOL_EXIT = 7
RED_NEXTEST_EXIT = 100  # nextest: the run finished, tests failed
VERDICTS = ("GREEN", "RED", "INCOMPLETE")
# nextest's final status line of a red test (with or without the `(n/m)` counter or a
# `[try]` slot): `FAIL [ 2.300s] (2/4) <binary-id> <test>`; `TRY n FAIL` lines of a
# retried test are not matched (a later pass makes it FLAKY, a last fail repeats as FAIL)
_RED_LINE = re.compile(r"^\s*(?:FAIL|TIMEOUT|SIGSEGV|SIGABRT|SIGKILL|SIGBUS|SIGILL|ABORT|LEAK-FAIL)"
                       r"\s+\[\s*[\d.]+s\]\s+(?:\[[^\]]*\]\s+)?(?:\(\s*\d+/\d+\)\s+)?"
                       r"(\S+)\s+(\S+)\s*$")
# a retried test's try (`TRY n FAIL|PASS ...`) and its FLAKY end line: a test
# whose LAST such line is a fail is red (retries from a flake override, S4)
_TRY_LINE = re.compile(r"^\s*(?:TRY\s+\d+\s+(PASS|FAIL|TIMEOUT|SIGSEGV|SIGABRT|SIGKILL|ABORT)"
                       r"|(FLAKY)\s+\d+/\d+)"
                       r"\s+\[\s*[\d.]+s\]\s+(?:\[[^\]]*\]\s+)?(?:\(\s*\d+/\d+\)\s+)?"
                       r"(\S+)\s+(\S+)\s*$")
DEADLINE_S = 7200
RUNNER = "scripts/pipeline/coverage_runner.sh"
LANE_SCRIPT = "scripts/pipeline/coverage_lane.sh"


# --- parsing ----------------------------------------------------------------

def parse_lcov(text, root=None):
    """`{repo-relative path: lines hit}` of an lcov export (LH, else DA > 0).

    Files outside `root` (the registry, the toolchain) are dropped; with no
    `root`, paths are kept as they stand.
    """
    out = {}
    cur, hit, da = None, None, 0
    prefix = (str(root).rstrip("/") + "/") if root else None
    for line in text.splitlines():
        if line.startswith("SF:"):
            cur, hit, da = line[3:], None, 0
        elif line.startswith("LH:") and cur is not None:
            hit = int(line[3:] or 0)
        elif line.startswith("DA:") and cur is not None:
            try:
                if int(line.split(",")[1]) > 0:
                    da += 1
            except (IndexError, ValueError):
                pass
        elif line == "end_of_record" and cur is not None:
            path = cur
            if prefix:
                if not path.startswith(prefix):
                    cur = None
                    continue
                path = path[len(prefix):]
            out[path] = hit if hit is not None else da
            cur = None
    return out


def granularity_of(binary, harness_bins):
    """`module` for lib unit-test binaries and harness binaries, `binary` for a flat test file."""
    return "module" if ("::" not in binary or binary in harness_bins) else "binary"


def profile_parts(base, harness_bins=()):
    """`{(binary, module|None): [profile files]}` under the runner's profile directory."""
    base = pathlib.Path(base)
    parts = {}
    if not base.is_dir():
        return parts
    for bdir in sorted(p for p in base.iterdir() if p.is_dir() and not p.name.startswith("_")):
        binary = bdir.name.replace("@", "/")  # the runner writes `/` of a bin id as `@`
        fine = granularity_of(binary, set(harness_bins)) == "module"
        for mdir in sorted(p for p in bdir.iterdir() if p.is_dir()):
            module = mdir.name if fine and mdir.name != "_binary" else None
            files = sorted(str(f) for f in mdir.rglob("*") if f.suffix in (".profdata", ".profraw"))
            if files:
                parts.setdefault((binary, module), []).extend(files)
    return parts


# --- aggregation (runs on the lane, next to the instrumented binaries) ------

def _export_part(key, files, objects, work, profdata, llvm_cov, root):
    binary, module = key
    obj = objects.get(binary)
    if not obj:
        # a profile without its binary is a part the map cannot measure: an
        # error, never a silently empty part (review C1)
        raise LookupError("no object for %s (not in `nextest list`)" % binary)
    tag = re.sub(r"[^A-Za-z0-9_.-]", "_", "%s__%s" % (binary, module or "_"))
    merged = os.path.join(work, tag + ".profdata")
    subprocess.run([profdata, "merge", "-sparse", "-o", merged] + files,
                   check=True, capture_output=True)
    extra = []
    for other in objects.get("_bins", []):
        extra += ["-object", other]
    out = subprocess.run([llvm_cov, "export", "-format=lcov", "-summary-only",
                          "-instr-profile", merged, obj] + extra,
                         check=True, capture_output=True, text=True)
    os.remove(merged)
    return key, parse_lcov(out.stdout, root)


def aggregate(base, objects, root, jobs=None, profdata="llvm-profdata", llvm_cov="llvm-cov",
              harness_bins=()):
    """Merge every part's profiles and export them: `(parts, errors)`.

    `parts` is the `parts` list of the map; `errors` names every part that
    could not be measured (no object, a failed merge or export). They are
    COUNTED, never dropped: a map with one error is not complete (review C1).
    """
    parts = profile_parts(base, harness_bins)
    work = os.path.join(base, "_merged")
    os.makedirs(work, exist_ok=True)
    out, errors = [], []
    with concurrent.futures.ThreadPoolExecutor(max_workers=jobs or cpus.threads()) as pool:
        futs = {pool.submit(_export_part, k, v, objects, work, profdata, llvm_cov, root): k
                for k, v in sorted(parts.items())}
        for f in concurrent.futures.as_completed(futs):
            binary, module = futs[f]
            try:
                _, files = f.result()
            except LookupError as exc:
                errors.append(str(exc))
                continue
            except subprocess.CalledProcessError as exc:
                errors.append("%s %s: exit %s" % (binary, module or "_", exc.returncode))
                continue
            except OSError as exc:
                errors.append("%s %s: %s" % (binary, module or "_", exc))
                continue
            out.append({"binary": binary, "module": module,
                        "files": {p: n for p, n in sorted(files.items()) if n > 0}})
    return sorted(out, key=lambda p: (p["binary"], p["module"] or "")), sorted(errors)


def expected_binaries(doc):
    """The test binaries of `nextest list` that run >= 1 test: the map must measure each."""
    out = []
    for bid, suite in sorted((doc.get("rust-suites") or {}).items()):
        for case in (suite.get("testcases") or {}).values():
            if case.get("ignored"):
                continue
            if (case.get("filter-match") or {}).get("status") == "mismatch":
                continue
            out.append(bid)
            break
    return out


def run_ok(rc):
    """Did nextest run to its end, green? Only exit 0 counts: red tests (100), a
    timeout (124), a kill (137, negative) or any other exit make the map incomplete."""
    return rc == 0


def _foreign_exit(rc):
    """A child's exit passed on: RED_EXIT is ours alone (the night reads it as
    RED and journals coverage_red) -- a foreign 6 (rsync) is a failed build, 1
    (re-check H2 m2)."""
    return 1 if rc == RED_EXIT else rc


def red_tests_from_log(path):
    """The red tests of a nextest log as `<binary-id> <test>`, in log order, each once;
    None when the log cannot be read (the caller then knows no names, never "none")."""
    try:
        with open(path, encoding="utf-8", errors="replace") as fh:
            lines = fh.read().splitlines()
    except OSError:
        return None
    order, red, tried = [], set(), {}
    for line in lines:
        m = _RED_LINE.match(line)
        if m:
            name = "%s %s" % (m.group(1), m.group(2))
            red.add(name)
        else:
            m = _TRY_LINE.match(line)
            if not m:
                continue
            # re-check H2 m4: the last try of a retried test decides
            name = "%s %s" % (m.group(3), m.group(4))
            tried[name] = m.group(1) not in (None, "PASS")
        if name not in order:
            order.append(name)
    return [n for n in order if n in red or tried.get(n)]


def verdict_of(cov):
    """GREEN, RED or INCOMPLETE of a map. GREEN needs `complete` (a map's own GREEN
    word never stands in for it); RED is a map with red tests; else INCOMPLETE."""
    if not cov:
        return "INCOMPLETE"
    if cov.get("verdict") == "RED" or cov.get("red_tests"):
        return "RED"
    return "GREEN" if is_complete(cov) else "INCOMPLETE"


def _shown(names, n=5):
    return ", ".join(names[:n]) + (" (+%d)" % (len(names) - n) if len(names) > n else "")


def objects_from_nextest_list(doc):
    """`{binary_id: path, "_bins": [workspace executables]}` from `cargo nextest list --message-format json`.

    The executables (`non-test-binaries`, e.g. `meclaw`) go to every export as
    extra objects: tests that boot a colony run them as child processes, and
    the child writes its profile into the test's directory (it inherits
    LLVM_PROFILE_FILE).
    """
    out, bins = {}, []
    for bid, suite in sorted((doc.get("rust-suites") or {}).items()):
        if suite.get("binary-path"):
            out[bid] = suite["binary-path"]
    meta = doc.get("rust-build-meta") or {}
    target = meta.get("target-directory") or ""
    for entries in (meta.get("non-test-binaries") or {}).values():
        for e in entries:
            # nextest names a bin target `bin-exe` (S1b, #1165: asking for
            # `bin` left this list empty and every child unmeasured)
            if e.get("kind") in ("bin", "bin-exe") and e.get("path"):
                path = e["path"]
                bins.append(path if os.path.isabs(path) else os.path.join(target, path))
    out["_bins"] = sorted(set(bins))
    return out


def profiles_per_binary(base):
    """`{binary: n}` -- the profile files each binary left under the runner's directory."""
    out = {}
    for (binary, _), files in profile_parts(base).items():
        out[binary] = out.get(binary, 0) + len(files)
    return out


def incomplete_reasons(run_ok, nextest_exit, errors, measured, expected, expected_ids,
                       profiles, measured_ids=None):
    """Why a map is not complete -- empty when it is."""
    out = []
    if not run_ok:
        out.append("nextest exit %s" % nextest_exit if nextest_exit is not None
                   else "run not finished")
    if errors:
        out.append("%d part errors" % len(errors))
    if expected_ids is not None and measured_ids is not None:
        # by set, not by count (review minor): a measured binary nobody
        # expected never stands in for an expected one that is missing
        missing = sorted(set(expected_ids) - set(measured_ids))
        if missing:
            shown = ", ".join(missing[:5]) + (" (+%d)" % (len(missing) - 5)
                                              if len(missing) > 5 else "")
            want = len(set(expected_ids))
            out.append("%d/%d binaries measured (missing: %s)"
                       % (want - len(missing), want, shown))
    elif measured < expected:
        out.append("%d/%d binaries measured" % (measured, expected))
    if profiles is not None:
        if not sum(profiles.values()):
            out.append("0 profiles")
        ids = list(expected_ids or [])
        empty = [b for b in ids if not profiles.get(b)]
        if empty:
            shown = ", ".join(empty[:5]) + (" (+%d)" % (len(empty) - 5) if len(empty) > 5 else "")
            out.append("no profiles for %d/%d binaries: %s" % (len(empty), len(ids), shown))
    return out


def coverage_map(rev, runner, parts, secs, tests, built_at=None, expected=None, errors=(),
                 run_ok=True, expected_ids=None, profiles=None, nextest_exit=None,
                 red_tests=None):
    """The map. `complete` only when the run finished green (`run_ok`), no part
    had an error, every expected binary was measured and -- given `profiles`
    (`profiles_per_binary`) -- left >= 1 profile; `incomplete` names each
    reason. `impact` narrows by a complete map of the matching rev only (review C1).
    `red_tests` (`red_tests_from_log`; None = no log read) and nextest exit 100
    make the `verdict` RED; a RED run without a name in the log says so."""
    mods = any(p["module"] for p in parts)
    measured_ids = {p["binary"] for p in parts}
    measured = len(measured_ids)
    if expected is None:
        expected = measured if expected_ids is None else len(expected_ids)
    errors = list(errors)
    why = incomplete_reasons(run_ok, nextest_exit, errors, measured, expected, expected_ids,
                             profiles, measured_ids=measured_ids)
    reds = list(red_tests or [])
    red = bool(reds) or nextest_exit == RED_NEXTEST_EXIT
    if reds:
        why.append("%d red tests: %s" % (len(reds), _shown(reds)))
    elif red and red_tests is not None:
        why.append("red tests (no name found in the nextest log)")
    return {"schema": SCHEMA, "rev": rev,
            "built_at": built_at or time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
            "runner": runner, "granularity": "module" if mods else "binary",
            "secs": round(secs, 1), "tests": tests, "parts": parts,
            "binaries_expected": expected, "binaries_measured": measured,
            "errors": len(errors), "error_list": errors[:50], "run_ok": bool(run_ok),
            "incomplete": why, "complete": not why, "red_tests": reds,
            "verdict": "RED" if red else ("INCOMPLETE" if why else "GREEN")}


def load_map(path):
    """`(map, None)`, or `(None, why)` for a file that is missing, no JSON or
    no map (review minor: a broken file was a traceback) -- one line for the
    caller to print, never a half-read map."""
    try:
        with open(path, encoding="utf-8") as fh:
            cov = json.load(fh)
    except OSError as exc:
        return None, "no coverage map %s (%s)" % (path, exc.strerror or exc)
    except ValueError as exc:
        return None, "broken coverage map %s (%s)" % (path, str(exc).splitlines()[0])
    if not isinstance(cov, dict) or not isinstance(cov.get("parts"), list) or "rev" not in cov:
        return None, "broken coverage map %s (no `rev` or `parts`)" % path
    return cov, None


def map_path(rev, map_dir=None):
    """Where `build` writes `coverage-<rev>.json`: `map_dir`, else the directory
    `narrow.newest_map()` reads (`narrow.coverage_dir()`). The lane output
    (`--out-dir`, coverage.json and the rest) stays where it is."""
    from pipeline import narrow
    return pathlib.Path(map_dir or narrow.coverage_dir()) / ("coverage-%s.json" % rev)


def is_complete(cov):
    """A map proved complete. A map without the field (older than C1) is not."""
    return bool(cov) and cov.get("complete") is True


def count_profiles(base):
    return sum(1 for f in pathlib.Path(base).rglob("*.profdata") if "_merged" not in f.parts)


# --- staleness and findings -------------------------------------------------

def age_days(cov, now=None):
    now = now or datetime.datetime.now(datetime.timezone.utc)
    at = datetime.datetime.fromisoformat(cov["built_at"].replace("Z", "+00:00"))
    return (now - at).total_seconds() / 86400.0


def check_stale(cov, now=None, db=None, db_path=None):
    """True when the map is older than 7 days; then journals `process_finding coverage_stale`."""
    age = age_days(cov, now)
    if age <= STALE_DAYS:
        return False
    _finding("coverage_stale", cov.get("rev") or "?", round(age, 2), STALE_DAYS,
             cov.get("built_at"), db=db, db_path=db_path)
    return True


def _finding(rule, subject, value, threshold, since, db=None, db_path=None, extra=None):
    from pipeline import events, store
    own = db is None
    if own:
        db = store.open(db_path)
    try:
        with store.tx(db):
            events.emit(db, "process_finding", "finding",
                        dict(extra or {}, rule=rule, subject=subject, value=value,
                             threshold=threshold, since=since))
    finally:
        if own:
            store.close(db)


def built(cov, db=None, db_path=None):
    """Journal `coverage_built` (V0 section 3) for a finished map."""
    from pipeline import events, store
    own = db is None
    if own:
        db = store.open(db_path)
    data = {"rev": cov["rev"], "runner": cov["runner"], "secs": cov["secs"],
            "tests": cov["tests"], "files": len({f for p in cov["parts"] for f in p["files"]}),
            "edges": sum(len(p["files"]) for p in cov["parts"]),
            "granularity": cov["granularity"]}
    if cov.get("browsers_opt_out") is not None:  # re-check tv-bn M7c M1: the hole is named
        data["browsers_opt_out"] = cov["browsers_opt_out"]
    try:
        with store.tx(db):
            events.emit(db, "coverage_built", "status", data)
    finally:
        if own:
            store.close(db)
    return data


# --- lanes ------------------------------------------------------------------

def lanes_file():
    """The kit's host file: `MECLAW_LANES_FILE`, else `~/.config/meclaw/lanes` (strand.sh lanes_file)."""
    return os.environ.get("MECLAW_LANES_FILE") or os.path.expanduser("~/.config/meclaw/lanes")


def lane_entry(name, hosts=None):
    """`(ssh_target, {key: value})` of lane `name` from the host file, or `(None, {})`."""
    try:
        with open(hosts or lanes_file(), encoding="utf-8") as fh:
            for line in fh:
                cols = line.split("#", 1)[0].split()
                if cols and cols[0] == name and len(cols) > 1:
                    opts = dict(c.split("=", 1) for c in cols[2:] if "=" in c)
                    return cols[1], opts
    except OSError:
        pass
    return None, {}


def lane_ssh(name, hosts=None):
    """The ssh target of lane `name` from the host file (never hard-coded)."""
    return lane_entry(name, hosts)[0]


def lane_refused(ssh, opts):
    """Why the coverage run never goes to this lane, or None (review I2):
    the owner's machine -- `local`, or class egon -- is no build lane."""
    if ssh == "local":
        return "the lane is the owner's machine (local)"
    if opts.get("class") == "egon":
        return "the lane is class egon (the owner's machine, beside the live colony)"
    return None


def lane_up(ssh):
    """True when `ssh` answers; `local` and an unknown lane never count as up."""
    if ssh in (None, "local"):
        return False
    try:
        from pipeline import lane  # the kit's narrow point (re-check tv-bn M7e B-M1)
        r = subprocess.run(lane.ssh_argv(ssh, "true", connect_timeout=5), capture_output=True, timeout=20)
        return r.returncode == 0
    except (OSError, subprocess.TimeoutExpired):
        return False


# --- CLI --------------------------------------------------------------------

def _holder(args):
    """The lease key of the build's strand: `--strand` in its `<wave>/<strand>`
    form (the night's and the kit's), "" otherwise -- no git process (the
    build's own subprocess calls stay the only ones)."""
    return args.strand if args.strand and "/" in args.strand else ""


def _refused(args, msg, code):
    """GH #1199 (kit find V2-A8): a build refused before its lane run says so
    on stderr AND stdout and at the lease; `code` is never 0."""
    from . import progress
    return progress.refuse("coverage build", msg, out=sys.stdout, err=sys.stderr,
                           holder=_holder(args), code=code, lane=getattr(args, "lane", "") or "")


def _pass(args, done, red):
    """`HH:MM:SS pass <done>/1 ...` flushed into the log and noted at the lease."""
    from . import progress
    line = progress.pass_line(done, 1, done, red)
    progress.emit(line, sys.stderr, sys.stdout)
    progress.beat(_holder(args), line, lane=getattr(args, "lane", "") or "")


def _cmd_coverage(args):
    if args.op == "aggregate":
        with open(args.objects, encoding="utf-8") as fh:
            objects = objects_from_nextest_list(json.load(fh))
        objects["_bins"] = spawn_closure.child_objects(objects["_bins"])   # S1b, #1165
        t0 = time.monotonic()
        harness = [b for b in (args.harness or "").split(",") if b]
        with open(args.objects, encoding="utf-8") as fh:
            expected = expected_binaries(json.load(fh))
        parts, errors = aggregate(args.profiles, objects, args.root, jobs=args.jobs,
                                  profdata=args.profdata, llvm_cov=args.llvm_cov,
                                  harness_bins=harness)
        cov = coverage_map(args.rev, args.runner, parts,
                           args.secs + (time.monotonic() - t0), count_profiles(args.profiles),
                           expected=len(expected), errors=errors,
                           run_ok=run_ok(args.nextest_exit), expected_ids=expected,
                           profiles=profiles_per_binary(args.profiles),
                           nextest_exit=args.nextest_exit,
                           red_tests=(red_tests_from_log(args.nextest_log)
                                      if args.nextest_log else None))
        # S1b (#1165): which spawned bins the map proves -- only the build sets it
        cov[spawn_closure.FIELD] = spawn_closure.marker(parts, objects["_bins"], args.root)
        if getattr(args, "browser_junit", None):   # S1b ruling 10.10.: only the build proves a lock
            cov[browser_cover.FIELD] = browser_cover.marker(parts, args.browser_junit)
        pathlib.Path(args.out).write_text(json.dumps(cov, sort_keys=True), encoding="utf-8")
        print(json.dumps({k: cov[k] for k in ("rev", "granularity", "secs", "tests", "complete",
                                              "verdict", "red_tests",
                                              "binaries_expected", "binaries_measured",
                                              "errors", "incomplete")}))
        return 0
    if args.op in ("stale", "journal"):
        cov, why = load_map(args.map)
        if why:
            print("coverage: %s" % why, file=sys.stderr)
            return 2
    if args.op == "stale":
        stale = check_stale(cov, db_path=args.db)
        print("stale" if stale else "fresh")
        return 1 if stale else 0
    if args.op == "journal":
        print(json.dumps(built(cov, db_path=args.db)))
        return 0
    # build: on a lane, never switching one on
    ssh, opts = lane_entry(args.lane)
    why = lane_refused(ssh, opts)
    if why:
        return _refused(args, "lane refused: %s; the coverage run goes to a build lane of the wave "
                              "only" % why, 5)
    if not lane_up(ssh):
        _finding("coverage_skipped_no_lane", args.lane, 0, 1,
                 time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()), db_path=args.db)
        return _refused(args, "lane %s is not up -- skipped (process_finding "
                              "coverage_skipped_no_lane)" % args.lane, 3)
    # the cargo token of the wave, through the kit (review I1): an armed wave
    # without a token for this strand and lane never starts the build
    tok = subprocess.run(["scripts/strand.sh", "token", "check", "--pid", str(os.getpid()),
                          "--lane", args.lane], cwd=os.path.dirname(_SCRIPTS))
    if tok.returncode != 0:
        return _refused(args, "no cargo token for lane %s (strand.sh token check exit %d) -- not built"
                        % (args.lane, tok.returncode), _foreign_exit(tok.returncode))
    cmd = ["timeout", "--kill-after=60", str(DEADLINE_S), "scripts/pipeline/coverage_lane.sh",
           ssh, args.strand, args.out_dir] + (["browser"] if args.scope == "browser" else [])
    print("coverage: %s" % " ".join(shlex.quote(c) for c in cmd), file=sys.stderr)
    _pass(args, 0, 0)   # GH #1199: the one pass of a build, said before the lane runs
    try:
        r = subprocess.run(cmd, cwd=os.path.dirname(_SCRIPTS))
    finally:
        # K9 (welle-validierung-2): `seen` once more after the lane run, whatever
        # its exit -- checked in only before it, a long build left `seen` at its
        # start and the idle reclaim took the lane back as "idle" right after it.
        # On the abort too (re-check v2-fix-b M4): a ^C or an exception in the
        # run goes on up, after the kit heard `seen`.
        subprocess.run(["scripts/strand.sh", "token", "check", "--pid", "0", "--lane", args.lane],
                       cwd=os.path.dirname(_SCRIPTS), stdout=subprocess.DEVNULL,
                       stderr=subprocess.DEVNULL)
    _pass(args, 1, int(r.returncode != 0))
    if r.returncode == TOOL_EXIT:
        _finding("coverage_tool_missing", args.lane, 0, 1,
                 time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()), db_path=args.db,
                 extra={"tool": "llvm-tools"})
        print("coverage: lane %s has no llvm-tools (rustup component add llvm-tools there) --"
              " no map (process_finding coverage_tool_missing)" % args.lane, file=sys.stderr)
        return TOOL_EXIT
    if r.returncode != 0:
        return _foreign_exit(r.returncode)
    path = pathlib.Path(args.out_dir) / "coverage.json"
    cov, why = load_map(path)  # W6 M5: a broken lane map is one line, no traceback
    if cov is None:
        print("coverage: %s" % why, file=sys.stderr)
        return 2
    if args.scope == "browser":   # S1b: the probe of the locks -- never installed, never journaled
        rc, line = browser_cover.probe_report(cov)
        print(line)
        return rc
    cov["runner"] = args.lane
    final = map_path(cov["rev"], getattr(args, "map_dir", None))
    final.parent.mkdir(parents=True, exist_ok=True)
    final.write_text(json.dumps(cov, sort_keys=True), encoding="utf-8")
    print(json.dumps(built(cov, db_path=args.db)))
    if verdict_of(cov) == "RED":
        reds = list(cov.get("red_tests") or [])
        _finding("coverage_red", cov["rev"], len(reds), 0, cov["built_at"], db_path=args.db,
                 extra={"tests": reds[:50]})
        print("coverage: map %s is RED: %s -- a finding (coverage_red), impact keeps rdeps"
              % (cov["rev"], ("%d red tests: %s" % (len(reds), _shown(reds, 20))) if reds
                 else "red tests, no name found in the nextest log"), file=sys.stderr)
        return RED_EXIT
    if not is_complete(cov):
        _finding("coverage_incomplete", cov["rev"], cov.get("binaries_measured", 0),
                 cov.get("binaries_expected", 0), cov["built_at"], db_path=args.db)
        print("coverage: map %s is NOT complete (%s/%s binaries, %s errors, run_ok=%s: %s) --"
              " impact keeps rdeps" % (cov["rev"], cov.get("binaries_measured"),
                                       cov.get("binaries_expected"), cov.get("errors"),
                                       cov.get("run_ok"),
                                       "; ".join(cov.get("incomplete") or ["no reason recorded"])),
              file=sys.stderr)
        return 1
    return 0


def register(sub):
    """`pipeline coverage build --lane <name>|aggregate|stale|journal` (V0 section 9)."""
    p = sub.add_parser("coverage", help="the coverage map (test part -> files it executes)")
    p.add_argument("op", choices=("build", "aggregate", "stale", "journal"))
    p.add_argument("--lane", help="build: the lane (a name from the host file)")
    p.add_argument("--strand", default="coverage", help="build: the strand name on the lane")
    p.add_argument("--out-dir", default=os.path.expanduser("~/.local/state/meclaw/pipeline/graph"))
    p.add_argument("--map-dir", default=None,
                   help="build: where coverage-<rev>.json goes (default: where narrowing reads,"
                        " $MECLAW_PIPELINE_COVERAGE_DIR or ~/.local/state/meclaw/pipeline/coverage)")
    p.add_argument("--map", help="stale/journal: the coverage map")
    p.add_argument("--profiles", help="aggregate: the runner's profile directory")
    p.add_argument("--objects", help="aggregate: `cargo nextest list --message-format json` output")
    p.add_argument("--root", help="aggregate: the tree root (paths become relative to it)")
    p.add_argument("--rev", default="worktree")
    p.add_argument("--runner", default="local")
    p.add_argument("--secs", type=float, default=0.0, help="aggregate: seconds the run took")
    p.add_argument("--jobs", type=int, default=None)
    p.add_argument("--nextest-exit", type=int, default=0,
                   help="aggregate: the exit of the nextest run (anything but 0 -> incomplete)")
    p.add_argument("--harness", default="", help="aggregate: comma list of harness binary ids")
    p.add_argument("--nextest-log", default=None,
                   help="aggregate: the nextest log; its red tests make the verdict RED, by name")
    p.add_argument("--out", help="aggregate: the map file to write")
    p.add_argument("--browser-junit", default=None,
                   help="aggregate: junit of the browser-lock call (S1b): sets `browser_locks`")
    p.add_argument("--scope", choices=("full", "browser"), default="full",
                   help="build: `browser` probes the browser locks alone; the map is never installed")
    p.add_argument("--profdata", default=os.environ.get("MECLAW_LLVM_PROFDATA", "llvm-profdata"))
    p.add_argument("--llvm-cov", default=os.environ.get("MECLAW_LLVM_COV", "llvm-cov"))
    p.add_argument("--db", default=None, help="store path (default $MECLAW_PIPELINE_DB)")
    p.set_defaults(func=_cmd_coverage)
