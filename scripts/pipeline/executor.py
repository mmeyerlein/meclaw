"""meclaw -- the lane executor: one dispatch, one run, a DAG of stations (`pipeline.py exec`).

Runs ON the lane (or on the owner's machine as the lane `local`), reads the
dispatch of the kern from stdin (plans/welle-dev-tempo V0 section 6) and
streams the events of V0 section 3 on stdout, one JSON object per line. It
never touches the store: everything the kern needs is in the stream.

ORDER: hygiene (the built-in disk check; W2's lane hygiene runs in front of
the executor, `lane_sync.sh guard` -> `run_guards` -> `hygiene_before`) ->
`lane_fingerprint` -> the stations as a DAG -> `pass_ended` (always last).
A station is ready when every station it comes `after` has ended and its
guard (`when`) holds; ready stations start in CPM order (longest remaining
path over the measured `calib_s`, else a declared `prior_s`, else
`timeout_min`), ties by least slack,
as soon as their `needs` fit the free capacity. `cpu` may be overbooked by
`+cpu_overbook`; every other dimension never; a station bigger than the lane
starts alone. A station with `alone: true` (`plan.ALONE`, GH #1191) books
`ALONE_CPU`, more than any lane holds: it starts on an empty lane, and nothing
with a booking starts beside it.

THE RESERVATIONS (GH #1162 B-S4-1, conservative since B-S4-3). Every station
that cannot start yet -- a predecessor has not ended, it does not fit, it waits
for the cargo lock, or a reservation keeps it back -- holds a reservation, in
rank order: its earliest start for its whole estimated length, after the
reservations of its predecessors, beside what runs and the reservations before
it, on every dimension (`earliest_slot`, `reserve`).
A ready station starts only if it moves none of them (conservative backfill).
The estimate is `calib_s`, else `prior_s`, else `timeout_min`; where none is
there nothing backfills. One reservation for the first wide station (EASY, B-S4-1) let a
narrow station take the width a narrow successor chain needed (V6: release warm
670.8 s, conservative 584.6 s). A station a reservation keeps back for
`RESERVE_AGE_S` in all -- only the time it would fit and waits counts --
waits for none, goes first and holds the first reservation (fairness).

THE CARGO LOCK. A station that needs `cargo` waits for the lane lock
(`+lock_file`), taken once per run before the first such station and held
until the run ends -- like `scripts/gate.sh` holds it per run, for the same
reason: one target directory takes one build at a time, and a lock released
between two stations hands a foreign rlib to the next one. A wait of a second
or more is its own NOTE row (`lock-wait`); the tree sync of
`scripts/cargo-target.sh` runs under the lock (`tree-sync` NOTE). Every child
started after that carries `MECLAW_CARGO_LOCK_HELD=1`.

THE BUILD STATION builds exactly the cargo targets of the parts that are not
cached (`build_argvs`) -- a filterset (`-E`) selects what RUNS, never what is
BUILT -- and then, still under the lock, writes `cargo nextest list
--list-type binaries-only` and `cargo metadata` to `<ws>/reuse/`. Every
nextest station after it runs through `scripts/test-tier.sh` with
`MECLAW_TIER_REUSE=<ws>/reuse`: nextest then reuses the build and never calls
cargo, so it runs beside `clippy` instead of queueing behind its lock.

LAB WIDTH (GH #1161). A station that books `colony` gets that many slots of the
port register (`ports.py`), the lowest free ones no running station holds, as
MECLAW_COLONY_SLOTS; the scenario runners take them as `--slots`. A booking the
register cannot serve is RED before the station starts. A station with
`parallel: {"by": "cmd"}` runs its commands side by side, each exactly once, each
at its share of the booked threads, each on its own nextest profile when the plan
names one (`parallel.profiles`); the first red part is the station's exit.

VERDICTS. A station's verdict is its exit code, except for the rules ported
from the station loop of `scripts/gate.sh` (`PORTED_RULES` below).

Exit: GREEN 0, RED 1, ABORTED 2, ASK 4 -- the verdict of `pass_ended`.

STOPPING A RUN. SIGTERM to the executor -- a foreign stop of its scope, or
`pipeline.py exec --abort <run>` (the pid in `<ws>/exec.pid`) -- aborts the
run like a watcher: the stations' own sessions get SIGTERM (SIGKILL after
the grace), every station that never started ends ABORTED, and the stream
ends with `pass_ended ABORTED` before the process goes; never `lost`.
"""
import datetime
import fcntl
import json
import math
import os
import re
import shlex
import shutil
import signal
import subprocess
import sys
import threading
import time
import xml.etree.ElementTree as ET

from . import events, finding, flakes, gitenv, hang_dump, ports, receipt, store

# W2's `hygiene.check(ctx, dispatch, ...)` collects and evicts workspaces under
# the lane root; it runs in FRONT of this executor (`lane_sync.sh guard` ->
# `run_guards` -> `hygiene_before`, the same place as for a legacy `run`), so
# the executor keeps only its own disk check and emits `hygiene_checked` once.

EXIT = {"GREEN": 0, "RED": 1, "ABORTED": 2, "ASK": 4}
DIMS = ("cpu", "colony", "browser_chromium", "browser_webkit", "export", "cargo")
WATCH_PERIOD_S = 30          # the one clock of the executor (OR-DT-8): the disk watcher
KILL_GRACE_S = 10
# TV-K5 (b), ruling K6 2026-10-11: a station cap is a hang marker. `_spawn` waits
# up to the cap, writes the diagnosis and kills; `timeout` around the command is
# only the backstop this many seconds later.
HANG_BACKSTOP_S = 60
HANG_TAIL_LINES = 40
# The test a nextest or unittest log names as running: nextest's `SLOW`/`START`/
# `TERMINATING` lines (`SLOW [> 60.000s] <binary> <test>`), unittest's `-v` line
# without its verdict (`test_x (mod.Class) ... `).
_RUNNING_TEST = re.compile(r"^\s*(?:SLOW|START|TERMINATING)\s+\[[^]]*\]\s+(.+?)\s*$")
_RUNNING_UNIT = re.compile(r"^(test\S* \([^)]+\))(?: \.\.\.)?\s*$")


def hang_line(name, cap_min):
    """`HANG: station <name> exceeded <n> min` -- the RED reason of a cap."""
    try:
        cap = "%g" % float(cap_min if cap_min is not None else 30)
    except (TypeError, ValueError):
        cap = str(cap_min)
    return "HANG: station %s exceeded %s min" % (name, cap)


def running_test(lines):
    """The last test the log names as running, or None."""
    for line in reversed(lines):
        m = _RUNNING_TEST.match(line) or _RUNNING_UNIT.match(line)
        if m:
            return m.group(1)
    return None


def hang_diagnosis(st, log, pid):
    """Write the hang diagnosis of a station over its cap into its log, before
    the kill: the line, the running test, the last 40 log lines and the
    process tree of the command's session (`ps -o pid,etime,args --forest`)."""
    try:
        log.flush()
        with open(log.name, encoding="utf-8", errors="replace") as fh:
            lines = fh.read().splitlines()
    except (OSError, AttributeError, ValueError):
        lines = []
    tail = lines[-HANG_TAIL_LINES:]
    try:
        tree = subprocess.run(["ps", "-o", "pid,etime,args", "--forest", "--sid", str(pid)],
                              stdin=subprocess.DEVNULL, capture_output=True, text=True,
                              timeout=10).stdout
    except (OSError, subprocess.SubprocessError) as exc:
        tree = "ps failed: %s\n" % exc
    out = ["", hang_line(st.get("name"), st.get("timeout_min")),
           "running test: %s" % (running_test(lines) or "none named in the log"),
           "--- last %d log lines ---" % HANG_TAIL_LINES] + tail + [
           "--- process tree (ps -o pid,etime,args --forest) ---"]
    try:
        log.write("\n".join(out) + "\n" + tree)
        log.flush()
    except (OSError, ValueError):
        pass


GREENISH = ("GREEN", "CACHED")   # a FLAKY is a finding (OR-TV-2), green only as legacy


def ends_red(data):
    """True when a `station_ended` body counts red for the pass: RED, and a FLAKY
    unless it carries `legacy: true` (VG TV-F1/TV-F4)."""
    v = data.get("verdict")
    return v == "RED" or (v == "FLAKY" and data.get("legacy") is not True)


def pass_verdict(ends, aborted=False, station_aborted=False):
    """The pass verdict from its `station_ended` bodies (`finish`)."""
    if aborted:
        return "ABORTED"
    if any(ends_red(d) for d in ends):
        return "RED"
    if station_aborted or any(d.get("verdict") == "ABORTED" for d in ends):
        return "ABORTED"
    if any(d.get("verdict") == "ASK" for d in ends):
        return "ASK"
    return "GREEN"
# `scripts/test-tier.sh`: an empty selection (EXIT_NO_TESTS, GH #1167 i).
NO_TESTS_EXIT = 6
# The colon is part of it: a refused filterset says
# `no tests matched (filterset refused): ...` and is never an empty rest.
NO_TESTS_SIGNATURE = "test-tier: RED -- no tests matched: "
LISTED_PREFIX = "test-tier: LISTED "

# scripts/gate.sh PRECHECK_STOP_ALSO: stations whose own cargo column is 0 and
# that still cannot be judged without the build.
PRECHECK_STOP_ALSO = frozenset({"scenarios:memory", "scenarios:builder", "export-audit",
                                "persona-cases", "export-tree-tests"})
# scripts/gate.sh missing_tool: station -> the tool it cannot run without (SKIP).
MISSING_TOOL = {"shellcheck": "shellcheck", "deny": "cargo-deny", "deny-advisories": "cargo-deny"}
# GH #721 (`registry lacks` in scenarios:builder) has no retry of its own any
# more: a second round that decided the station was a retry counted as a verdict
# (TV-B8, OR-TV-2; fund tv-bn-ci, Welle Testing). The red round is judged like
# any other and its case goes to the triage, which carries #721 (`flakes.GH721_*`).
EXIT_NO_TESTS = 6            # scripts/test-tier.sh: a filter in which no test ran
PID_FILE = "exec.pid"        # <ws>/exec.pid: the executor of the run, for `exec --abort`
LANE_ROOT = "/srv/target"    # the lane root when neither --ws nor MECLAW_LANE_ROOT names one
TERMINATED = (-signal.SIGTERM, 128 + signal.SIGTERM)

PORTED_RULES = """
Ported from the station loop of scripts/gate.sh:
  run: false              tests -> SKIP planned-for-shards, slow-tests -> SKIP no-tests-station-in-ci
  missing tool            shellcheck / deny / deny-advisories without the tool -> SKIP <tool> not installed
  browser_short           browser:* without node or playwright -> SKIP that counts in the total
  display-lab             no node on the lane -> RED no-node (lane)
  --with <class>          a paid suite class on a lane -> RED, never a network call
  precheck red            cargo stations, build stations, stations after one and PRECHECK_STOP_ALSO -> SKIP precheck red
  corpus-committed        strand: NOTE (matches its sources | the corpus is stale ...)
  test-bans-inventory     exit 0 -> NOTE with the TEST-BANS-INVENTORY line; else RED with it
  slow-tests              SLOW-VERDICT as reason; exit 1 -> NOTE in every mode (TV-K5); exit 2 -> RED
  persona-receipt         0 GREEN, 6 RED, 3/4/5 ASK or NOTE by --decide; strand NOTE; other exit RED
  deny-advisories         NOTE clean | NOTE findings -- read <log>
  scenarios:builder       no retry: a red round is red, `registry lacks` goes to the triage (GH #721, TV-B8)
  lock-wait / tree-sync / stage-lock   NOTE rows before the first cargo station
"""


# --- small helpers -------------------------------------------------------------
def _which(name, env):
    return shutil.which(name, path=env.get("PATH"))


def _parse_ts(text):
    if not text:
        return None
    try:
        ts = datetime.datetime.fromisoformat(text.replace("Z", "+00:00"))
    except ValueError:
        return None
    if ts.tzinfo is None:
        ts = ts.replace(tzinfo=datetime.timezone.utc)
    return ts


def _free_g(path):
    p = path
    while p and not os.path.exists(p):
        p = os.path.dirname(p)
    try:
        return round(shutil.disk_usage(p or "/").free / 2**30, 1)
    except OSError:
        return None


# A browser station that could not drive a browser is a SKIP that counts in the
# total (gate.sh `browser_short`): its `station_ended` carries `in_total`.
COUNTED_SKIP_PREFIXES = ("no-node:", "no-playwright:")


def legacy_v1(doc):
    """The v1 file of a v2 receipt (`target/gate/<tree>/<mode>-<rev>.json`).

    `receipt.legacy` reads CACHED/FLAKY as GREEN; the file keeps exactly the
    top-level keys `scripts/gate.sh` wrote (`receipt.V1_KEYS`)."""
    old = receipt.legacy(doc)
    return {k: old[k] for k in receipt.V1_KEYS}


def _write_json_atomic(path, doc):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    tmp = "%s.tmp.%d" % (path, os.getpid())
    with open(tmp, "w", encoding="utf-8") as fh:
        json.dump(doc, fh, indent=2)
        fh.write("\n")
    os.replace(tmp, path)


# The cpu an `alone` station books (GH #1191): above every lane and every overbook,
# so `_fits` ("bigger than the lane: it runs alone") starts it only when no station
# holds a thread, and none fits beside it -- every station books at least one
# thread (`plan._needs`). Its threads stay the plan's `needs.cpu` (`_book_threads`).
# gh1002 measured the lane's load beside `tests`: 6/10 isolated on build04.
ALONE_CPU = 1e6


def _needs(st):
    # A view of its own on purpose (review Z-A R2 M4): the executor reserves
    # what a station declares; whether a pass goes to a lane is the one
    # predicate of gate/schedule. Hand plans may differ here; it goes with
    # the legacy path in Z-I.
    n = {d: 0 for d in DIMS}
    for k, v in (st.get("needs") or {}).items():
        if k in n:
            n[k] = float(v or 0)
    if st.get("alone"):
        n["cpu"] = ALONE_CPU
    if "needs" not in st and st.get("cargo") == 1:  # legacy plan without `needs` only
        n["cargo"] = max(n["cargo"], 1)
    return n


ABORT_REASON = re.compile(r"^[a-z_]{1,32}$")


def _abort_reason(kind):
    """The `reason` of a station ended by an abort of `kind` (V0 section 2:
    `aborted_disk|aborted_process|aborted_deadline`; a W2 watcher kind keeps its name)."""
    kind = str(kind or "process")
    return "aborted_%s" % kind if ABORT_REASON.match(kind) else "aborted_process"


SELFTEST_SHARDS_ENV = "MECLAW_SELFTEST_SHARDS"


def _book_selftest_shards(env, st):
    """gate-selftest starts the shards the lane booked: the resolved `needs.cpu` goes to
    the runner as `MECLAW_SELFTEST_SHARDS`, on a remote lane too, and replaces any
    override in `env` -- an invalid one included, so the station runs the booked number
    (its log says so: `MECLAW_SELFTEST_SHARDS_IGNORED`)."""
    try:
        cpu = int((st.get("needs") or {}).get("cpu") or 0)
    except (TypeError, ValueError):
        return
    if cpu >= 1:
        raw = env.get(SELFTEST_SHARDS_ENV)
        if raw is not None and raw != str(cpu):  # the runner's log names the replaced value
            env[SELFTEST_SHARDS_ENV + "_IGNORED"] = raw
        env[SELFTEST_SHARDS_ENV] = str(cpu)


# The slots of the port register a colony station booked (GH #1161), handed over
# like MECLAW_SELFTEST_SHARDS: `scenarios:memory` booked eight colonies and its
# runner, given no slot, ran one case at a time (396 s on build03, 2026-10-09).
COLONY_SLOTS_ENV = "MECLAW_COLONY_SLOTS"


def _part_width(threads, parts):
    """The threads of one part of a `parallel` station: its share of the booking, so
    the parts together never run wider than the station booked (0 = not booked)."""
    if threads < 1:
        return 0
    return max(1, threads // max(1, parts))


def _is_parts(st, cmds):
    """`parallel: {"by": "cmd"}` (V0 section 4) with more than one command."""
    return (st.get("parallel") or {}).get("by") == "cmd" and len(cmds) > 1


def _tier_filter_argv(argv, minus):
    """A `test-tier.sh filter <expr> ...` argv with `minus` taken out of its filter;
    every other argument (`--run-ignored all`) stays. Anything else unchanged."""
    if len(argv) >= 3 and os.path.basename(argv[0]) == "test-tier.sh" and argv[1] == "filter":
        return [argv[0], argv[1], "(%s) & not (%s)" % (argv[2], minus)] + list(argv[3:])
    return list(argv)


def _tier_filter_argv_set(argv, expr):
    """A `test-tier.sh filter <expr> ...` argv with its filter replaced by `expr`."""
    if len(argv) >= 3 and os.path.basename(argv[0]) == "test-tier.sh" and argv[1] == "filter":
        return [argv[0], argv[1], expr] + list(argv[3:])
    return list(argv)


def _station_threads(st, env):
    """The thread count a station's tests ran with (T2 of the triage, VG TV-F2)."""
    got = str((env or {}).get("NEXTEST_TEST_THREADS") or "")
    if got.isdigit() and int(got) > 0:
        return int(got)
    return max(1, int((st.get("needs") or {}).get("cpu") or 1))


# The live Egon's machine never runs a station wider (watchdog trips, 29.08.).
EGON_MAX_THREADS = 4
# The dispatch key with the runner's class from the store (`runners.class`,
# #1158): `lane` books its full width, everything else -- `egon`, a pass
# without a lane, a missing or unknown class -- stays under the Egon cap, and
# so does every class on a machine whose host file names a local egon lane.
# `+nice` is the nice/ionice prefix only and never lifts the cap.
RUNNER_CLASS_KEY = "+runner_class"
UNCAPPED_CLASSES = ("lane",)
# libtest (`cargo test --doc` in `doctests`) reads only RUST_TEST_THREADS (re-check I-2c).
THREAD_KEYS = ("NEXTEST_TEST_THREADS", "CARGO_BUILD_JOBS", "RUST_TEST_THREADS")


def egon_machine():
    """True when THIS machine's host file names a `local` lane of class egon:
    the live Egon runs here, whatever the dispatch says (#1158 B-1). A host
    file that cannot be read as lanes counts as Egon's (fail-closed)."""
    from . import lane  # lazy: lane is the transport, the executor runs without it
    try:
        return any(ln.transport == "local" and ln.cls == "egon" for ln in lane.load_lanes())
    except Exception:  # noqa: BLE001 -- an unreadable host file never lifts the cap
        return True


def egon_capped(d):
    """True unless the dispatch names a runner class that may run its booked
    width (fail-closed: no dispatch, no key or an unknown value is Egon's) on a
    machine that is not Egon's own (`egon_machine`)."""
    cls = (d or {}).get(RUNNER_CLASS_KEY)
    return not (isinstance(cls, str) and cls in UNCAPPED_CLASSES) or egon_machine()


def _book_threads(env, st, egon=False):
    """Every station runs at most the threads it booked (re-check Z-I I-2): nextest
    threads, cargo jobs and libtest threads are the resolved `needs.cpu`, on a remote lane too, and
    replace the run-wide width of the lane; on egon never over `EGON_MAX_THREADS`.
    An unresolved symbol (< 1) leaves `env` alone -- admission refuses it."""
    try:
        cpu = int((st.get("needs") or {}).get("cpu") or 0)
    except (TypeError, ValueError):
        return
    if cpu < 1:
        return
    if egon:
        cpu = min(cpu, EGON_MAX_THREADS)
    for key in THREAD_KEYS:
        env[key] = str(cpu)


# GH #1162 B-S4-1: the fairness bound of the reservations (V6 sim: 120 s and more keep
# the whole gain of the warm release, 60 s eats it). Since B-S4-3 it counts only the
# time a reservation keeps a station back; counted from the first hold, 300 s cost the
# warm release 13.5 s (598.1 against 584.6).
RESERVE_AGE_S = 300.0


def _fits_beside(need, used, cap):
    """Whether `need` fits beside `used` under `cap` by the rule of `_Run._fits`:
    every dimension, a station bigger than the lane fits an empty one."""
    for dim, cap_d in cap.items():
        n = need.get(dim, 0)
        u = used.get(dim, 0)
        if n > 0 and u > 0 and u + n > cap_d + 1e-9:
            return False
    return True


def earliest_slot(need, secs, ivals, ready_at, cap):
    """The earliest time >= `ready_at` at which `need` fits for `secs` seconds
    beside the intervals `ivals` ([(needs, start, end)]: what runs and the
    reservations before it) under `cap`; None when `ready_at`, `secs` or an end
    is unknown -- nothing backfills in front of a reservation it cannot time."""
    if ready_at is None or secs is None or any(e is None for _, _, e in ivals):
        return None
    for t in sorted({ready_at} | {e for _, _, e in ivals if e > ready_at}):
        points = sorted({t} | {s for _, s, _ in ivals if t < s < t + secs})
        if all(_fits_beside(need, _used_at(ivals, x), cap) for x in points):
            return t
    return None  # not reached: after the last end the lane is empty


def _used_at(ivals, x):
    used = {}
    for needs, start, end in ivals:
        if start <= x + 1e-9 and end > x + 1e-9:
            for dim, v in needs.items():
                used[dim] = used.get(dim, 0) + v
    return used


def reserve(blocked, ivals, cap):
    """The start each blocked station reserves beside `ivals` and the
    reservations before it, in rank order (GH #1162 B-S4-3). A row is
    (needs, ready_at, secs[, after]): `after` names the earlier rows that must
    end first. None when one of them cannot be timed."""
    ivals, out = list(ivals), []
    for row in blocked:
        need, ready_at, secs = row[:3]
        for i in (row[3] if len(row) > 3 else ()):
            if ready_at is not None:
                ready_at = max(ready_at, out[i] + blocked[i][2])
        t = earliest_slot(need, secs, ivals, ready_at, cap)
        if t is None:
            return None
        out.append(t)
        if secs:
            ivals.append((need, t, t + secs))
    return out


def _finite_secs(x):
    """`x` as seconds when it is a finite number > 0, else None (GH #1162 hA-m-1):
    the reservations only ever end when every estimate does. The same rule is
    `schedule._finite_secs` on the kern side."""
    try:
        x = float(x)
    except (TypeError, ValueError):
        return None
    return x if math.isfinite(x) and x > 0 else None


def _estimate(st):
    """The seconds a station is expected to take: `calib_s` (measured), else `prior_s`
    (declared, `plan.PRIOR_S`), else `timeout_min`, else None. A value that is no
    finite number > 0 is as if it were not there."""
    for field in ("calib_s", "prior_s"):
        secs = _finite_secs(st.get(field))
        if secs is not None:
            return secs
    minutes = _finite_secs(st.get("timeout_min"))
    return None if minutes is None else minutes * 60


def _needs_cargo(st):
    return _needs(st)["cargo"] > 0


def _tier_filter(argv):
    """Is `argv` a `scripts/test-tier.sh filter <expr> ...` command?"""
    return len(argv) >= 3 and argv[0].endswith("test-tier.sh") and argv[1] == "filter"


def _after(st, names):
    return [a for a in (st.get("after") or []) if a in names and a != st["name"]]


def _isolation_cause(failed, rc, log_path):
    """Why `crate-isolation` is red (#1166 re-check m-5): the `-p` whose call
    failed alone, or the cause of a refused member list (the line
    `crate-isolation: <manifest>:1: <cause>` its one command prints)."""
    argv = list(failed[0][0]) if failed else []
    if "-p" in argv[:-1]:
        return "`cargo check -p %s` failed alone (exit %d)" % (argv[argv.index("-p") + 1], rc)
    try:
        with open(log_path, encoding="utf-8", errors="replace") as fh:
            for line in fh:
                if line.startswith("crate-isolation: "):
                    return line[len("crate-isolation: "):].rstrip("\n")
    except OSError:
        pass
    return ""


# --- the build station ------------------------------------------------------------
def _joint_groups(by_crate, test_targets):
    """The crates of `by_crate` ({crate: [test names]}) in groups that ONE cargo
    call each builds exactly: cargo applies every `--test` to every `-p`, so a
    crate joins a group only while no name of the group hits a test target of a
    member that member did not ask for. A crate whose test targets
    (`test_targets`, from the plan's metadata) are unknown builds alone
    (fail-closed). One call instead of one per crate: every separate `-p` set
    is its own feature variant of the shared crates, built and kept apart
    (S6-DIAG, #1166: 3 calls 164 s, merged 56 s). The price: no crate is built
    in its own feature variant here any more -- the release station
    `crate-isolation` (gate_plan.py) checks each member alone."""
    known = test_targets or {}
    groups = []
    for crate in by_crate:
        if crate in known:
            for g in groups:
                if all(m in known for m in g):
                    names = {b for m in g + [crate] for b in by_crate[m]}
                    if all(set(known[m]) & names <= set(by_crate[m]) for m in g + [crate]):
                        g.append(crate)
                        break
            else:
                groups.append([crate])
        else:
            groups.append([crate])
    return groups


def build_argvs(build, cached_parts, all_parts=None):
    """The cargo argv lists (without `cargo`) for the `build` station.

    `build` is the plan's field {meclaw_bin, workspace, packages, tests:
    [[crate, binary]], lib_bins: [crate] | true, extra: [[crate, binary]]};
    `cached_parts` holds the part ids (`crate::binary`, a crate for its lib)
    whose results the kern took from the cache -- those are not built. A whole
    package (`packages`, `lib_bins`) is left out only when every part of it in
    `all_parts` (the parts of all stations) is cached; without `all_parts` it is
    always built. One `nextest run --no-run` per group of crates for the named
    binaries (`_joint_groups`, `test_targets` from the plan), one for whole
    packages, one for `--lib --bins`; `workspace` replaces all of them. A
    binary of a crate the package call builds is not named once more (GH #1162:
    the browser locks were built twice).

    `restrict` ([[crate, binary]], phase 1 `build:browser`, GH #1162 Weg d)
    builds only those binaries, each in the call of the full build that builds
    it, with that call's `-p` set and nothing else changed: resolver 3 unifies
    features per cargo call, so a narrower set gives other unit hashes and the
    crates build twice (build04, run-9e5307de0bc4). The groups therefore come
    from the full `by_crate`, never from the restricted pairs; a call may name
    a binary another member of its group asks for in phase 2 (a subset of
    phase 2, same variant). No `--bin meclaw`."""
    def covered(crate):
        if all_parts is None:
            return False
        mine = [p for p in all_parts if p == crate or p.startswith(crate + "::")]
        return bool(mine) and all(p in cached_parts for p in mine)

    restrict = build.get("restrict")
    if restrict is not None:
        restrict = [(c, b) for c, b in restrict if "%s::%s" % (c, b) not in cached_parts]
    out = []
    if build.get("meclaw_bin") and restrict is None:
        out.append(["build", "--bin", "meclaw"])
    no_run = ["nextest", "run", "--no-run"]

    def tests_of(pairs):
        argv = []
        for _c, b in pairs:
            if b not in argv[1::2]:
                argv += ["--test", b]
        return argv
    if build.get("workspace"):
        if restrict is None:
            out.append(no_run + ["--workspace"])
        elif restrict:
            out.append(no_run + ["--workspace"] + tests_of(restrict))
        return out
    pkgs = [p for p in (build.get("packages") or []) if not covered(p)]
    by_crate = {}
    for crate, binary in list(build.get("tests") or []) + list(build.get("extra") or []):
        if "%s::%s" % (crate, binary) in cached_parts or crate in pkgs:
            continue
        bins = by_crate.setdefault(crate, [])
        if binary not in bins:
            bins.append(binary)
    groups = _joint_groups(by_crate, build.get("test_targets"))
    if restrict is not None:
        in_pkgs = [(c, b) for c, b in restrict if c in pkgs]
        if in_pkgs:
            argv = list(no_run)
            for p in pkgs:
                argv += ["-p", p]
            out.append(argv + tests_of(in_pkgs))
        for group in groups:
            mine = [(c, b) for c, b in restrict if c in group]
            if mine:
                out.append(no_run + [a for c in group for a in ("-p", c)] + tests_of(mine))
        return out
    for group in groups:
        # one `--test` per name: cargo applies it to every `-p` of the call
        # anyway, and two members asking for their `misc` share it (re-check m-2)
        out.append(no_run + [a for c in group for a in ("-p", c)]
                   + tests_of([(c, b) for c in group for b in by_crate[c]]))
    if pkgs:
        argv = list(no_run)
        for p in pkgs:
            argv += ["-p", p]
        out.append(argv)
    lib_bins = build.get("lib_bins")
    if lib_bins is True:
        out.append(no_run + ["--workspace", "--lib", "--bins"])
    elif lib_bins:
        crates = [c for c in lib_bins if not covered(c)]
        if crates:
            argv = list(no_run)
            for c in crates:
                argv += ["-p", c]
            out.append(argv + ["--lib", "--bins"])
    return out


CARGO_TARGET_SH = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
                               "cargo-target.sh")


def test_features(tree, argv):
    """The test-hook flags (`--features meclaw-colony/test-hooks` or nothing)
    for a cargo build of the build station with the selection in `argv`
    (GH #1208, review H8; `build --bin meclaw` too since re-check M1).

    `#![cfg(feature = "test-hooks")]` tests were empty stubs in every gate: no
    test build turned the feature on. The rule lives ONCE, in
    `meclaw_test_features` of scripts/cargo-target.sh -- the file
    `scripts/test-tier.sh` reads it from too, so the build station and a tier
    run build the same variant of meclaw-colony into the shared target/. The
    tree's own copy wins (it is what is gated), this checkout's is the
    fallback. A probe that fails is an error, never "no flags": a silent empty
    answer would put the stubs back."""
    lib = os.path.join(tree, "scripts", "cargo-target.sh")
    if not os.path.isfile(lib):
        lib = CARGO_TARGET_SH
    res = subprocess.run(
        ["bash", "-c", '. "$1" || exit 2; shift; meclaw_test_features "$@"', "_", lib, tree] + list(argv),
        capture_output=True, text=True, timeout=30)
    if res.returncode:
        raise RuntimeError("meclaw_test_features failed (exit %d): %s"
                           % (res.returncode, res.stderr.strip()))
    return [ln for ln in res.stdout.splitlines() if ln]


def unrequested_binaries(build, cached_parts, selections, docs):
    """The binary ids `nextest list` names for a `--test` call of the build
    station that the plan never asked for (re-check m-1, #1166): cargo applies
    every `--test` to every `-p`, and `_joint_groups` keeps the binary set equal
    only as far as the plan's metadata knows the test targets. This reads
    cargo's own answer. Asked for = `tests` + `extra` of `build` minus the
    cached parts; whole-package and `--lib --bins` calls build all of their
    crates by design and are not judged."""
    asked = {"%s::%s" % (c, b) for c, b in
             list(build.get("tests") or []) + list(build.get("extra") or [])}
    asked -= set(cached_parts or ())
    out = []
    for sel, doc in zip(selections, docs):
        if "--test" not in sel:
            continue
        for bid in sorted((doc or {}).get("rust-binaries") or {}):
            if bid not in asked and bid not in out:
                out.append(bid)
    return out


def _merge_binaries(docs):
    """One `binaries-only` document out of several (one per build selection)."""
    merged = {"rust-build-meta": {}, "rust-binaries": {}}
    for doc in docs:
        meta = merged["rust-build-meta"]
        for k, v in (doc.get("rust-build-meta") or {}).items():
            if k not in meta:
                meta[k] = v
            elif isinstance(v, dict) and isinstance(meta[k], dict):
                for kk, vv in v.items():
                    meta[k].setdefault(kk, vv)
            elif isinstance(v, list) and isinstance(meta[k], list):
                meta[k] += [x for x in v if x not in meta[k]]
        merged["rust-binaries"].update(doc.get("rust-binaries") or {})
    return merged


# --- JUnit ------------------------------------------------------------------------
def junit_results(path):
    """Per-binary results of a nextest JUnit file: [{part, verdict, secs, tests, failed_names}]."""
    out = []
    root = ET.parse(path).getroot()
    suites = [root] if root.tag == "testsuite" else list(root.iter("testsuite"))
    for suite in suites:
        tests = {"total": 0, "passed": 0, "failed": 0, "flaky": 0, "skipped": 0}
        failed, secs = [], 0.0
        for tc in suite.iter("testcase"):
            tests["total"] += 1
            try:
                secs += float(tc.get("time") or 0)
            except ValueError:
                pass
            tags = {child.tag for child in tc}
            if tags & {"failure", "error"}:
                tests["failed"] += 1
                failed.append(tc.get("name", ""))
            elif "skipped" in tags:
                tests["skipped"] += 1
            else:
                tests["passed"] += 1
                if tags & {"flakyFailure", "flakyError", "rerunFailure", "rerunError"}:
                    tests["flaky"] += 1
        res = {"part": suite.get("name", ""), "verdict": "RED" if failed else "GREEN",
               "secs": round(secs, 3), "tests": tests}
        if failed:
            res["failed_names"] = failed
        out.append(res)
    return out


# --- the fingerprint ----------------------------------------------------------------
# Keys of the fingerprint that measure a moment, not the lane: the free space
# moves with every run on the target (tv-bn M7o: the second lane run of
# test_pipeline_ir_path.CacheOffLane under four pre shards compared the first
# run's `target_free_g` and wrote a `lane-drift` NOTE for an unchanged lane).
# `incremental_policy` is the run's CARGO_INCREMENTAL, measured under the mode's
# words (re-check tv-bn M7t A-M3): an integration pass beside a register written
# under `test` words differs by its mode, not by its lane.
FINGERPRINT_MEASURES = frozenset({"target_free_g", "incremental_policy"})


def fingerprint_drift(fp, expected):
    """The keys in which the lane `fp` differs from its register `expected`: a
    measure (FINGERPRINT_MEASURES) never, a value None on either side (a probe
    that timed out under load, a key the register did not have) never -- the
    rule of `hygiene.fingerprint_drift`."""
    return sorted(k for k, v in (expected or {}).items()
                  if k in fp and k not in FINGERPRINT_MEASURES
                  and v is not None and fp[k] is not None and fp[k] != v)


# Every field of a fingerprint (V0 section 2.3), the order of `lane_fingerprint.sh`.
FINGERPRINT_FIELDS = ("rustc", "cargo", "nextest", "node", "playwright", "python", "kernel",
                      "vcpu", "cores", "ram_g", "target_fs", "target_free_g", "ulimit_n",
                      "incremental_policy", "linker", "build_env", "cargo_config", "sccache", "linger",
                      "pdftotext", "portal_venv", "browsers")


def _run_fingerprint_sh(args, env, argv=None, prelude="", cwd=None):
    """`lane_fingerprint.sh <args>` under `env` in `cwd`: `(answer, None)`, or `(None, why)`
    when it did not answer -- no start, a time-out, an exit other than 0, no JSON
    object (re-check tv-bn M7r A-M1: a half answer is no answer). `argv`: a
    command that runs the script from its stdin (`sh -s`) on a lane instead,
    `prelude` before the script on that stdin. `cwd`: the tree whose toolchain
    rustup picks (re-check bundle N kit-fp M1, N-K1); a local run without one
    answers nothing -- the gate's own cwd chose `rustup default`."""
    sh = shutil.which("sh") or "/bin/sh"
    try:
        if argv is None:
            if not cwd:
                return None, "no tree to measure in"
            r = subprocess.run([sh, LANE_FINGERPRINT_SH] + list(args), capture_output=True, text=True,
                               timeout=60, env=dict(env), stdin=subprocess.DEVNULL, cwd=cwd)
        else:
            with open(LANE_FINGERPRINT_SH, encoding="utf-8") as fh:
                script = fh.read()
            r = subprocess.run(list(argv), capture_output=True, text=True, timeout=60,
                               input=prelude + script)
    except (OSError, subprocess.SubprocessError) as exc:
        return None, "no answer (%s)" % exc.__class__.__name__
    if r.returncode != 0:
        return None, "exit %d" % r.returncode
    try:
        said = json.loads(r.stdout)
    except ValueError:
        return None, "no JSON"
    if not isinstance(said, dict):
        return None, "no JSON object"
    return said, None


def station_env(plus_env, environ):
    """The environment every station of a run starts in -- THE one rule of it
    (re-check tv-bn M7t A-M2, OR-T11-22): `environ`, the environment the executor
    inherits, without git's repository variables, under the dispatch's `+env`.
    The executor takes it with its own `os.environ` (`base_env`); the gate's cache
    lookup takes it before a run on this machine or a lane of transport `local`
    with this process's (`stations_build`). A host's stations start under `env -i`
    of REMOTE_BASE with the `+env` over it, no `station_env` there: `stations_build`
    measures that environment on the host itself, and `gate._cache_doubt` refuses a
    record when the run's differs (fail-closed). A second rebuild beside it looked up
    without the gate's RUSTFLAGS a local executor then built with: CACHED across
    a change of the build."""
    env = gitenv.without_repo_env(environ)
    env.update({k: str(v) for k, v in (plus_env or {}).items()})
    return env


def build_fingerprint(env, tree):
    """`{"rustc", "cargo", "linker", "build_env", "cargo_config"}` of the stations'
    environment `env` in `tree`: the script's `--build-env <tree>` (re-check tv-bn
    M7r A-M2, one source), started IN the tree like the stations, so rustup picks
    the tree's toolchain (N-K1), and the cargo configuration outside the tree is
    measured from its parent up (N-K2). The gate lays it over the register's
    fingerprint before its cache lookup, so a change of RUSTFLAGS, a CARGO_PROFILE_*,
    the toolchain pin or a `config.toml` since the register was written is a change
    of the key, never a CACHED. No answer: all None -- no key (`cache.keys_for_plan`)."""
    return _build_part(_run_fingerprint_sh(["--build-env", tree], env, cwd=tree)[0])


# The fields of a `--build-env` answer: the build part of a fingerprint.
BUILD_PART = ("rustc", "cargo", "linker", "build_env", "cargo_config")

# The files rustup picks the toolchain of a tree by (rustup's names, its order).
TOOLCHAIN_FILES = ("rust-toolchain", "rust-toolchain.toml")


def _build_part(said):
    """The BUILD_PART of a `--build-env` answer; no answer: all None -- no key."""
    said = said or {}
    part = {k: said.get(k) for k in BUILD_PART}
    if not isinstance(part["build_env"], dict):
        part["build_env"] = None
    return part


def _toolchain_prelude(repo):
    """The shell lines that make a fresh directory hold the toolchain files of
    `repo` (their bytes as data, never a word of the command line) and enter it:
    on a host the tree may not be there yet, or be the last pass's -- rustup picks
    the toolchain THIS tree pins (N-K1). The directory goes when the shell ends."""
    lines = ['fp_d=$(mktemp -d) || exit 3', "trap 'rm -rf \"$fp_d\"' EXIT"]
    for name in TOOLCHAIN_FILES:
        path = os.path.join(repo, name)
        if os.path.isfile(path):
            with open(path, encoding="utf-8") as fh:
                lines.append("printf '%%s' %s >\"$fp_d/%s\"" % (shlex.quote(fh.read()), name))
    lines.append('cd "$fp_d" || exit 3')
    return "".join(line + "\n" for line in lines)


def stations_build(ln, plus_env, repo, host_tree=None):
    """The build part of the fingerprint of the stations a dispatch with `+env`
    `plus_env` starts on lane `ln` -- measured in THEIR environment, the one rule
    for the gate's run and its `--plan-only` (orchestrator OR-T12-18 A-M3): on
    this machine (`ln` None or transport `local`) `station_env` over this
    process's environment, the executor's inheritance; on a host the script runs
    THERE in the command form of the executor (`lane.exec_stream`: `env -i` of
    REMOTE_BASE, the PATH line) with `plus_env` over it, because the build
    environment is the whole environment and this side cannot know the host's
    (re-check tv-bn M7w A-M1). `plus_env` travels like the dispatch's `+env`, as
    data on stdin (`export` lines for a `bash -s`, which reads a pipe byte by byte,
    then its `exec sh -s` reads the script -- dash would read ahead): never a
    word of the command line, whose names the narrow point keeps to its lists.
    A name that is no shell name cannot be exported: no answer, no key.

    The toolchain is the tree's (re-check bundle N kit-fp M1, N-K1): here the
    script starts in `repo`, the tree the stations run in; on a host in a fresh
    directory holding `repo`'s toolchain files (`_toolchain_prelude`), the cargo
    configuration measured from the parent of `host_tree` -- the dispatch's
    `tree` there -- up. A host pass without `host_tree`: no answer, no key."""
    if ln is None or ln.transport == "local":
        return build_fingerprint(station_env(plus_env, os.environ), repo)
    from . import lane  # lazy: lane is the transport, the executor runs without it
    env = {str(k): str(v) for k, v in (plus_env or {}).items()}
    if not host_tree or any(not re.match(r"^[A-Za-z_][A-Za-z0-9_]*$", k) for k in env):
        return _build_part(None)
    try:
        prelude = _toolchain_prelude(repo)
    except (OSError, UnicodeDecodeError):
        return _build_part(None)
    prelude += "".join("export %s=%s\n" % (k, shlex.quote(v)) for k, v in sorted(env.items()))
    argv = lane._remote_argv(ln, "%s && exec bash -s" % lane.REMOTE_PATH)
    # not `exec`: the shell removes the directory after the script (its trap); bash
    # has read this whole line before `sh -s` reads the script from the same stdin
    return _build_part(_run_fingerprint_sh([], {}, argv=argv, prelude=prelude + (
        "sh -s -- --build-env %s; exit $?\n" % shlex.quote(host_tree)))[0])


def lane_fingerprint(tree, target, env):
    """V0 section 2.3: `lane_fingerprint.sh --tree <tree> <target>`,
    the one producer of a fingerprint (re-check tv-bn M7q M1, OR-T11-22).

    `lanes fingerprint` writes the same script's answer into the register; the
    cache keys (`cache._tools`), the cache doubt (`gate._cache_doubt`) and the
    drift NOTE (`fingerprint_drift`) compare the two. A second producer here gave
    a second form of the same facts (`3.12.3` beside `Python 3.12.3`, `ext2/ext3`
    beside `ext4`): after every `lanes fingerprint` the next run missed every cache
    entry, doubted its pass and noted a drift. `linker`, `build_env` and
    `incremental_policy` come from `env`, the stations' environment (M7r A-M2,
    M7t A-M3: the policy was the register's echo, not the run's CARGO_INCREMENTAL). The browsers are `browser_probe` (the same script with
    `--browsers`, under the station environment and its bound).

    A script that does not answer -- or misses a field -- is NO fingerprint (M7r
    A-M1, fail-closed): every field None and `unanswered` says why; no cache key is
    made from it, no entry recorded under it, and the register keeps what it had."""
    said, why = _run_fingerprint_sh(["--tree", tree, target], env, cwd=tree)
    missing = [k for k in FINGERPRINT_FIELDS if k != "browsers" and k not in (said or {})]
    if why is None and missing:
        why = "no field %s" % ",".join(missing)
    fp = {k: (None if why else said.get(k)) for k in FINGERPRINT_FIELDS}
    fp["browsers"] = browser_probe(tree, env)
    if why:
        fp["unanswered"] = why
    return fp


# The engines a station books (`browser_<engine>`, `pipeline/plan.py` NEEDS) as
# Playwright keeps them: its `browsers.json` name, the bundle directory prefix, the
# binaries a headless launch starts, and the library directories its own wrapper
# puts on LD_LIBRARY_PATH (`minibrowser-wpe/MiniBrowser`; the headless shell sets none).
BROWSER_BUNDLES = {
    "chromium": ("chromium-headless-shell", "chromium_headless_shell",
                 ("chrome-headless-shell-linux64/chrome-headless-shell",), ()),
    "webkit": ("webkit", "webkit",
               tuple("minibrowser-wpe/bin/%s" % b for b in (
                   "MiniBrowser", "WPEWebProcess", "WPENetworkProcess", "WPEGPUProcess")),
               ("minibrowser-wpe/lib", "minibrowser-wpe/sys/lib")),
}

# The switch a station that books an engine runs its proofs with: a driver whose
# engine does not start is then a failing run, not a SKIP (GH #1161).
REQUIRE_BROWSERS_ENV = "MECLAW_REQUIRE_BROWSERS"


# The lane's own probe (`--browsers`): one script answers for the scheduler's
# fingerprint over ssh and for the executor here (GH #1161).
LANE_FINGERPRINT_SH = os.path.join(os.path.dirname(os.path.abspath(__file__)),
                                   "lane_fingerprint.sh")


def browser_probe(tree, env):
    """Per engine whether it can start on this lane: "ok", or why not in plain words.

    GH #1161, OR-TB.S3.17: build02's WebKit bundle could not load libGL.so.1, every
    WebKit proof SKIPped and `browser:display` read GREEN; then (10.10.) stale
    laboratory copies in its `sys/lib` shadowed the system Mesa, `ldd` resolved
    every binary, the field said "ok", and every WebKit start died at newPage with
    "Could not create EGL display". The probe is `lane_fingerprint.sh --browsers`:
    the pinned revision's bundle, `ldd` under the bundle's own library path, then a
    real start -- launch, newPage, setContent, evaluate -- under the tree's wkenv.sh
    and MECLAW_REQUIRE_BROWSERS=1 in this run's environment, bounded by
    MECLAW_BROWSER_PROBE_SECS. Anything it cannot check is a reason, never "ok"."""
    run_env = dict(env)
    run_env[REQUIRE_BROWSERS_ENV] = "1"
    try:
        secs = int(run_env.get("MECLAW_BROWSER_PROBE_SECS") or 30)
    except ValueError:
        secs = 30
    sh = shutil.which("sh") or "/bin/sh"
    try:
        r = subprocess.run([sh, LANE_FINGERPRINT_SH, "--browsers", tree], capture_output=True,
                           text=True, timeout=len(BROWSER_BUNDLES) * (secs + 10) + 30,
                           env=run_env, stdin=subprocess.DEVNULL)
        said = json.loads(r.stdout)
    except (OSError, subprocess.SubprocessError, ValueError) as exc:
        why = "the browser probe gave no answer: %s" % str(exc).splitlines()[0] if str(exc) \
            else "the browser probe gave no answer"
        return {e: why for e in BROWSER_BUNDLES}
    if not isinstance(said, dict):
        return {e: "the browser probe gave no answer" for e in BROWSER_BUNDLES}
    return {e: str(said.get(e) or "not probed") for e in BROWSER_BUNDLES}


def _booked_engines(st):
    """The engines a station books, `chromium`/`webkit`, in BROWSER_BUNDLES order."""
    needs = _needs(st)
    return [e for e in BROWSER_BUNDLES if needs.get("browser_" + e, 0) > 0]


# --- the scope ----------------------------------------------------------------------
def _default_scope_stopper(run):
    """`systemctl --user stop pipe-<run>.scope` -- only when this process runs in it."""
    unit = "pipe-%s.scope" % run
    try:
        with open("/proc/self/cgroup", encoding="utf-8") as fh:
            if unit not in fh.read():
                return
    except OSError:
        return
    # --no-block: the executor is inside the scope it stops, and a foreign
    # stop may already be running; waiting for that job would wait for itself.
    subprocess.run(["systemctl", "--user", "stop", "--no-block", unit], timeout=60,
                   stdin=subprocess.DEVNULL, capture_output=True)


# --- the run --------------------------------------------------------------------------

def isolation_issues(d):
    """`{test: issue}` an isolation names: the register's issues
    (`flake_issues`, tb-s10) under the quarantine's, which win."""
    out = {}
    for t, i in (d.get("flake_issues") or {}).items():
        if t and str(i).isdigit():
            out[t] = int(i)
    for e in d.get("quarantine") or []:
        if isinstance(e, dict) and e.get("test") and e.get("issue") is not None:
            out[e["test"]] = e["issue"]
    return out

# GH #1178 L-1: CPython 3.12 has no getter for the `warn_on_full_buffer` of a
# wakeup fd, so restoring a previous fd would lose its flag. The kit keeps
# books instead: every fd `_sigterm_wakeup` installs is recorded here with the
# flag it was set with, and dropped again by its undo. Only the main thread
# sets wakeup fds (Python's rule), so no lock is needed.
_KIT_WAKEUP_FLAGS = {}
# The documented CPython default of `signal.set_wakeup_fd(fd)`. Used, and only
# used, for a previous fd the kit did not set: its real flag is unknowable, so
# the restore spells out the default explicitly instead of guessing.
_CPYTHON_DEFAULT_WARN_ON_FULL_BUFFER = True


def _restore_flag(prev_fd):
    """The `warn_on_full_buffer` to restore `prev_fd` with: the kit's own flag
    if the kit set that fd, else the documented CPython default (fail closed:
    a foreign setter's flag is unknown, never guessed)."""
    return _KIT_WAKEUP_FLAGS.get(prev_fd, _CPYTHON_DEFAULT_WARN_ON_FULL_BUFFER)


class _Run:
    # The default clock also for a `_Run` built without `__init__` (tests that
    # call `_Run.__new__` and fill only the fields they need).
    clock = staticmethod(time.monotonic)

    def __init__(self, dispatch, out, err, scope_stopper, clock=time.monotonic):
        d = self.d = dispatch
        # The run's one monotonic clock (TV-K1): station seconds, deadlines and
        # the lock wait read it; self-tests hand in a fake one.
        self.clock = clock
        self.out, self.err = out, err
        self.scope_stopper = scope_stopper
        self.run_id = d["run"]
        self.mode = d.get("mode") or "strand"
        self.tree = d["tree"]
        self.target = d["target"]
        self.ws = d["ws"]
        self.logs_dir = os.path.join(self.ws, "logs")
        self.reuse_dir = os.path.join(self.ws, "reuse")
        from . import lane   # the one source of the own lane name (re-check tv-bn M7q)
        self.runner = lane.own_name(d.get("runner") or d.get("lane"))
        self.lock_file = d.get("+lock_file") or ""
        self.export_root = d.get("+export_root") or ""
        self.stations = list(d.get("stations") or [])
        self.by_name = {s["name"]: s for s in self.stations}
        self.names = set(self.by_name)
        cap = dict(d.get("capacity") or {})
        self.cap = {k: float(cap.get(k, 0) or 0) for k in DIMS}
        self.cap["cpu"] *= float(d.get("+cpu_overbook") or 1.0)
        self.used = {k: 0.0 for k in DIMS}
        # The colony slots (GH #1161): a station gets the lowest free slots it booked,
        # never one a running station holds; a booking the register cannot serve is
        # RED before the station starts (`_red_before_start`).
        self.slot_free = list(range(min(int(self.cap["colony"]), ports.SLOT_COUNT)))
        self.slots = {}
        self.state = {n: "pending" for n in self.by_name}
        self.est = {s["name"]: _estimate(s) for s in self.stations}
        self.build_docs = []   # nextest list documents of every build station of the run
        # The build stations (`build`, phase 1 `build:browser`, GH #1162 Weg d).
        self.builds = {s["name"] for s in self.stations if s.get("build") is not None} | {"build"}
        self.started_at, self.held_since = {}, {}
        self.held_secs = {}   # the closed stretches a reservation kept a station back
        self.verdicts, self.secs = {}, {}
        self.red_ended = set()   # the stations that count red: RED and a non-legacy FLAKY
        self.precheck_red = False
        self.precheck_stopped = set()
        self.fail_fast_hit = False
        self.aborted = None
        # Without a lock file no station needs cargo (`run` refuses that
        # dispatch), and no child is told the lock is held ("absent").
        self.lock_state = "none" if self.lock_file else "absent"
        self.lock_fd = None
        self.lock_wait_secs = 0
        self.procs = {}
        self.threads = []
        self.cond = threading.Condition()
        self.emit_lock = threading.Lock()
        self.closed = False
        self.events = []
        self.cache_hits = 0
        self.stop_event = threading.Event()
        self.t_start = self.clock()
        self.base_env = station_env(d.get("+env"), os.environ)  # pd-1155 kit find, M7t A-M2
        # The cache: whole stations and parts the kern decided before the dispatch.
        self.cached_whole, self.cached_parts, self.cached_any = {}, {}, set()
        self.cut_argvs = {}
        for ck in d.get("cache_known") or []:
            st, part = ck.get("station"), ck.get("part") or ""
            if part:
                self.cached_parts.setdefault(st, {})[part] = ck
                self.cached_any.add(part)
            else:
                self.cached_whole[st] = ck
        for name, ck in self.cached_whole.items():
            for part in (self.by_name.get(name) or {}).get("parts") or []:
                self.cached_any.add(part)
        gate_dir = os.path.join(self.target, "gate", os.path.basename(self.tree.rstrip("/")))
        self.legacy_path = os.path.join(gate_dir, "%s-%s.json" % (self.mode, d.get("rev") or "unknown"))
        self.legacy_last = os.path.join(gate_dir, "last-%s.json" % self.mode)
        self.head = {
            "mode": self.mode, "rev": d.get("rev"), "base": d.get("base"),
            "dirty": bool(d.get("dirty")), "archive": d.get("archive"),
            "repo": d.get("repo"), "lane": self.runner, "run": self.run_id,
            "plan": [s["name"] for s in self.stations],
            "started": d.get("+started_at") or store.now(),  # the journal's start (#1152)
            "owner_decisions": [], "fingerprint": {},
            "plan_reason": {s["name"]: s.get("reason") or [] for s in self.stations},
            "scopes": {s["name"]: s.get("scope", "") for s in self.stations},
            "round": d.get("round") or 1,
        }
        self._priorities()

    # --- the stream -------------------------------------------------------------------
    def emit(self, event, data, kind=None):
        data = dict(data)
        if event not in ("lane_fingerprint", "workspace_changed") and "run" not in data:
            data["run"] = self.run_id
        kind = kind or events.EVENTS[event].kinds[0]
        events.validate(event, kind, data)
        ev = events.to_line({"at": store.now(), "run": self.run_id, "kind": kind,
                             "event": event, "text": events.summary_text(event, data),
                             "data": data})
        with self.emit_lock:
            if self.closed:
                return ev
            self.events.append(ev)
            line = json.dumps(ev, sort_keys=True)
            self.out.write(line + "\n")
            self.out.flush()
            self.events_fh.write(line + "\n")
            self.events_fh.flush()
            if event == "cache_hit":
                self.cache_hits += 1
            if event in ("station_ended", "pass_ended"):
                self._write_receipts()
        return ev

    def _head_rows(self):
        """The core's `pass_planned` and `run_dispatched` of this run, for the receipt.

        The core journals both before the dispatch; the lane never streams
        them. The receipt the lane writes beside its stream is the projection
        of these two rows plus the stream, so it is the document the core
        rebuilds from the journal (`receipt.build`)."""
        h, d = self.head, self.d
        planned = {"run": self.run_id, "mode": h["mode"], "repo": h["repo"], "rev": h["rev"],
                   "base": h["base"], "stations": h["plan"], "reason": h["plan_reason"],
                   "cache_known": len({ck.get("station") for ck in d.get("cache_known") or []}),
                   "dirty": h["dirty"], "archive": h["archive"],
                   "lock_wait_secs": self.lock_wait_secs}
        dispatched = {"run": self.run_id, "runner": self.runner, "tree": self.tree,
                      "attempt": d.get("attempt") or 1, "round": h["round"],
                      "deadline_at": d.get("deadline_at"), "ws": self.ws}
        return [{"at": h["started"], "run": self.run_id, "kind": "status",
                 "event": "pass_planned", "data": planned},
                {"at": d.get("+dispatched_at") or h["started"], "run": self.run_id, "kind": "note",
                 "event": "run_dispatched", "data": dispatched}]

    def _write_receipts(self):
        v2 = receipt.project(self._head_rows() + self.events)
        _write_json_atomic(os.path.join(self.ws, "receipt.json"), v2)
        v1 = legacy_v1(v2)
        _write_json_atomic(self.legacy_path, v1)
        _write_json_atomic(self.legacy_last, v1)

    def ended(self, name, verdict, secs=0, reason="", exit_code=None, scope=None, **extra):
        self.emit("station_ended", self._ended_data(name, verdict, secs, reason, exit_code,
                                                    scope, **extra))

    def _ended_data(self, name, verdict, secs=0, reason="", exit_code=None, scope=None, **extra):
        data = {"station": name, "verdict": verdict, "secs": round(float(secs), 3),
                "exit": exit_code,
                "scope": scope if scope is not None else self.by_name.get(name, {}).get("scope", "")}
        if reason:
            data["reason"] = reason
        if verdict == "SKIP" and reason.startswith(COUNTED_SKIP_PREFIXES):
            data["in_total"] = True
        data.update({k: v for k, v in extra.items() if v is not None})
        return data

    def note_row(self, name, scope, secs, reason=""):
        """A NOTE row that is not a station of the plan (lock-wait, tree-sync, ...)."""
        self.ended(name, "NOTE", secs, reason, None, scope=scope)

    # --- order --------------------------------------------------------------------------
    def _priorities(self):
        names = [s["name"] for s in self.stations]
        dur = {}
        for s in self.stations:
            est = _estimate(s)
            dur[s["name"]] = 30 * 60.0 if est is None else est
        succ = {n: [] for n in names}
        for s in self.stations:
            for a in _after(s, self.names):
                succ[a].append(s["name"])
        rank, es = {}, {}

        def rank_of(n, seen=()):
            if n in rank:
                return rank[n]
            if n in seen:
                return dur[n]
            rank[n] = dur[n] + max([rank_of(m, seen + (n,)) for m in succ[n]] or [0])
            return rank[n]

        def es_of(n, seen=()):
            if n in es:
                return es[n]
            if n in seen:
                return 0.0
            preds = _after(self.by_name[n], self.names)
            es[n] = max([es_of(p, seen + (n,)) + dur[p] for p in preds] or [0.0])
            return es[n]
        for n in names:
            rank_of(n)
            es_of(n)
        cp = max([rank[n] + es[n] for n in names] or [0])
        self.prio = {n: (-rank[n], cp - es[n] - rank[n], i) for i, n in enumerate(names)}

    def _fits(self, needs):
        for dim in DIMS:
            n = needs[dim]
            if n <= 0:
                continue
            cap = self.cap[dim]
            if dim == "colony":
                # Re-check tb-s3 n1 M-neu-1: a lane's colonies (`cores=`, 12 on
                # build04) can exceed the register's slots (8); a third colony
                # station admitted by the colonies found no slot and was RED.
                cap = min(cap, ports.SLOT_COUNT)
            if self.used[dim] + n <= cap + 1e-9:
                continue
            if self.used[dim] == 0:      # bigger than the lane: it runs alone
                continue
            return False
        return True

    # --- the decisions before a start ------------------------------------------------
    def _skip_reason(self, st):
        """Why a ready station does not run (SKIP), or None. Order as in gate.sh."""
        name = st["name"]
        if self.aborted:
            return "aborted"
        if self.fail_fast_hit:
            return "fail-fast"
        only = self.d.get("+only")
        if only and name not in only:
            return "not in --only"
        if st.get("run") is False:
            return {"tests": "planned-for-shards",
                    "slow-tests": "no-tests-station-in-ci"}.get(name, "not run in %s" % self.mode)
        if self.precheck_red and self._precheck_stops(st):
            return "precheck red"
        preds = [p for p in _after(st, self.names) if p != "precheck"]
        outcome = (st.get("when") or {}).get("outcome") or "success"
        # A FLAKY off the legacy list counts red here too (TV-F1, re-check tv-p1c).
        bad = [p for p in preds if p in self.red_ended or self.verdicts.get(p) == "ABORTED"]
        if outcome == "success" and bad:
            return "%s %s" % (bad[0], self.verdicts[bad[0]].lower())
        if outcome == "failure" and not bad:
            return "no failure before it"
        tool = MISSING_TOOL.get(name)
        if tool and not _which(tool, self.base_env):
            return "%s not installed" % tool
        why = self._browser_short(name)
        if why:
            return why
        if st.get("build") is not None and not self._build_argvs(st):
            return "all cached"
        return None

    def _precheck_stops(self, st):
        """Whether a red precheck stops `st`: the cargo stations, the build and
        what runs after it, and PRECHECK_STOP_ALSO (`--no-precheck-stop` none)."""
        name = st["name"]
        if self.d.get("+no_precheck_stop") or name == "precheck":
            return False
        return bool(_needs_cargo(st) or name in self.builds
                    or any(a in self.builds for a in st.get("after") or [])
                    or name in PRECHECK_STOP_ALSO)

    def _red_before_start(self, st):
        """The lane refusals gate.sh grades RED before a station runs."""
        name = st["name"]
        booked = int(_needs(st)["colony"])
        if booked < 0:
            return ("colony booking unresolved (%d): the lane's colonies were not known "
                    "when the run was dispatched -- refused, it would run on no slots" % booked)
        if booked > 0 and self.slots.get(name) is None:
            # Re-check tb-s3 h1 M-1: two numbers, each by its name -- the lane's
            # colonies (`schedule.colony_capacity`, its `cores=`) and the slots of
            # the port register; the old text called the lane's 2 "the port register".
            return ("colony slots: booked %d, the lane offers %d colonies (cores= in "
                    "the host file, schedule.colony_capacity) and the port register has "
                    "%d slots (scripts/pipeline/ports.py) -- more would share ports"
                    % (booked, int(self.cap["colony"]), ports.SLOT_COUNT))
        said = ((getattr(self, "head", None) or {}).get("fingerprint") or {}).get("browsers") or {}
        for engine in _booked_engines(st):
            if said.get(engine) != "ok":
                return ("browser_%s booked, but %s does not start on this lane: %s"
                        % (engine, engine, said.get(engine) or "not probed"))
        if name == "display-lab" and not _which("node", self.base_env):
            return "no-node (lane): install node on the build host, the runline tests call it"
        for cmd in st.get("cmds") or []:
            argv = cmd.get("argv") or []
            for i, a in enumerate(argv):
                val = None
                if a == "--with" and i + 1 < len(argv):
                    val = argv[i + 1]
                elif a.startswith("--with="):
                    val = a.split("=", 1)[1]
                if val is not None and val.strip("'\"") not in ("none", ""):
                    return "--with %s on a lane: a lane runs the keyless class none only" % val.strip("'\"")
        return None

    def _browser_short(self, name):
        if not name.startswith("browser:"):
            return None
        if _booked_engines(self.by_name.get(name) or {}):
            return None  # the probe judges it: RED before the start (OR-TB.S3.17)
        if not _which("node", self.base_env):
            return "no-node: the browser proofs did not run (counted)"
        if not os.path.isdir(os.path.join(self.tree, "workshop", "tools", "node_modules",
                                          "playwright")):
            return ("no-playwright: workshop/tools/node_modules is missing, "
                    "the browser proofs did not run (counted)")
        return None

    def _cached_station(self, st):
        """The cache entries that make a whole station CACHED, or None."""
        name = st["name"]
        if name in self.cached_whole:
            return [self.cached_whole[name]]
        parts = st.get("parts") or []
        mine = self.cached_parts.get(name) or {}
        if st.get("nextest") is not None and parts and all(p in mine for p in parts):
            return [mine[p] for p in parts]
        return None

    def _build_argvs(self, st):
        build = dict(st.get("build") or {})
        if build.get("meclaw_bin"):
            users = [s for s in self.stations if "artefact:meclaw" in (s.get("inputs") or [])]
            if users and all(s["name"] in self.cached_whole or self._cached_station(s)
                             for s in users):
                build["meclaw_bin"] = False
        all_parts = {p for s in self.stations for p in (s.get("parts") or [])}
        # A part is left out only when EVERY station that carries it takes it
        # from the cache (re-check H2 B1 c): cached in one station and run in
        # another, it must be built.
        everywhere = {p for p in self.cached_any
                      if all(s["name"] in self.cached_whole
                             or p in (self.cached_parts.get(s["name"]) or {})
                             for s in self.stations if p in (s.get("parts") or []))}
        return build_argvs(build, everywhere, all_parts)

    # --- the scheduler ---------------------------------------------------------------
    def _moves_reservations(self, blocked, needs, est, now):
        """Whether a station of `needs` started now for `est` seconds moves the
        reserved start of any station in `blocked` ([(name, passes)], rank order;
        GH #1162 B-S4-3). A predecessor that runs ends at its estimate, one that
        reserves ends after its reservation, any other as if it started now; a
        station that will be skipped (`passes`) takes no room and no time.
        Unknown counts as moving."""
        if not blocked:
            return False
        if est is None:
            return True
        ends, ivals = {}, []
        for n, s in self.state.items():
            if s == "running":
                e = self.est.get(n)
                ends[n] = None if e is None else max(now, self.started_at[n] + e)
                ivals.append((_needs(self.by_name[n]), self.started_at[n], ends[n]))
        pos, rows = {}, []
        for b, passes in blocked:
            ready_at, after = now, []
            for p in _after(self.by_name[b], self.names):
                if self.state[p] == "done":
                    continue
                if p in pos:
                    after.append(pos[p])
                    continue
                if p in ends:
                    end = ends[p]
                else:
                    e = self.est.get(p)
                    end = None if e is None else now + e
                ready_at = None if ready_at is None or end is None else max(ready_at, end)
            pos[b] = len(rows)
            rows.append(({}, ready_at, 0.0, after) if passes
                        else (_needs(self.by_name[b]), ready_at, self.est.get(b), after))
        before = reserve(rows, ivals, self.cap)
        moved = reserve(rows, ivals + [(needs, now, now + est)], self.cap)
        return (before is None or moved is None
                or any(a > b + 1e-6 for a, b in zip(moved, before)))

    def _held_for(self, name, now):
        """Seconds a reservation kept `name` back so far (RESERVE_AGE_S counts these)."""
        since = self.held_since.get(name)
        return self.held_secs.get(name, 0.0) + (now - since if since is not None else 0.0)

    def _age_wait(self):
        """Seconds until the next kept-back station reaches RESERVE_AGE_S, or None."""
        now = self.clock()
        due = [RESERVE_AGE_S - self._held_for(n, now) for n in self.held_since
               if self.state[n] == "pending"]
        return max(0.0, min(due)) if due else None

    def _start_ready(self):
        progressed = False
        now = self.clock()
        cands = [n for n, s in self.state.items() if s == "pending"]
        aged = {n for n in cands if self._held_for(n, now) >= RESERVE_AGE_S}
        cands.sort(key=lambda n: (n not in aged, self.prio[n]))
        blocked, holding = [], set()   # the reservations, in order; kept back this pass
        for name in cands:
            st = self.by_name[name]
            needs = _needs(st)
            if any(self.state[p] != "done" for p in _after(st, self.names)):
                blocked.append((name, self._skip_reason(st) is not None))
                continue
            why = self._skip_reason(st)
            if why == "aborted":
                # Never started after an abort: ABORTED, not SKIP -- the store
                # books SKIP as `done` and the pass would read as fail-open.
                self._finish(name, "ABORTED", 0)
                self.ended(name, "ABORTED", 0, _abort_reason(self.aborted))
                progressed = True
                continue
            if why:
                self._finish(name, "SKIP", 0)
                self.ended(name, "SKIP", 0, why)
                progressed = True
                continue
            if not self._fits(needs):
                blocked.append((name, False))
                continue
            if (name not in aged
                    and self._moves_reservations(blocked, needs, self.est.get(name), now)):
                self.held_since.setdefault(name, now)
                holding.add(name)
                blocked.append((name, False))
                continue
            if needs["cargo"] > 0 and self.lock_state != "held":
                if self.lock_state == "none":
                    self.lock_state = "waiting"
                    threading.Thread(target=self._take_lock, daemon=True).start()
                # It starts the moment the lock is held: its reservation stands from
                # now (the lock thread waits for this pass; V6 takes the lock at once).
                blocked.append((name, False))
                continue
            self.state[name] = "running"
            self.started_at[name] = now
            self.held_since.pop(name, None)
            self.held_secs.pop(name, None)
            for k in DIMS:
                self.used[k] += needs[k]
            self._take_slots(name, needs)
            t = threading.Thread(target=self._station, args=(st,), daemon=True)
            self.threads.append(t)
            t.start()
            return True  # a start changes the candidates and the reservations: next pass
        # A whole pass without a start: a stretch ends where no reservation keeps the
        # station back any more (it does not fit, or a predecessor runs again).
        for name in [n for n in self.held_since if n not in holding]:
            self.held_secs[name] = self._held_for(name, now)
            del self.held_since[name]
        return progressed

    def _take_slots(self, name, needs):
        """Book the station's colony slots (caller holds `cond`); None = cannot serve."""
        want = int(needs.get("colony") or 0)
        if want <= 0:
            return
        if want > len(self.slot_free):
            self.slots[name] = None
            return
        self.slots[name], self.slot_free = self.slot_free[:want], self.slot_free[want:]

    def _give_slots(self, name):
        got = self.slots.pop(name, None)
        if got:
            self.slot_free = sorted(self.slot_free + got)

    def _finish(self, name, verdict, secs, red=None):
        """Book a station as ended (caller holds `cond`). `red`: the station counts
        red (`ends_red` of its `station_ended`); None = `verdict == "RED"`."""
        self.state[name] = "done"
        self.verdicts[name] = verdict
        if verdict == "RED" if red is None else red:
            self.red_ended.add(name)
        self.secs[name] = float(secs)
        if name == "precheck" and verdict == "RED":
            self.precheck_red = True
            # GH #1162: `build` (and the release export build) start beside the
            # precheck; a red precheck ends them as it would have kept them from
            # starting -- SKIP `precheck red`, their processes killed.
            stopped = [n for n, s in self.state.items()
                       if s == "running" and self._precheck_stops(self.by_name[n])]
            self.precheck_stopped.update(stopped)
            self._kill([p for (n, _pid), p in self.procs.items() if n in stopped])
        if name in self.red_ended and self.d.get("+fail_fast"):
            self.fail_fast_hit = True

    def schedule(self):
        # Whole-station cache hits first: they never start, and their
        # successors see them as green.
        with self.cond:
            for st in self.stations:
                hits = self._cached_station(st)
                if not hits:
                    continue
                for ck in hits:
                    self.emit("cache_hit", {"station": st["name"], "part": ck.get("part") or "",
                                            "key": ck.get("key"), "from_run": ck.get("from_run")})
                self._finish(st["name"], "CACHED", 0)
                self.ended(st["name"], "CACHED", 0)
        with self.cond:
            while True:
                while self._start_ready():
                    pass
                pending = [n for n, s in self.state.items() if s != "done"]
                if not pending:
                    break
                running = [n for n, s in self.state.items() if s == "running"]
                # GH #1162 hA-m-1: a station a reservation keeps back starts once it
                # reaches RESERVE_AGE_S -- no dead edge, even when nothing runs.
                held = [n for n in self.held_since if self.state[n] == "pending"]
                if not running and self.lock_state != "waiting" and not held:
                    # Nothing runs, nothing can start: a cycle or a dead edge.
                    for n in pending:
                        self._finish(n, "SKIP", 0)
                        self.ended(n, "SKIP", 0, "unreachable")
                    continue
                self.cond.wait(self._age_wait())

    # --- the cargo lock -----------------------------------------------------------------
    def _stage_lock_note(self):
        path = self.base_env.get("MECLAW_STAGE_LOCK") or ""
        if not path or not os.path.exists(path):
            return
        try:
            fd = os.open(path, os.O_RDONLY)
        except OSError:
            return
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
            fcntl.flock(fd, fcntl.LOCK_UN)
        except OSError:
            self.note_row("stage-lock", os.path.basename(path), 0,
                          "twin measurement running; timing tests may flake")
        finally:
            os.close(fd)

    def _take_lock(self):
        self._stage_lock_note()
        wait = 0
        try:
            os.makedirs(os.path.dirname(self.lock_file) or ".", exist_ok=True)
            fd = os.open(self.lock_file, os.O_WRONLY | os.O_CREAT, 0o644)
            try:
                # A lock nobody holds is no wait (GH #764: a loaded host booked
                # the time around the take as a phantom second); only a held
                # lock is a queue, and only that one is measured.
                fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                t0 = self.clock()
                fcntl.flock(fd, fcntl.LOCK_EX)
                wait = int(self.clock() - t0)
        except OSError as exc:
            print("exec: cannot take the cargo lock %s: %s" % (self.lock_file, exc), file=self.err)
            self.abort("process")
            return
        with self.cond:
            if self.closed or self.aborted:
                os.close(fd)
                return
            self.lock_fd = fd
        if wait >= 1:
            self.lock_wait_secs = wait
            self.note_row("lock-wait", "%ds behind other runs" % wait, wait)
        note = self._tree_sync()
        if note:
            self.note_row("tree-sync", note, 0)
        with self.cond:
            self.lock_state = "held"
            self.cond.notify_all()

    def _tree_sync(self):
        """`meclaw_tree_sync` + `meclaw_write_stamp` of scripts/cargo-target.sh, under the lock."""
        lib = os.path.join(self.tree, "scripts", "cargo-target.sh")
        if self.mode == "ci" or not os.path.isfile(lib):
            return ""
        script = ('. "$1/scripts/cargo-target.sh" || exit 2; d=$(meclaw_dirty_files "$1"); '
                  'meclaw_tree_sync "$1" "$2" "$3" "$d" "$4"; '
                  'meclaw_write_stamp "$1" "$2" "$3" "$d"')
        try:
            r = subprocess.run(["bash", "-c", script, "tree-sync", self.tree, self.target,
                                self.d.get("rev") or "", "1" if self.d.get("+resync") else "0"],
                               capture_output=True, text=True, timeout=900,
                               env=self.base_env, cwd=self.tree, stdin=subprocess.DEVNULL)
        except (OSError, subprocess.SubprocessError) as exc:
            return "tree sync failed: %s" % exc
        return r.stdout.strip().splitlines()[-1] if r.stdout.strip() else ""

    # --- one station ---------------------------------------------------------------------
    def _env(self, st, extra=None):
        env = dict(self.base_env)
        env["CARGO_TARGET_DIR"] = self.target
        if self.lock_file:
            env["MECLAW_GATE_LOCK"] = self.lock_file
        if self.lock_state == "held":
            env["MECLAW_CARGO_LOCK_HELD"] = "1"
        name = st["name"]
        if self.export_root and name == "export-tree-tests":
            os.makedirs(self.export_root, exist_ok=True)
            env["MECLAW_EXPORT_WORKDIR"] = self.export_root
        if self.export_root and name == "export-audit":
            check = os.path.join(self.export_root, "check")
            os.makedirs(check, exist_ok=True)
            env["MECLAW_EXPORT_CHECK_TARGET"] = check
        _book_threads(env, st, egon=egon_capped(getattr(self, "d", None)))
        if name == "gate-selftest":
            _book_selftest_shards(env, st)
        # Only a station that booked colonies gets slots; a value left in the lane's
        # environment never reaches another (it would share the booked ones).
        slots = self.slots.get(name) if hasattr(self, "slots") else None
        if slots:
            env[COLONY_SLOTS_ENV] = ",".join(str(n) for n in slots)
        else:
            env.pop(COLONY_SLOTS_ENV, None)
        # A station that books an engine runs its proofs with the switch; any other
        # leaves a value of the lane's environment behind.
        if _booked_engines(st):
            env[REQUIRE_BROWSERS_ENV] = "1"
        else:
            env.pop(REQUIRE_BROWSERS_ENV, None)
        env.update(extra or {})
        return env

    def _wrap(self, st, argv, remaining_s, env):
        pre = []
        tmo = _which("timeout", env)
        if tmo:
            # A backstop only: the cap itself is `_spawn`'s wait, which takes the
            # hang diagnosis before it kills (TV-K5). It fires when the executor
            # cannot (gone, stuck).
            pre = [tmo, "-k", "30", "%.3fs" % (max(remaining_s, 0.001) + HANG_BACKSTOP_S)]
        # Every cargo station, the build, and the nextest stations (`tests` and
        # `slow-tests` had `cargo:1` under gate.sh and ran under nice/ionice there).
        if (self.d.get("+nice") or egon_machine()) and (_needs_cargo(st) or st.get("build") is not None
                                    or st.get("nextest") is not None):
            pre += ["nice", "-n", "19"]
            if _which("ionice", env):
                pre += ["ionice", "-c3"]
        return pre + argv

    def _spawn(self, st, argv, cwd, env, log, t0, capture=None):
        """Run one command into the station log; returns (rc, timed_out)."""
        limit = float(st.get("timeout_min") or 30) * 60
        remaining = limit - (self.clock() - t0)
        if remaining <= 0:
            return 124, True
        log.write("$ %s\n" % shlex.join(argv))
        log.flush()
        with self.cond:
            if self.aborted or st["name"] in self.precheck_stopped:
                return 143, False
        if remaining > hang_dump.LEAD_S:
            # Python under the station dumps its stacks before the cap (TV-Z5).
            # The dump's instant crosses the process boundary, so it is on the
            # real monotonic clock (TV-Z9: an injected clock does not travel).
            env = dict(env, **{hang_dump.ENV: hang_dump.at(time.monotonic() + remaining)})
        try:
            proc = subprocess.Popen(self._wrap(st, argv, remaining, env), cwd=cwd, env=env,
                                    stdin=subprocess.DEVNULL,
                                    stdout=capture if capture is not None else log,
                                    stderr=log, start_new_session=True)
        except OSError as exc:
            log.write("exec: %s\n" % exc)
            return 127, False
        with self.cond:
            self.procs[(st["name"], proc.pid)] = proc
            late = self.aborted or st["name"] in self.precheck_stopped
        if late:
            # The abort came between the check above and this register: its
            # snapshot of `procs` missed this process, so it goes here.
            self._kill([proc])
        try:
            rc = proc.wait(timeout=remaining)
        except subprocess.TimeoutExpired:
            # The cap is a hang marker (TV-K5): the diagnosis first, while the
            # hang still stands, then the kill. 124 is `timeout`'s own code.
            hang_diagnosis(st, log, proc.pid)
            self._kill([proc])
            proc.wait()
            with self.cond:
                self.procs.pop((st["name"], proc.pid), None)
            return 124, True
        with self.cond:
            self.procs.pop((st["name"], proc.pid), None)
        elapsed = self.clock() - t0
        timed_out = rc in (124, 137) and elapsed >= limit - 0.5
        return rc, timed_out

    def _cmds(self, st):
        """[(argv, cwd, extra_env)] of a station, `{receipt}` filled in."""
        out = []
        nt = st.get("nextest")
        profile = (nt or {}).get("profile") or "default"
        excluded = []
        want = []
        if nt is not None:
            mine = self.cached_parts.get(st["name"]) or {}
            want = [p for p in st.get("parts") or [] if p in mine]
        cmds = list(st.get("cmds") or [])
        parts = _is_parts(st, cmds)
        cut = []
        if want:
            # The cache cut goes into EVERY `test-tier.sh filter` command, each
            # with its own filter and the rest of its argv kept (GH #1161 and V1
            # of the re-check: `--run-ignored all` of the browser locks was lost).
            # No such command: nothing is cut, the station runs whole.
            not_ = " | ".join("binary_id(=%s)" % p for p in want)
            new_cmds = []
            for c in cmds:
                argv = list(c.get("argv") or [])
                if _tier_filter(argv):
                    argv = _tier_filter_argv(argv, not_)
                    c = dict(c, argv=argv)
                    cut.append(argv)
                new_cmds.append(c)
            if cut:
                cmds, excluded = new_cmds, want
        reuse = os.path.isfile(os.path.join(self.reuse_dir, "binaries.json"))
        profiles = list((st.get("parallel") or {}).get("profiles") or []) if parts else []
        for i, c in enumerate(cmds):
            argv = [self.legacy_path if a == "{receipt}" else a for a in c.get("argv") or []]
            if not argv:
                continue
            cwd = c.get("cwd") or ""
            cwd = cwd if os.path.isabs(cwd) else os.path.join(self.tree, cwd)
            extra = {}
            if nt is not None:
                p = profile
                if i < len(profiles) and profiles[i]:
                    p = profiles[i]    # one profile per part: each its own JUnit file
                elif "--run-ignored" in argv and p == "browser":
                    p = "browser-ignored"
                extra["MECLAW_TIER_PROFILE"] = p
                if reuse:
                    extra["MECLAW_TIER_REUSE"] = self.reuse_dir
                    # the gate decided where it runs (test-tier.sh's #942 probe)
                    extra["MECLAW_TIER_LANE"] = "1"
            was_cut = any(argv[:len(c)] == c for c in cut)
            if nt is not None and self.d.get("quarantine") and "test-tier.sh" in " ".join(argv[:1]):
                argv = argv + self._quarantine_argv()
                expr = self._quarantine_expr()
                if st["name"] == flakes.QUARANTINE_STATION:
                    # the quarantined tests of THIS build only (an unknown binary
                    # makes nextest refuse the whole filter)
                    argv = (_tier_filter_argv_set(argv, expr) if expr else ["true"])
                elif expr:
                    extra[flakes.TIER_EXCLUDE_ENV] = expr
            if was_cut:
                self.cut_argvs.setdefault(st["name"], []).append(argv)
            out.append((argv, cwd, extra))
        return out, excluded

    def _run_build(self, st, log, t0):
        argvs = self._build_argvs(st)
        env = self._env(st)
        rc, timed_out = 0, False
        for argv in argvs:
            # every build of the station carries the test hooks, `build --bin
            # meclaw` included: one variant of meclaw-colony in the shared
            # target/ and one `target/debug/meclaw` (GH #1208, re-check M1)
            feat = test_features(self.tree, argv)
            rc, timed_out = self._spawn(st, ["cargo"] + argv + feat, self.tree, env, log, t0)
            if rc:
                return rc, timed_out
        selections = [a[3:] for a in argvs if a[:3] == ["nextest", "run", "--no-run"]]
        if not selections:
            return 0, False
        os.makedirs(self.reuse_dir, exist_ok=True)
        docs = []
        for sel in selections:
            tmp = os.path.join(self.reuse_dir, ".list.%d.json" % len(docs))
            with open(tmp, "w", encoding="utf-8") as cap:
                rc, timed_out = self._spawn(
                    st, ["cargo", "nextest", "list", "--list-type", "binaries-only",
                         "--message-format", "json"] + sel + test_features(self.tree, sel),
                    self.tree, env, log, t0, capture=cap)
            if rc:
                return rc, timed_out
            try:
                with open(tmp, encoding="utf-8") as fh:
                    docs.append(json.loads(fh.read().strip().splitlines()[-1]))
            except (OSError, ValueError, IndexError) as exc:
                log.write("exec: unreadable nextest list output: %s\n" % exc)
                return 1, False
            os.unlink(tmp)
        # The documents of every build station of this run (GH #1162 Weg d): a
        # browser command that starts after phase 2 still finds its binaries.
        with self.cond:
            self.build_docs.extend(docs)
            merged = _merge_binaries(list(self.build_docs))
        _write_json_atomic(os.path.join(self.reuse_dir, "binaries.json"), merged)
        extra = unrequested_binaries(st.get("build") or {}, self.cached_any, selections, docs)
        if extra:
            log.write("exec: nextest list names binaries the plan did not ask for: %s\n"
                      % ", ".join(extra))
            self.note_row("%s-unrequested" % st["name"], "%d binaries" % len(extra), 0,
                          "unrequested binaries: " + ", ".join(extra))
        meta = os.path.join(self.reuse_dir, "cargo-metadata.json")
        with open(meta + ".tmp", "w", encoding="utf-8") as cap:
            rc, timed_out = self._spawn(st, ["cargo", "metadata", "--format-version", "1"],
                                        self.tree, env, log, t0, capture=cap)
        if rc:
            return rc, timed_out
        os.replace(meta + ".tmp", meta)
        return 0, False

    def _run_parts(self, cmds, st, log, t0, failed=None, results_out=None):
        """`parallel: {"by": "cmd"}`: the commands run side by side, each exactly once,
        each into its own part log at its share of the booked threads (`_part_width`);
        the part logs land in the station log in command order. The station's exit is
        the first red part's (command order). Returns (rc, timed_out, other) -- `other`
        is (rc, timed_out) of the remaining parts, which an isolation of the first red
        part must not turn green (GH #1161)."""
        name = st["name"]
        n = len(cmds)
        results = [(1, False)] * n
        paths = [os.path.join(self.logs_dir, "%s.part-%d.log" % (name, i + 1))
                 for i in range(n)]

        def one(i):
            argv, cwd, extra = cmds[i]
            with open(paths[i], "w", encoding="utf-8") as plog:
                try:
                    env = self._env(st, extra)
                    try:
                        width = _part_width(int(env.get("NEXTEST_TEST_THREADS") or 0), n)
                    except ValueError:
                        width = 0
                    if width:
                        for key in THREAD_KEYS:
                            env[key] = str(width)
                    results[i] = self._spawn(st, argv, cwd, env, plog, t0)
                except Exception as exc:  # noqa: BLE001 -- a part that cannot run is red
                    plog.write("exec: part %d failed in the executor: %s: %s\n"
                               % (i + 1, type(exc).__name__, exc))
                    results[i] = (1, False)

        threads = [threading.Thread(target=one, args=(i,), daemon=True) for i in range(n)]
        for t in threads:
            t.start()
        for t in threads:
            t.join()
        for i, (_argv, _cwd, extra) in enumerate(cmds):
            prof = extra.get("MECLAW_TIER_PROFILE")
            log.write("# part %d/%d%s: exit %d%s\n" % (
                i + 1, n, " (%s)" % prof if prof else "", results[i][0],
                " (timeout)" if results[i][1] else ""))
            try:
                with open(paths[i], encoding="utf-8", errors="replace") as fh:
                    shutil.copyfileobj(fh, log)
            except OSError as exc:
                log.write("exec: part log unreadable: %s\n" % exc)
        log.flush()
        if results_out is not None:
            results_out[:] = results
        red = [i for i in range(n) if results[i][0]]
        timed_out = any(to for _rc, to in results)
        if not red:
            return 0, timed_out, (0, False)
        first = red[0]
        if failed is not None:
            failed[:] = [cmds[first]]
        others = [results[i] for i in range(n) if i != first]
        other_rc = next((r for r, _to in others if r), 0)
        return results[first][0], timed_out, (other_rc, any(to for _r, to in others))

    def _run_cmds(self, cmds, st, log, t0, failed=None):
        rc, timed_out = 0, False
        for cmd in cmds:
            argv, cwd, extra = cmd
            rc, timed_out = self._spawn(st, argv, cwd, self._env(st, extra), log, t0)
            if rc:
                if failed is not None:
                    failed[:] = [cmd]
                break
        return rc, timed_out

    def _station(self, st):
        """One station thread: whatever fails in it, the station ends (ABORTED,
        `aborted_process`), its capacity comes back and the scheduler wakes --
        a station left `running` would hang the run without `pass_ended`."""
        name = st["name"]
        t0 = self.clock()
        done = {}
        try:
            self._station_run(st, t0, done)
        except Exception as exc:  # noqa: BLE001 -- the thread must never leave the station running
            print("exec: station %s failed in the executor: %s: %s"
                  % (name, type(exc).__name__, exc), file=self.err)
            if "verdict" not in done:
                done.update(verdict="ABORTED", secs=self.clock() - t0)
                try:
                    self.ended(name, "ABORTED", done["secs"], "aborted_process")
                except Exception as exc2:  # noqa: BLE001 -- the stream itself failed
                    print("exec: cannot end station %s: %s" % (name, exc2), file=self.err)
        finally:
            with self.cond:
                for k, v in _needs(st).items():
                    self.used[k] -= v
                self._give_slots(name)
                self._finish(name, done.get("verdict", "ABORTED"), done.get("secs", 0.0),
                             red=done.get("red"))
                self.cond.notify_all()

    def _station_run(self, st, t0, done):
        name = st["name"]
        started_wall = time.time()
        log_path = os.path.join(self.logs_dir, "%s.log" % name)
        red = self._red_before_start(st)
        cmds, excluded = ([], []) if st.get("build") is not None else self._cmds(st)
        self.emit("station_started", {"station": name, "needs": st.get("needs") or {},
                                      "cmds": len(st.get("cmds") or []) or len(cmds),
                                      "scope": st.get("scope", "")})
        for part in excluded:
            ck = self.cached_parts[name][part]
            self.emit("cache_hit", {"station": name, "part": part, "key": ck.get("key"),
                                    "from_run": ck.get("from_run")})
        scope = st.get("scope", "")
        failed_cmd = []
        other = None     # the other parts' (rc, timed_out) of a `parallel` station
        part_results = []  # every part's (rc, timed_out) of a `parallel` station
        with open(log_path, "w", encoding="utf-8") as log:
            if self.slots.get(name):
                log.write("# colony slots (booked %d): %s\n"
                          % (len(self.slots[name]), ",".join(map(str, self.slots[name]))))
            if _needs_cargo(st) and self.mode != "ci":
                env = self._env(st)
                log.write("# cargo hygiene (lane): CARGO_BUILD_JOBS=%s NEXTEST_TEST_THREADS=%s "
                          "TMPDIR=%s\n" % (env.get("CARGO_BUILD_JOBS", "<unset>"),
                                           env.get("NEXTEST_TEST_THREADS", "<unset>"),
                                           env.get("TMPDIR", "<unset>")))
            if red:
                rc, timed_out = 1, False
                log.write("exec: %s\n" % red)
            elif st.get("build") is not None:
                rc, timed_out = self._run_build(st, log, t0)
            elif _is_parts(st, cmds):
                rc, timed_out, other = self._run_parts(cmds, st, log, t0, failed_cmd,
                                                       part_results)
            else:
                rc, timed_out = self._run_cmds(cmds, st, log, t0, failed_cmd)
        secs = self.clock() - t0
        with self.cond:
            stopped = name in self.precheck_stopped
        if stopped:
            self.ended(name, "SKIP", secs, "precheck red")
            done.update(verdict="SKIP", secs=secs)
            return
        verdict, reason, decision = self._judge(st, rc, timed_out, log_path, red, failed_cmd)
        emptied = None
        if verdict == "RED" and not red and not timed_out and excluded and failed_cmd:
            emptied = self._emptied_by_cache(st, rc, log_path, excluded, failed_cmd[0], t0)
            if emptied is not None:
                hits = "%d part(s) (cached_parts)" % len(emptied) if len(emptied) > 5 else \
                    ", ".join("%s@%s" % (c["part"], c["from_run"]) for c in emptied)
                verdict, reason = "CACHED", "selection emptied by the cache: %s" % hits
                at = cmds.index(failed_cmd[0])
                if at:
                    # Re-check H2 m-N2: the commands before the emptied one RAN
                    # (green, or `_run_cmds` had stopped there): no CACHED label.
                    verdict, reason = "GREEN", "%d earlier command(s) ran green, then the " \
                        "cache cut (%s) emptied the selection" % (at, hits)
                rest = cmds[at + 1:]
                if other is not None:
                    # A `parallel` station's parts all ran, once (GH #1161): none
                    # runs again after the cut -- the WebKit part alone is 327 s of
                    # 469.6 s (integration pass 10.10.). The other parts' results
                    # decide; the first red one is judged like a red station.
                    reds = [i for i, (prc, pto) in enumerate(part_results)
                            if i != at and (prc or pto)]
                    if reds:
                        k = reds[0]
                        emptied = None
                        rc, timed_out = part_results[k]
                        failed_cmd[:] = [cmds[k]]
                        rest_r = [part_results[i] for i in reds[1:]]
                        other = (next((r for r, _t in rest_r if r), 0),
                                 any(t for _r, t in rest_r))
                        verdict, reason, decision = self._judge(st, rc, timed_out, log_path,
                                                                red, failed_cmd)
                    elif len(cmds) > 1:
                        verdict, reason = "GREEN", "after the cache cut (%s) the other " \
                            "%d part(s) ran green" % (hits, len(cmds) - 1)
                elif rest:
                    # The emptied command says nothing about the ones after it:
                    # they run now, and only their green keeps the station.
                    with open(log_path, "a", encoding="utf-8") as log:
                        log.write("\n# emptied by the cache: the remaining %d command(s)\n"
                                  % len(rest))
                        log.flush()
                        failed_cmd[:] = []
                        # Re-check tb-s7 H2 M-N3: ONE deadline per station, from
                        # its start -- a fresh clock here let a 6 s station run
                        # 8 s and end GREEN (measured on b2e3a1645).
                        rc, timed_out = self._run_cmds(rest, st, log, t0, failed_cmd)
                    secs = self.clock() - t0
                    if rc:
                        emptied = None
                        verdict, reason, decision = self._judge(st, rc, timed_out, log_path, red, failed_cmd)
                    else:
                        verdict, reason = "GREEN", "after the cache cut (%s) the rest ran" % hits
        junits = self._emit_junit(st, cmds, started_wall) if st.get("nextest") is not None else []
        if verdict == "RED":
            self._tail(name, log_path)
        if name == "precheck":
            self._precheck_notes(log_path)
        if decision:
            self.emit("owner_decision", {"station": name, "decision": "without",
                                         "reason": decision})
        self.emit("station_log", {"station": name, "path": log_path})
        extra = {}
        if st.get("build") is not None:
            extra["build_s"] = round(secs, 3)
        elif st.get("nextest") is not None:
            extra["run_s"] = round(secs, 3)
        if emptied is not None:
            extra["cached_parts"] = emptied
        data = self._ended_data(name, verdict, secs, reason, rc, scope=scope, **extra)
        result = None
        if (verdict == "RED" and not red and not timed_out and not self.aborted and failed_cmd
                and rc not in TERMINATED):
            result = self._isolate(st, failed_cmd[0], log_path, started_wall, junits)
            # The parts of a `parallel` station all ran, once: none runs again here,
            # and a red one among them keeps an isolated-green first part red.
            rest = [] if other is not None else cmds[cmds.index(failed_cmd[0]) + 1:]
            if other is not None and result is not None and result.get("verdict") == "FLAKY":
                result = dict(result, later={"ran": True, "exit": other[0],
                                             "timed_out": other[1]})
            if result is not None and result.get("verdict") == "FLAKY" and rest:
                # Delta C1: a flake in the first command says nothing about the
                # commands after it -- they run now, and only their green keeps
                # the station FLAKY (a finding, never cached: OR-TV-2).
                rest_wall = time.time()
                with open(log_path, "a", encoding="utf-8") as log:
                    log.write("\n# after the isolation: the remaining %d command(s)\n"
                              % len(rest))
                    log.flush()
                    # Re-check tb-s7 H2 M-N3: the station's own deadline (`t0`),
                    # not a fresh one after the first command and its isolation.
                    rc2, to2 = self._run_cmds(rest, st, log, t0)
                if st.get("nextest") is not None:
                    junits += self._emit_junit(st, rest, rest_wall)
                result = dict(result, later={"ran": not self.aborted, "exit": rc2,
                                             "timed_out": to2})
            # The station held its lane through the isolation and the later
            # commands: its seconds are that whole span, not the first red call
            # (GH #1161, S3-diag: `scenarios:memory` FLAKY read 3 s for 137 s).
            secs = self.clock() - t0
            data["secs"] = round(secs, 3)
            if self.aborted:
                # Delta M1: a run stopped during the isolation judges nothing --
                # no isolation rows for the register, the station ABORTED.
                result = None
                data.update(verdict="ABORTED", reason=_abort_reason(self.aborted))
        for event, body in flakes.end_station(data, result, legacy=self.d.get("flaky_legacy") or ()):
            self.emit(event, body)
            if event == "station_ended":
                verdict = body["verdict"]
                done["red"] = ends_red(body)
        done.update(verdict=verdict, secs=secs)

    @staticmethod
    def _log_has(path, text):
        try:
            with open(path, encoding="utf-8", errors="replace") as fh:
                return text in fh.read()
        except OSError:
            return False

    def _tail(self, name, path):
        try:
            with open(path, encoding="utf-8", errors="replace") as fh:
                lines = fh.read().splitlines()[-20:]
        except OSError:
            return
        for line in lines:
            print("    | %s" % line, file=self.err)

    def _precheck_notes(self, path):
        try:
            with open(path, encoding="utf-8", errors="replace") as fh:
                for line in fh:
                    if line.startswith("NOTE "):
                        print("    | %s" % line.rstrip("\n"), file=self.err)
        except OSError:
            pass

    def _decision_for(self, name):
        reason = ""
        for d in self.d.get("decide") or []:
            if isinstance(d, dict):
                if d.get("station") == name:
                    reason = d.get("reason") or ""
            elif isinstance(d, str) and "=" in d:
                st, rest = d.split("=", 1)
                if st == name:
                    reason = rest.split(":", 1)[1] if ":" in rest else rest
        return reason

    def _emptied_by_cache(self, st, rc, log_path, excluded, failed, t0):
        """GH #1167 (i): `[{part, key, from_run}]` when the nextest selection
        was empty only because the cache took its binaries out, else None.

        Fail closed, every point proven: the command that exited 6 with the
        tier's `no tests matched: ` (a refused filterset has its own line) is
        one the cache cut; something WAS subtracted; every subtracted part is a
        green hit with a key and its run, the key EQUAL to the plan's own key
        (`cache_keys`; filter and narrowing are in it, V12 -- no plan key, no
        proof); every part left in the selection is in the reused build
        (`binaries.json`), so a rest that was never built cannot look empty;
        and `nextest list` with the very same argv (`MECLAW_TIER_LIST`) lists
        no test -- run through `_spawn` (re-check H2 m-N1): registered as the
        station's process, so an abort ends it, and bounded by the station's
        own deadline (`t0`)."""
        argv, cwd, extra = failed
        if rc != NO_TESTS_EXIT or argv not in (self.cut_argvs.get(st["name"]) or []) \
                or not self._log_has(log_path, NO_TESTS_SIGNATURE):
            return None
        mine = self.cached_parts.get(st["name"]) or {}
        want = self.d.get("cache_keys") or {}
        out = []
        for part in excluded:
            ck = mine.get(part) or {}
            key, run = ck.get("key"), ck.get("from_run")
            if not key or not run:
                return None
            plan_key = want.get("%s::%s" % (st["name"], part))
            if plan_key is None or plan_key != key:
                return None
            out.append({"part": part, "key": key, "from_run": run})
        with open(log_path, "a", encoding="utf-8") as log:
            rest = [p for p in st.get("parts") or [] if p not in excluded]
            missing = self._not_built(rest)
            if missing:
                log.write("\n# cache cut: remainder not built: %s\n" % ", ".join(missing[:10]))
                return None
            env = self._env(st, dict(extra, MECLAW_TIER_LIST="1"))
            log.write("\n# cache cut: proving the remainder empty (nextest list, same argv)\n")
            log.flush()
            cap_path = log_path + ".list"
            try:
                with open(cap_path, "w+", encoding="utf-8", errors="replace") as cap:
                    lrc, timed_out = self._spawn(st, argv, cwd, env, log, t0, capture=cap)
                    cap.seek(0)
                    stdout = cap.read()
            except OSError as exc:
                log.write("# cache cut: list failed: %s\n" % exc)
                return None
            finally:
                try:
                    os.unlink(cap_path)
                except OSError:
                    pass
            log.write(stdout)
            listed = [l for l in stdout.splitlines() if l.startswith(LISTED_PREFIX)]
            if lrc != 0 or timed_out or not listed or listed[-1] != LISTED_PREFIX + "0":
                log.write("# cache cut: the remainder is not proven empty (exit %d%s, %s)\n"
                          % (lrc, ", timed out" if timed_out else "",
                             listed[-1] if listed else "no count"))
                return None
        return out

    def _not_built(self, parts):
        """The parts missing from the reused build's `binaries.json` (none
        without a reused build: then nextest builds what it lists)."""
        path = os.path.join(self.reuse_dir, "binaries.json")
        if not os.path.isfile(path):
            return []
        try:
            with open(path, encoding="utf-8") as fh:
                doc = json.load(fh)
            built = set((doc.get("rust-binaries") or {}).keys())
        except (OSError, ValueError, AttributeError):
            return list(parts) or ["<binaries.json unreadable>"]
        return [p for p in parts if p not in built]

    def _judge(self, st, rc, timed_out, log_path, red_reason, failed=None):
        """`_judge_rules`, and a RED reason names its first finding and its log (#1150).
        `failed`: `[cmd]` of the command that ended the station red, if one did."""
        verdict, reason, decision = self._judge_rules(st, rc, timed_out, log_path, red_reason,
                                                      failed)
        if verdict == "RED":
            reason = finding.red_reason(reason, log_path)
        return verdict, reason, decision

    def _judge_rules(self, st, rc, timed_out, log_path, red_reason, failed=None):
        """(verdict, reason, owner decision) -- the rules of the gate.sh station loop."""
        name = st["name"]
        if name == flakes.QUARANTINE_STATION and not self.aborted:
            # VG TV-F3: NOTE, never RED -- a red, a timeout or a crash of a
            # quarantined test is a finding; only an expiry without a fix is RED.
            verdict, reason = flakes.quarantine_station_verdict(
                0 if rc == 0 and not timed_out and not red_reason else (rc or 1),
                self.d.get("quarantine_expired") or [])
            return verdict, reason, None
        if red_reason:
            return "RED", red_reason, None
        if self.aborted and rc != 0:
            return "ABORTED", _abort_reason(self.aborted), None
        if rc in TERMINATED and not timed_out:
            # SIGTERM from outside (a scope stop reaches every process of the
            # run at once): no verdict on the commit. Our own timeout says 124.
            return "ABORTED", "aborted_process", None
        if timed_out:
            return "RED", hang_line(name, st.get("timeout_min")), None
        log_rel = log_path
        if name == "corpus-committed" and self.mode == "strand":
            return ("NOTE", "matches its sources" if rc == 0 else
                    "the corpus is stale; regenerate it and commit it with this change", None)
        if name == "slow-tests":
            slow = ""
            try:
                with open(log_path, encoding="utf-8", errors="replace") as fh:
                    for line in fh:
                        if line.startswith("SLOW-VERDICT "):
                            slow = line[len("SLOW-VERDICT "):].rstrip("\n")
            except OSError:
                pass
            if rc == 0:
                return "GREEN", slow, None
            if rc == 1:
                # TV-K5 (a), ruling K6 2026-10-11: a time is a perf signal and a
                # finding of the wave in every mode, never a verdict on the commit.
                return ("NOTE", "%sa finding of the wave, named in the log"
                        % ("%s; " % slow if slow else ""), None)
            return "RED", slow, None
        if name == "test-bans-inventory":
            # VORGABEN § 5.4: the count is a finding to read; a rise is RED.
            count = ""
            try:
                with open(log_path, encoding="utf-8", errors="replace") as fh:
                    for line in fh:
                        if line.startswith("TEST-BANS-INVENTORY "):
                            count = line[len("TEST-BANS-INVENTORY "):].rstrip("\n")
            except OSError:
                pass
            return ("NOTE" if rc == 0 else "RED"), count, None
        if name == "gate-selftest":
            # The shard runner names the red shard and its log (GH #1126 / #1150);
            # the exit code alone decides, the line only explains.
            shards = ""
            try:
                with open(log_path, encoding="utf-8", errors="replace") as fh:
                    for line in fh:
                        if line.startswith("SELFTEST-VERDICT "):
                            shards = line.rstrip("\n").split(" ", 2)[-1]
            except OSError:
                pass
            return ("GREEN" if rc == 0 else "RED"), shards, None
        if name == "persona-receipt":
            return self._persona(rc, log_path)
        if name == "deny-advisories":
            return "NOTE", "clean" if rc == 0 else "findings -- read %s" % log_rel, None
        if name == "crate-isolation" and rc:
            return "RED", _isolation_cause(failed, rc, log_path), None
        if st.get("nextest") is not None and rc == EXIT_NO_TESTS:
            return "RED", "no tests matched", None
        return ("GREEN" if rc == 0 else "RED"), "", None

    def _persona(self, rc, log_path):
        text = ""
        try:
            with open(log_path, encoding="utf-8", errors="replace") as fh:
                for line in fh:
                    if line.startswith("PERSONA-RECEIPT "):
                        parts = line.rstrip("\n").split(" ", 2)
                        text = parts[2] if len(parts) > 2 else ""
        except OSError:
            pass
        text = text or "no PERSONA-RECEIPT line in %s" % log_path
        if rc not in (0, 3, 4, 5, 6):
            return "RED", "the checker failed (exit %d)" % rc, None
        if self.mode == "strand":
            why = {0: "covers its sources",
                   3: "the persona sources moved (%s); measure and commit the report" % text,
                   4: "no persona report yet; measure and commit one",
                   5: "the committed report is incomplete; measure and commit a full one",
                   6: "the committed report is red (%s)" % text}[rc]
            return "NOTE", why, None
        if rc == 0:
            return "GREEN", "", None
        if rc == 6:
            return "RED", text, None
        why = self._decision_for("persona-receipt")
        if why:
            return "NOTE", "owner decided: without -- %s" % why, why
        return "ASK", text, None

    # --- the flake hook (W8) ----------------------------------------------------------
    def _known_binaries(self):
        try:
            with open(os.path.join(self.reuse_dir, "binaries.json"), encoding="utf-8") as fh:
                return set((json.load(fh).get("rust-binaries") or {}).keys())
        except (OSError, ValueError, AttributeError):
            return None

    def _quarantine_argv(self):
        """`--tool-config-file pipeline:<ws>/quarantine.toml` -- written once per run.

        The dispatch's `quarantine` (the kern's `dispatch_quarantine`) plus the
        profile `isolate`; [] without a build of this run (an entry whose
        binary nextest does not know would refuse the whole file)."""
        with self.cond:
            if getattr(self, "_qargv", None) is None:
                known = self._known_binaries()
                if known is None:
                    self._qargv = []
                else:
                    self._qargv, dropped = flakes.write_quarantine(
                        self.ws, self.d.get("quarantine") or [], known,
                        profiles=flakes.QUARANTINE_PROFILES)
                    for test in dropped:
                        print("exec: quarantine entry %s dropped (binary not in this build)"
                              % test, file=self.err)
            return list(self._qargv)

    def _quarantine_expr(self):
        """The filterset of the dispatch's quarantine, narrowed to this build."""
        known = self._known_binaries()
        if known is None:
            return ""
        return flakes.quarantine_filter(self.d.get("quarantine") or [], known)

    def _isolate(self, st, cmd, log_path, started_wall, junits):
        """The red station's triage (flakes.triage_station, OR-TV-2) -- None when it
        cannot be triaged at all (the station ends as judged)."""
        argv, cwd, extra = cmd
        kind = flakes.station_kind(st, argv)
        name = st["name"]
        cwd_abs = os.path.join(self.tree, cwd) if cwd else self.tree
        env = self._env(st, extra)
        restore = None
        proof = None
        ignored = ()
        if kind == "nextest":
            ignored = flakes.runs_ignored(argv) or ()
            qargv = self._quarantine_argv()
            if not qargv or not junits:
                return None
            results = [dict(r, station=name) for p in junits for r in flakes.junit_parts(p)]
            base = ["cargo-nextest", "nextest", "run",
                    "--binaries-metadata", os.path.join(self.reuse_dir, "binaries.json"),
                    "--cargo-metadata", os.path.join(self.reuse_dir, "cargo-metadata.json")] + qargv
            cwd_abs = self.tree
        elif kind == "unittest":
            try:
                with open(log_path, encoding="utf-8", errors="replace") as fh:
                    text = fh.read()
            except OSError:
                return None
            marker = "$ %s\n" % shlex.join(argv)
            text = text[text.rfind(marker) + len(marker):] if marker in text else text
            results = flakes.unittest_test_results(name, text)
            base = list(argv)
        elif kind == "scenario":
            path = flakes.scenario_runner_path(argv, cwd_abs)
            got = flakes.scenario_last_run(path, started_wall)
            if got is None:
                return {"verdict": "RED", "isolated": 0, "events": [],
                        "reason": "%s: no last_run.json of this run" % flakes.UNEXPLAINED}
            results = flakes.scenario_test_results(name, got[0], got[1])
            base = flakes.scenario_base_argv(argv)
            try:
                with open(path, "rb") as fh:
                    restore = (path, fh.read())
            except OSError:
                restore = None
            proof = self._scenario_proof(path)
        else:
            return None
        if kind != "nextest":
            # The register's observation (welle-tb S3, #1161): a unittest or
            # scenario station has no JUnit file, so its `test_result` lines are
            # the ones read here -- without them `refresh` never sees a FLAKY
            # test of the station and the issue duty ends in `not in the register`.
            for r in results:
                self.emit("test_result", r)
        counter = [0]
        # Delta I1: ONE budget for the whole isolation of this station -- the
        # station's own timeout_min, counted from here for every run together,
        # never a fresh timeout per run.
        iso_t0 = self.clock()
        budget = float(st.get("timeout_min") or 30) * 60

        def run_cmd(iso_argv, iso_env=None):
            counter[0] += 1
            if proof is not None:
                proof.mark()
            iso_log = os.path.join(self.logs_dir, "%s.isolate-%d.log" % (name, counter[0]))
            with open(iso_log, "w", encoding="utf-8") as fh:
                rc, _ = self._spawn(st, iso_argv, cwd_abs, dict(env, **(iso_env or {})), fh,
                                    iso_t0)
            try:
                with open(iso_log, encoding="utf-8", errors="replace") as fh:
                    return rc, fh.read()
            except OSError:
                return rc, ""

        try:
            return flakes.triage_station(
                name, kind, results, run_cmd, isolation_issues(self.d), base_argv=base, ignored=ignored,
                expired=lambda: self.clock() - iso_t0 >= budget, proof=proof,
                threads=_station_threads(st, env), clock_tests=flakes.clock_tests(
                    results, self.tree) if kind == "unittest" else ())
        finally:
            if restore is not None:
                # The isolation rewrote the runner's file ten times: the station's
                # own result is what the receipt and the next reader see.
                with open(restore[0], "wb") as fh:
                    fh.write(restore[1])

    @staticmethod
    def _scenario_proof(path):
        """`proof(test, rc, output)` of one scenario isolation run (Delta I2):
        the runner's `last_run.json` was rewritten after `mark()` and names the
        case green -- a run that never wrote it, or skipped the case, proves
        nothing."""
        def stamp():
            try:
                return os.stat(path).st_mtime_ns
            except OSError:
                return None
        before = [stamp()]

        def proof(test, rc, output):
            now = stamp()
            if now is None or (before[0] is not None and now <= before[0]):
                return False
            try:
                cases = flakes.parse_scenario_last_run(path)
            except (OSError, ValueError, TypeError, AttributeError):
                return False
            return cases.get(flakes.case_id(test)) == "GREEN"
        proof.mark = lambda: before.__setitem__(0, stamp())
        return proof

    def _emit_junit(self, st, cmds, started_wall):
        paths, profiles = [], []
        for _argv, _cwd, extra in cmds:
            p = extra.get("MECLAW_TIER_PROFILE")
            if p and p not in profiles:
                profiles.append(p)
        for p in profiles:
            path = os.path.join(self.tree, "target", "nextest", p, "junit.xml")
            try:
                if os.path.getmtime(path) < started_wall - 1:
                    continue        # left over from an earlier run
                # The register's form (W8): the executor's plus `flaky_names`
                # and `hard_failures` -- what nextest retried green, and what
                # no isolation may turn green.
                results = flakes.junit_parts(path)
            except (OSError, ET.ParseError):
                continue
            paths.append(path)
            for r in results:
                self.emit("test_result", dict(r, station=st["name"]))
        return paths

    # --- watchers and the end --------------------------------------------------------
    def abort(self, kind):
        with self.cond:
            if self.aborted:
                return
            self.aborted = kind
            procs = list(self.procs.values())
            self.cond.notify_all()
        self._kill(procs)

    @staticmethod
    def _kill(procs):
        """SIGTERM to the own sessions of `procs`, SIGKILL after the grace."""
        for p in procs:
            try:
                os.killpg(p.pid, signal.SIGTERM)
            except (ProcessLookupError, PermissionError):
                pass

        def hard_kill():
            for p in procs:
                if p.poll() is None:
                    try:
                        os.killpg(p.pid, signal.SIGKILL)
                    except (ProcessLookupError, PermissionError):
                        pass
        t = threading.Timer(KILL_GRACE_S, hard_kill)
        t.daemon = True
        t.start()

    def _deadline_fired(self):
        elapsed = round(self.clock() - self.t_start, 1)
        self.emit("watchdog_fired", {"runner": self.runner, "kind": "deadline",
                                     "value": elapsed, "threshold": self.deadline_s,
                                     "action": "aborted"}, kind="finding")
        self.abort("deadline")

    def _watch(self):
        threshold = float((self.d.get("budget") or {}).get("free_min_g") or 0)
        if threshold <= 0:
            return
        while not self.stop_event.wait(WATCH_PERIOD_S):
            free = _free_g(self.target)
            # Fail closed like the entry check (GH #1154): a disk the watcher
            # cannot measure is no disk above the floor.
            if free is None or free < threshold:
                self.emit("watchdog_fired", {"runner": self.runner, "kind": "disk",
                                             "value": free, "threshold": threshold,
                                             "action": "aborted"}, kind="finding")
                self.abort("disk")
                return

    def _hygiene(self):
        """True when the lane may run; emits `hygiene_checked`."""
        budget = self.d.get("budget") or {}
        if budget.get("need_free_g") is not None:
            # the admission's sum: reserve + the live runs' reservation + expected
            need = float(budget["need_free_g"] or 0)
        else:
            need = float(budget.get("reserve_g") or 0) + float(self.d.get("expected_g") or 0)
        free = _free_g(self.target)
        # A floor that cannot be measured refuses (fail-closed, Delta A I-neu-1).
        ok = free >= need if free is not None else need <= 0
        self.emit("hygiene_checked", {"runner": self.runner, "free_g": free, "reserved_g": need,
                                      "leftovers": 0, "zombies": 0, "ports_busy": 0,
                                      "drift": [], "verdict": "GREEN" if ok else "ABORTED"},
                  kind="note" if ok else "finding")
        if not ok:
            print("exec: only %sG free on %s -- need %sG (reserve + live + expected)"
                  % (free, self.target, need), file=self.err)
        return ok

    def _fingerprint(self):
        expected = self.d.get("fingerprint_expected") or {}
        # the linker, the build environment and the incremental policy are the
        # stations' own (`base_env`), measured -- never the register's echo (M7t A-M3)
        fp = lane_fingerprint(self.tree, self.target, self.base_env)
        self.head["fingerprint"] = fp
        self.emit("lane_fingerprint", fp)
        if fp.get("unanswered"):   # M7r A-M1: said in the run, not only in the event
            self.note_row("lane-fingerprint", fp["unanswered"], 0,
                          "no fingerprint: no cache key, no cache entry, the register keeps its own")
        drift = fingerprint_drift(fp, expected)
        if drift:
            self.note_row("lane-drift", ",".join(drift), 0, "the lane differs from its register")
        # A booked engine that does not start is said in the run, not only in the
        # receipt (Re-Check S3 Teil 2, point 5): the scheduler books it 0 on this
        # lane. A run that books no engine says nothing (no node in most test lanes).
        booked = {e for st in self.by_name.values() for e in _booked_engines(st)}
        refused = {e: why for e, why in sorted((fp.get("browsers") or {}).items())
                   if why != "ok" and e in booked}
        if refused:
            self.note_row("browsers", ",".join(refused), 0, "capacity 0 on this lane: %s" % "; ".join(
                "%s: %s" % (e, why) for e, why in refused.items()))

    def _critical_path(self):
        memo = {}

        def cp(n, seen=()):
            if n in memo:
                return memo[n]
            if n in seen:
                return 0.0
            preds = _after(self.by_name[n], self.names)
            memo[n] = self.secs.get(n, 0.0) + max([cp(p, seen + (n,)) for p in preds] or [0.0])
            return memo[n]
        return round(max([cp(n) for n in self.by_name] or [0.0]), 3)

    def finish(self, hygiene_abort=False):
        counts = {k: 0 for k in ("green", "red", "cached", "flaky", "skip", "note", "ask",
                                 "aborted")}
        lane_seconds = 0.0
        for ev in self.events:
            if ev["event"] == "station_ended":
                v = ev["data"]["verdict"].lower()
                if v in counts:
                    counts[v] += 1
                lane_seconds += float(ev["data"].get("secs") or 0)
        # A station that ended ABORTED (its thread failed) is booked in
        # `verdicts` even when its stream line was lost: never GREEN then.
        station_aborted = counts["aborted"] or "ABORTED" in self.verdicts.values()
        verdict = pass_verdict([ev["data"] for ev in self.events if ev["event"] == "station_ended"],
                               aborted=bool(self.aborted or hygiene_abort),
                               station_aborted=bool(station_aborted))
        data = {"verdict": verdict, "secs": round(self.clock() - self.t_start, 3),
                "lane_seconds": round(lane_seconds, 3), "critical_path_s": self._critical_path(),
                "rounds": 1, "cache_hits": self.cache_hits, "stations": counts}
        if hygiene_abort:
            data["reason"] = "hygiene"
        elif self.aborted:
            data["reason"] = self.aborted
        elif verdict == "ABORTED":
            data["reason"] = "process"     # the kind of `abort`, as for a whole-run abort
        self.stop_event.set()
        if self.deadline_timer is not None:
            self.deadline_timer.cancel()
        with self.cond:
            if self.lock_fd is not None:
                os.close(self.lock_fd)      # the end of the run is the end of the lock
                self.lock_fd = None
        self.emit("pass_ended", data)
        with self.emit_lock:
            self.closed = True
            self.events_fh.close()
        return verdict

    def _on_sigterm(self, signum, frame):
        """A foreign stop (systemd stops the scope) or `exec --abort`: abort the
        run like a watcher -- the children go, `pass_ended ABORTED` follows."""
        self.terminated = True
        threading.Thread(target=self.abort, args=("process",), daemon=True).start()

    def _sigterm_wakeup(self):
        """GH #1178: Python runs `_on_sigterm` only in the main thread, which sleeps
        in `Condition.wait`/`join`. A SIGTERM a station thread takes (its mask
        resets after the vfork of its process start) does not wake it, so the
        abort waited for the end of the running station. The C handler writes
        each signal to the wakeup fd from whichever thread took it; a reader
        thread blocks on that pipe and aborts at once -- an event, no clock.
        SIGINT counts as SIGTERM here (L-6, OR-TB.S10.B.1): `execute` hands it
        to `_on_sigterm` as well, so a station thread's SIGINT aborts at once too.
        Returns the undo (restore the old fd, stop the reader), or None."""
        r, w = os.pipe()
        os.set_blocking(w, False)
        try:
            prev_fd = signal.set_wakeup_fd(w, warn_on_full_buffer=False)
        except (ValueError, OSError):
            os.close(r)
            os.close(w)
            return None
        _KIT_WAKEUP_FLAGS[w] = False
        prev_warn = _restore_flag(prev_fd)
        stops = (int(signal.SIGTERM), int(signal.SIGINT))
        stopped = threading.Event()

        def take(data):
            if any(s in data for s in stops):
                self._on_sigterm(signal.SIGTERM, None)

        def reader():
            try:
                while not stopped.is_set():
                    take(os.read(r, 64))
                os.set_blocking(r, False)  # the undo restored the fd: drain, then end
                while True:
                    try:
                        take(os.read(r, 4096))
                    except BlockingIOError:
                        return
            finally:
                os.close(w)
                os.close(r)
        reader_thread = threading.Thread(target=reader, daemon=True, name="sigterm-wakeup")
        reader_thread.start()

        def undo():
            # L-2 (re-check tb-s4): a C handler on a foreign thread may have read
            # `w` as the wakeup fd just before this restore and write right after
            # it. Closed here, at once, `w`'s number could already belong to a
            # file opened meanwhile, and the byte would land there. So the undo
            # only restores and wakes the reader with byte 0 (no signal has the
            # number 0); the reader closes `w` and `r` after it, which moves the
            # close well past the restore. That narrows the CPython-inherent
            # race, it cannot close it: CPython offers no barrier for handlers
            # in flight. BlockingIOError: the pipe is full, the reader has bytes
            # to read anyway and finds `stopped` set once it has drained them.
            # L-1: `prev_fd` returns with the flag it had: the kit's own flag
            # from `_KIT_WAKEUP_FLAGS` when the kit set it (nested runs), else
            # the documented CPython default True, spelled out -- a foreign
            # setter's flag is unknowable without a getter in CPython 3.12
            # (in the CLI prev_fd is -1, where the flag means nothing).
            signal.set_wakeup_fd(prev_fd, warn_on_full_buffer=prev_warn)
            _KIT_WAKEUP_FLAGS.pop(w, None)
            stopped.set()
            try:
                os.write(w, b"\0")
            except BlockingIOError:
                pass
            # GH #1218: the run ends with its reader. Woken by the byte above (or
            # holding a full pipe), it drains, closes both ends and returns -- an
            # event, no clock. Without the join it still drained after `execute`
            # returned and its two closes landed in whatever ran next.
            reader_thread.join()
        return undo

    def execute(self):
        os.makedirs(self.logs_dir, exist_ok=True)
        os.makedirs(self.target, exist_ok=True)
        with open(os.path.join(self.ws, PID_FILE), "w", encoding="utf-8") as fh:
            fh.write("%d %s\n" % (os.getpid(), self.run_id))
        self.terminated = False
        prev = None
        prev_int = None
        undo_wakeup = None
        if threading.current_thread() is threading.main_thread():
            prev = signal.signal(signal.SIGTERM, self._on_sigterm)
            # L-6 (OR-TB.S10.B.1, revertible): during the run a SIGINT is a stop
            # like SIGTERM -- the run ends ABORTED with `pass_ended`, instead of a
            # KeyboardInterrupt the main thread raised only at the end of the
            # station when a station thread took the signal. Nothing in scripts/
            # builds on a KeyboardInterrupt out of the executor (gate.py catches
            # its own Ctrl-C and asks the executor to abort, the same outcome).
            prev_int = signal.signal(signal.SIGINT, self._on_sigterm)
            undo_wakeup = self._sigterm_wakeup()
        try:
            verdict = self._execute()
        finally:
            if undo_wakeup is not None:
                undo_wakeup()
            if prev_int is not None:
                signal.signal(signal.SIGINT, prev_int)
            if prev is not None:
                signal.signal(signal.SIGTERM, prev)
        if verdict == "ABORTED":
            # W2 contract: `pass_ended ABORTED` is in the stream BEFORE the
            # scope goes (the kern reads 143 after it as ABORTED, exit 2).
            try:
                self.out.flush()
            except (OSError, ValueError):
                pass
            (self.scope_stopper or _default_scope_stopper)(self.run_id)
        return EXIT[verdict]

    def _execute(self):
        self.events_fh = open(os.path.join(self.ws, "events.jsonl"), "a", encoding="utf-8")
        self.deadline_timer = None
        ok = self._hygiene()
        self._fingerprint()
        no_pre = self.d.get("no_pre")
        if isinstance(no_pre, dict):
            # `--no-pre --reason` (W4): the retro sees every gate that ran
            # without a pre-run, beside the owner's other decisions.
            self.emit("owner_decision", {"station": "pre", "decision": "pre_skipped",
                                         "reason": no_pre.get("reason") or ""})
        if not ok:
            with self.cond:
                for n in self.by_name:
                    self._finish(n, "ABORTED", 0)
                    self.ended(n, "ABORTED", 0, "hygiene")
            return self.finish(hygiene_abort=True)
        deadline = _parse_ts(self.d.get("deadline_at"))
        self.deadline_s = None
        if deadline is not None:
            delay = (deadline - datetime.datetime.now(datetime.timezone.utc)).total_seconds()
            self.deadline_s = round(self.clock() - self.t_start + max(delay, 0), 1)
            self.deadline_timer = threading.Timer(max(delay, 0), self._deadline_fired)
            self.deadline_timer.daemon = True
            self.deadline_timer.start()
        watcher = threading.Thread(target=self._watch, daemon=True)
        watcher.start()
        self.schedule()
        for t in self.threads:
            t.join()
        return self.finish()


def run(dispatch, out, err, scope_stopper=None, clock=time.monotonic):
    """Execute one dispatch; the event stream goes to `out`, diagnostics to `err`.

    Returns the exit code of the run's verdict (GREEN 0, RED 1, ABORTED 2, ASK 4).
    `scope_stopper(run)` is called after `pass_ended ABORTED`; the default
    stops `pipe-<run>.scope` when this process runs inside it. `clock` is the
    run's monotonic clock (a fake one in the self-tests, TV-K1)."""
    cargo = [s.get("name") for s in dispatch.get("stations") or [] if _needs_cargo(s)]
    if cargo and not dispatch.get("+lock_file"):
        # fail-closed: a child told MECLAW_CARGO_LOCK_HELD=1 skips its own flock
        print("exec: dispatch refused: %s need cargo, but the dispatch names no +lock_file"
              % ", ".join(cargo), file=err)
        # Delta A M-neu-3: the stream still ends -- the kern books ABORTED, not lost.
        data = {"run": dispatch.get("run"), "verdict": "ABORTED", "secs": 0, "lane_seconds": 0,
                "critical_path_s": 0, "rounds": 1, "cache_hits": 0, "stations": {},
                "reason": "process"}
        events.validate("pass_ended", "status", data)
        out.write(json.dumps(events.to_line({
            "at": store.now(), "run": dispatch.get("run"), "kind": "status", "event": "pass_ended",
            "text": events.summary_text("pass_ended", data), "data": data}), sort_keys=True) + "\n")
        out.flush()
        return EXIT["ABORTED"]
    return _Run(dispatch, out, err, scope_stopper, clock=clock).execute()


def abort_run(run, ws=None, err=None):
    """`exec --abort <run>`: SIGTERM to the executor of `run` on this lane (its
    pid in `<ws>/exec.pid`); the executor then aborts like a watcher and ends
    the stream with `pass_ended ABORTED`. 0 = signalled, 1 = no such executor."""
    err = err or sys.stderr
    ws = ws or os.path.join(os.environ.get("MECLAW_LANE_ROOT") or LANE_ROOT, "ws", run)
    try:
        with open(os.path.join(ws, PID_FILE), encoding="utf-8") as fh:
            words = fh.read().split()
        pid, owner = int(words[0]), words[1]
    except (OSError, ValueError, IndexError):
        print("exec: no executor of %s (no %s in %s)" % (run, PID_FILE, ws), file=err)
        return 1
    try:
        with open("/proc/%d/cmdline" % pid, "rb") as fh:
            argv = fh.read().split(b"\0")
    except OSError:
        argv = []
    if owner != run or b"exec" not in argv:
        print("exec: the executor of %s is gone (pid %d)" % (run, pid), file=err)
        return 1
    try:
        os.kill(pid, signal.SIGTERM)
    except ProcessLookupError:
        print("exec: the executor of %s is gone (pid %d)" % (run, pid), file=err)
        return 1
    print("exec: SIGTERM to the executor of %s (pid %d)" % (run, pid), file=err)
    return 0


def _cmd_exec(args):
    if args.abort:
        return abort_run(args.abort, args.ws)
    if args.dispatch == "-":
        text = sys.stdin.read()
    else:
        with open(args.dispatch, encoding="utf-8") as fh:
            text = fh.read()
    try:
        dispatch = json.loads(text)
    except ValueError as exc:
        print("exec: the dispatch is not JSON: %s" % exc, file=sys.stderr)
        return 2
    return run(dispatch, sys.stdout, sys.stderr)


def register(sub):
    p = sub.add_parser("exec", help="run one dispatch on this lane (events on stdout)",
                       description=__doc__.splitlines()[0])
    p.add_argument("--dispatch", default="-", help="the dispatch JSON file, '-' = stdin")
    p.add_argument("--abort", metavar="RUN",
                   help="abort the running executor of RUN on this lane (SIGTERM; it ends "
                        "the stream with pass_ended ABORTED)")
    p.add_argument("--ws", help="the run's workspace (default <MECLAW_LANE_ROOT>/ws/<run>)")
    p.set_defaults(func=_cmd_exec)
