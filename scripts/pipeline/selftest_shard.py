#!/usr/bin/python3 -I
"""meclaw -- `python3 -m unittest <modules>`, split into shards that run side by side.

WHY THIS EXISTS
===============
`gate-selftest` ran its ~2 000 script tests in one serial `unittest` process,
684 s / 715 s / 772 s in three gates (GH #1126). The modules are independent,
so the station runs them in N processes at once.

    /usr/bin/python3 -I scripts/pipeline/selftest_shard.py unittest <module> [<module> ...]
    /usr/bin/python3 -I scripts/pipeline/selftest_shard.py --plan-only unittest <module> ...

Everything after `unittest` is what `python3 -m unittest` would get. The word
`unittest` comes right after the script, so the flake isolation still sees a
unittest station (`flakes.station_kind` reads the words after the interpreter,
`lane.python_rest`) and appends `-k <test id>` to it.

SHARD COUNT
===========
min(`plan.selftest_shards(nproc)`, nproc), nproc = the threads this process may
run on: `MECLAW_SELFTEST_SHARDS` when set (an integer >= 1, anything else is a
usage error, exit 2), else 4 -- three shards already reach the longest module;
never more shards than modules. Admission books `plan.CPU_SHARDS` against the
lane, and the executor hands the booked number to this runner as
`MECLAW_SELFTEST_SHARDS`, on a remote lane too: started == booked.

A SIGTERM or SIGINT on the runner terminates every shard (killed after
`KILL_GRACE_S`, half the executor's grace), starts no new one and ends with exit
128 + signal. The shards stay in the runner's process group, which is the
station's: the executor's stop (SIGTERM, then SIGKILL to the group) reaches every
shard and every process a test started. Each shard dies with the runner as well
(PR_SET_PDEATHSIG), so a SIGKILL on the runner alone (the OOM killer) leaves no
shard; the next start removes the state dir of a run whose runner is gone.
Limits: a stop on the runner alone signals the shards, not their children, and a
process that leaves the group survives the executor's stop.

ONE SHARD = THE OLD COMMAND
===========================
One shard, or any option among the unittest arguments (`-k`, `-v`, ...), runs
`python3 -m unittest <args>` in place of this process (exec): the exact
command of before, same output, same exit code.

DISTRIBUTION
============
Deterministic, every module exactly once. With a duration table
(`selftest_durations.json` next to this file, `{module: seconds}`): longest
first onto the least loaded shard (ties: the lower shard); a module the table
does not know weighs the table's median. Without a table: round-robin over
the sorted module names. With a table, a module heavier than the ideal shard
(sum / shards) is split into its TestCase classes first (class mode, #1160,
`plan_units`), each class one unit; a class heavier than the ideal shard runs
as parts of its test methods (method mode, `<module>.<Class>[k/n]`, each part
started with its test ids). `--plan-only` prints the distribution and runs
nothing:

    SHARD <k>/<n> modules=<m> tests=<t> est=<secs>s <module> ...
    SHARD-PLAN shards=<n> modules=<m> tests=<t> units=<u>

ORDER
=====
The units of each shard run in a random order (TV-K4: a hidden order
dependence shows up as a red run, not as luck): the runner prints
`SELFTEST-SEED <n>` first and shuffles with it; `MECLAW_SELFTEST_SEED=<n>`
replays that order (a non-integer is a usage error, exit 2). Which shard a
unit runs in stays deterministic. A call with unittest options (the
isolation's `-k`) is not shuffled -- it stays the exact command of before.

ISOLATION
=========
Every shard gets its own directory with its own `TMPDIR`, `XDG_STATE_HOME`
(gate and merge state) and state paths: pipeline store, lanes file, token
file, cargo lock, coverage dir. The test guard
(`scripts/tests/store_isolation.py`) moves the store again inside that
`TMPDIR`. The directories go when the run ends.

Still shared, as in the serial run: `HOME`, `CARGO_TARGET_DIR`, the import
graph (`MECLAW_PIPELINE_GRAPH_DIR`), the pool state (`MECLAW_POOL_STATE`) and
the port register (`MECLAW_PORT_REGISTER`). No station test builds with cargo,
and the tests that write the other three point them (or `--state-dir`) at a
temporary directory of their own first; the rest only read.

VERDICT (fail-closed)
=====================
A shard is GREEN only with exit 0 AND a unittest summary that says `OK`.
Exit != 0, no summary (a crash, a kill) or a `FAILED` summary is RED. One RED
shard makes the station RED. Each shard's output goes to its own log: next to
the station log when stdout is a file (`<station>.shard-<k>.log`), else into
`MECLAW_SELFTEST_LOG_DIR` or a kept temporary directory. The run prints

    SHARD <k>/<n> <GREEN|RED> tests=<t> fail=<f> <secs>s -- log <path> [-- <reason>]

per shard, the whole output of every RED shard, then one combined unittest
summary (`Ran <total> tests in <secs>s` + `OK` / `FAILED (...)`; a crashed
shard counts as one error, so the summary never reads OK), and last

    SELFTEST-VERDICT <GREEN|RED> <green>/<n> shards[; shard <k>/<n> <reason> -- log <file>
                     [-- red <test id>,...]][; seed <n>]

which the executor puts on the GATE line (GH #1150) and the store keeps as the
station's reason: a red line names the red tests of each red shard and the
seed that replays the order. Exit 0 GREEN, 1 RED,
2 usage, 128 + signal when stopped.
"""

import concurrent.futures
import json
import os
import random
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(1, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
import plan  # noqa: E402
from pipeline import flakes, gitenv, hang_dump  # noqa: E402

HERE = os.path.dirname(os.path.abspath(__file__))
DURATIONS = os.path.join(HERE, "selftest_durations.json")
LOG_DIR_ENV = "MECLAW_SELFTEST_LOG_DIR"
DURATIONS_ENV = "MECLAW_SELFTEST_DURATIONS"
# TV-K4 (tv-p1c): the order inside each shard is random; the seed is printed
# (`SELFTEST-SEED <n>`) and this variable replays it.
SEED_ENV = "MECLAW_SELFTEST_SEED"
# The state every shard gets for itself (variable -> file name in its state dir).
STATE_ENVS = {
    "MECLAW_PIPELINE_DB": "pipeline.db",
    "MECLAW_LANES_FILE": "lanes",
    "MECLAW_STRAND_TOKENS": "cargo.tokens",
    "MECLAW_GATE_LOCK": "cargo.lock",
    "MECLAW_PIPELINE_COVERAGE_DIR": "coverage",
    "XDG_STATE_HOME": "xdg-state",
}

_DEF = re.compile(r"^\s*def (test\w*)\s*\(", re.M)


def usage(msg):
    print("selftest_shard: %s" % msg, file=sys.stderr)
    return 2


def load_durations(path):
    """`{module: seconds}` from the table, {} when there is none or it is unreadable."""
    try:
        with open(path, encoding="utf-8") as fh:
            data = json.load(fh)
    except (OSError, ValueError):
        return {}
    if not isinstance(data, dict):
        return {}
    return {str(k): float(v) for k, v in data.items()
            if isinstance(v, (int, float)) and not isinstance(v, bool)}


# Measured shard seconds this far apart (max over mean) say the table no longer
# matches the run: a NOTE, never a verdict (re-check tb-s2 h2 m-N5). Runs whose
# slowest shard stays under the floor are start-up noise, never a NOTE.
# 1.3: the stale table of the A/B run (receipts/tb-s2) measured 1.46/1.61,
# the fresh one 1.09/1.12 (re-check tb-s2 h2 m-N8).
IMBALANCE_NOTE = 1.3
IMBALANCE_FLOOR_S = 5.0


def imbalance(secs):
    """The NOTE text when the slowest shard ran more than `IMBALANCE_NOTE`
    times the mean shard ('' otherwise, for fewer than two shards, and when
    the slowest shard ran under `IMBALANCE_FLOOR_S`)."""
    if len(secs) < 2 or max(secs) < IMBALANCE_FLOOR_S:
        return ""
    mean = sum(secs) / len(secs)
    if mean <= 0 or max(secs) / mean <= IMBALANCE_NOTE:
        return ""
    return ("shard imbalance max/mean %.2f > %.1f (%s s) -- is %s stale?"
            % (max(secs) / mean, IMBALANCE_NOTE, "/".join("%.1f" % x for x in secs),
               os.path.relpath(DURATIONS, os.path.dirname(os.path.dirname(HERE)))))


def distribute(modules, shards, durations=None):
    """`[[module, ...], ...]`: `shards` lists, every module in exactly one.

    With `durations`: longest first onto the least loaded shard (LPT), ties to
    the lower shard; unknown modules weigh the median. Without: round-robin
    over the sorted names."""
    mods = sorted(set(modules))
    shards = max(1, min(int(shards), len(mods) or 1))
    out = [[] for _ in range(shards)]
    if durations:
        known = sorted(durations[m] for m in mods if m in durations)
        median = known[len(known) // 2] if known else 1.0
        load = [0.0] * shards
        for m in sorted(mods, key=lambda m: (-durations.get(m, median), m)):
            k = min(range(shards), key=lambda i: (load[i], i))
            out[k].append(m)
            load[k] += durations.get(m, median)
        for lst in out:
            lst.sort()
    else:
        for i, m in enumerate(mods):
            out[i % shards].append(m)
    return out


def count_tests(module, root):
    """`def test*` lines in the module's file -- an estimate for the plan, 0 when unknown."""
    path = os.path.join(root, *module.split(".")) + ".py"
    try:
        with open(path, encoding="utf-8", errors="replace") as fh:
            return len(_DEF.findall(fh.read()))
    except OSError:
        return 0


# --- the class mode (#1160) ---------------------------------------------------
# One module can outweigh a whole shard: test_strand_sh (151 s) was the floor of
# gate-selftest and of pre. A module heavier than the ideal shard (sum / shards)
# runs as its TestCase classes, one unit each (`-m unittest <module>.<Class>`):
# the same tests, each exactly once. Kept whole (fail-closed): a module with
# `load_tests` (it composes its own suite, a class id cannot name that), one
# that does not import, one with fewer than two classes, and one whose classes
# do not add up to its test count. `setUpModule`/`tearDownModule` do NOT keep a
# module whole: unittest runs them once per process around the module's classes
# (`_handleModuleFixture`), so every shard runs them exactly as the whole module
# did -- both drivers (test_strand_sh, test_gate_sh) carry only the store guard.
MODULE_FIXTURES = ("load_tests",)
_DISCOVER = r"""
import importlib, json, os, sys, unittest
if sys.flags.safe_path:
    sys.path.insert(0, os.getcwd())   # `-I` (lane.PYTHON): the module imports from the root, as `-c` would
mod = importlib.import_module(sys.argv[1])
loader = unittest.TestLoader()
classes, methods, fixtures = {}, {}, []
def class_fixture(cls):
    return any(k not in (unittest.TestCase, object)
               and ("setUpClass" in vars(k) or "tearDownClass" in vars(k)) for k in cls.__mro__)
for name in dir(mod):
    obj = getattr(mod, name)
    if isinstance(obj, type) and issubclass(obj, unittest.TestCase) \
            and obj not in (unittest.TestCase, unittest.FunctionTestCase):
        suite = loader.loadTestsFromTestCase(obj)
        n = suite.countTestCases()
        if n:
            classes[name] = n
            methods[name] = [t.id() for t in suite]
            if class_fixture(obj):
                fixtures.append(name)
print(json.dumps({"fixture": any(hasattr(mod, f) for f in %r), "classes": classes,
                  "methods": methods, "class_fixtures": fixtures,
                  "total": loader.loadTestsFromModule(mod).countTestCases()}))
""" % (MODULE_FIXTURES,)
DISCOVER_TIMEOUT_S = 120


def discover(module, root):
    """`{fixture, classes: {name: tests}, methods: {name: [id]}, class_fixtures: [name], total}`
    of a module, imported in a child
    process (its import never touches this runner), or None when that fails."""
    try:
        from pipeline import lane   # the kit's one Python (M7p: `sys.executable` dropped -I)
        res = subprocess.run(lane.python_argv("-c", _DISCOVER, module), cwd=root,
                             capture_output=True, text=True, stdin=subprocess.DEVNULL,
                             timeout=DISCOVER_TIMEOUT_S)
        info = json.loads(res.stdout.strip().splitlines()[-1]) if res.returncode == 0 else None
    except (OSError, ValueError, IndexError, subprocess.TimeoutExpired):
        return None
    return info if isinstance(info, dict) else None


def plan_units(modules, shards, durations, root, discover_fn=None):
    """`(units, weights, tests)`: the modules, a heavy one split into its classes.

    Splits only with a duration table and more than one shard. A class weighs
    its own table entry (`<module>.<Class>`), else its share by test count of
    what the module's table entry leaves; a class heavier than the ideal shard
    runs as `method_units`. `tests` knows the count of every split unit."""
    mods = sorted(set(modules))
    if not durations or shards <= 1:
        return mods, dict(durations or {}), {}
    known = sorted(durations[m] for m in mods if m in durations)
    median = known[len(known) // 2] if known else 1.0
    weight = {m: durations.get(m, median) for m in mods}
    ideal = sum(weight.values()) / shards
    units, weights, tests = [], {}, {}
    for m in mods:
        info = (discover_fn or discover)(m, root) if weight[m] > ideal else None
        classes = (info or {}).get("classes") or {}
        whole = (not info or info.get("fixture") or len(classes) < 2
                 or sum(classes.values()) != info.get("total"))
        if whole:
            units.append(m)
            weights[m] = weight[m]
            continue
        own = {n: durations["%s.%s" % (m, n)] for n in classes if "%s.%s" % (m, n) in durations}
        rest = max(weight[m] - sum(own.values()), 0.0)
        others = float(sum(c for n, c in classes.items() if n not in own)) or 1.0
        for name in sorted(classes):
            unit = "%s.%s" % (m, name)
            w = own.get(name, rest * classes[name] / others)
            fixtures = info.get("class_fixtures")
            # A class fixture, or no word about one: the class stays whole
            # (re-check tb-s2 h2 m3 -- setUpClass would run once per part).
            parts = [] if not isinstance(fixtures, list) or name in fixtures else method_units(
                unit, ((info.get("methods") or {}).get(name)), classes[name], w, ideal, shards)
            for part in parts or [unit]:
                n = len(part.names) if parts else classes[name]
                units.append(part)
                weights[part] = w * n / classes[name]
                tests[part] = n
    return units, weights, tests


# --- the method mode (#1160, OR-TB.S2.6) ---------------------------------------
# One class can outweigh a whole shard: test_strand_sh.TestGateHost (94 tests,
# 115 s) was the floor of pre. A class heavier than the ideal shard runs as n
# parts of its test methods (n = shards, at most its tests: the parts spread
# over every shard, an estimate off for the class hits each shard alike),
# round-robin over the loader's order; a part is started with its test
# ids (`<module>.<Class>.<method>`). Kept whole (fail-closed): a class with its
# own setUpClass/tearDownClass anywhere in its MRO, or a discovery that does
# not say (unittest would run the fixture once per part, in parallel processes
# -- a fixed path or port would collide; re-check tb-s2 h2 m3), a module with
# `load_tests` (the module is never split), and a
# class whose method ids are missing, double, of another count or not
# `<module>.<Class>.<method>` (a class imported from elsewhere). A part without
# its ids starts as its key, which unittest cannot import: RED.
class MethodUnit(str):
    """One part of a class: the key `<module>.<Class>[k/n]` and its test ids."""
    names = ()


def method_units(unit, ids, count, weight, ideal, shards):
    """The parts of the class `unit`, or [] (the class stays whole)."""
    if weight <= ideal or ideal <= 0:
        return []
    n = min(int(shards), int(count))
    prefix = unit + "."
    if (n < 2 or not isinstance(ids, list) or len(ids) != count or len(set(ids)) != count
            or any(not isinstance(i, str) or not i.startswith(prefix)
                   or not i[len(prefix):].isidentifier() for i in ids)):
        return []
    parts = []
    for k in range(n):
        part = MethodUnit("%s[%d/%d]" % (unit, k + 1, n))
        part.names = tuple(ids[k::n])
        parts.append(part)
    return parts


def module_of(unit):
    """The module of a split unit (`<module>.<Class>` or `<module>.<Class>[k/n]`)."""
    return unit.split("[", 1)[0].rsplit(".", 1)[0]


def recount(units, tests, root, discover_fn=None):
    """RED reasons of a split run: a split module whose test count at the end
    is not the sum of its planned parts. A class or a method added between the
    plan and the end of the run did not run in any part (re-check tb-s2 h2
    m1); a module that no longer imports cannot be counted -- RED as well."""
    planned = {}
    for u in units:
        if u in tests:
            planned[module_of(u)] = planned.get(module_of(u), 0) + tests[u]
    out = []
    for m in sorted(planned):
        info = (discover_fn or discover)(m, root)
        total = info.get("total") if isinstance(info, dict) else None
        if total != planned[m]:
            out.append("%s has %s tests, %d planned" % (
                m, "no count of its" if total is None else total, planned[m]))
    return out


def unit_argv(units):
    """The unittest names of units: a method part expands into its test ids."""
    return [name for u in units for name in (getattr(u, "names", ()) or (u,))]


def judge(rc, text):
    """`(verdict, ran, failed, reason)` of one shard -- fail-closed."""
    s = flakes.unittest_summary(text)
    if s is None:
        return "RED", 0, 1, "exit %d without a unittest summary" % rc
    ran, failed = s
    if failed:
        return "RED", ran, failed, "%d failed" % failed
    if rc != 0:
        return "RED", ran, 1, "exit %d after an OK summary" % rc
    return "GREEN", ran, 0, ""


def red_names(text):
    """` -- red <id>,<id>` of the tests a shard's output names red, or ''."""
    failed = sorted(flakes.parse_unittest_output(text or "")["failed"])
    return " -- red %s" % ",".join(failed) if failed else ""


def shard_env(base_env, state_dir):
    """The environment of one shard: its own TMPDIR and state paths."""
    env = dict(base_env)
    tmp = os.path.join(state_dir, "tmp")
    os.makedirs(tmp, exist_ok=True)
    for name in ("TMPDIR", "TEMP", "TMP"):
        env[name] = tmp
    for name, leaf in STATE_ENVS.items():
        env[name] = os.path.join(state_dir, leaf)
    env.pop(plan.SELFTEST_SHARDS_ENV, None)
    # A runner inside a shard writes its own logs, never into this run's
    # (re-check tb-s2 h2 m4).
    env.pop(LOG_DIR_ENV, None)
    return env


def stdout_log():
    """The station log this process writes into, or None (not a regular file)."""
    try:
        path = os.readlink("/proc/self/fd/1")
    except OSError:
        return None
    return path if os.path.isfile(path) else None


def log_paths(n):
    """One log path per shard (see VERDICT)."""
    station = stdout_log()
    if station:
        stem = station[:-4] if station.endswith(".log") else station
        return ["%s.shard-%d.log" % (stem, k) for k in range(1, n + 1)]
    d = os.environ.get(LOG_DIR_ENV) or tempfile.mkdtemp(prefix="meclaw-selftest-logs-")
    os.makedirs(d, exist_ok=True)
    return [os.path.join(d, "shard-%d.log" % k) for k in range(1, n + 1)]


def _nproc():
    """The threads this process may run on (its affinity, else the machine's count)."""
    try:
        return max(1, len(os.sched_getaffinity(0)))
    except (AttributeError, OSError):
        return max(1, os.cpu_count() or 1)


_popen = subprocess.Popen
# Half the executor's KILL_GRACE_S: the runner kills its shards and removes their
# state before the executor's SIGKILL reaches the group.
KILL_GRACE_S = 5
RUNNER_PID_FILE = "runner.pid"
PR_SET_PDEATHSIG = 1
try:
    import ctypes
    _LIBC = ctypes.CDLL(None, use_errno=True) if sys.platform.startswith("linux") else None
except (ImportError, OSError):
    _LIBC = None


def _die_with_runner(runner):
    """`preexec_fn` of a shard: the kernel sends SIGKILL when the runner's thread that
    started it ends (the worker waits for its shard, so: when the runner dies)."""
    def preexec():
        _LIBC.prctl(PR_SET_PDEATHSIG, signal.SIGKILL)
        if os.getppid() != runner:  # the runner died before prctl
            os._exit(137)
    return preexec if _LIBC is not None else None


def sweep_dead_runs(tmp):
    """Removes the state dirs (`meclaw-selftest-*`) of runs whose runner is gone; a dir
    without a runner pid, or with a live (or foreign) one, stays."""
    try:
        names = os.listdir(tmp)
    except OSError:
        return
    for name in names:
        path = os.path.join(tmp, name)
        if not name.startswith("meclaw-selftest-") or name.startswith("meclaw-selftest-logs-"):
            continue
        try:
            with open(os.path.join(path, RUNNER_PID_FILE), encoding="ascii") as fh:
                pid = int(fh.read().strip())
            os.kill(pid, 0)
        except ProcessLookupError:
            shutil.rmtree(path, ignore_errors=True)
        except (OSError, ValueError):
            continue


class Children:
    """The shard processes of one run. A signal on the runner (`stop`) terminates every
    started shard, starts no new one, and kills what still lives after KILL_GRACE_S. The
    lock is reentrant: a second signal arrives in the same thread."""

    def __init__(self):
        self.lock = threading.RLock()
        self.procs = []
        self.stopped = None
        self.timer = None

    def start(self, argv, **kw):
        with self.lock:
            if self.stopped is not None:
                return None
            proc = _popen(argv, preexec_fn=_die_with_runner(os.getpid()), **kw)
            self.procs.append(proc)
            return proc

    def stop(self, signum, _frame=None):
        with self.lock:
            if self.stopped is not None:
                return
            self.stopped = signum
            procs = list(self.procs)
        self._signal(procs, "terminate")
        self.timer = threading.Timer(KILL_GRACE_S, self._signal, (procs, "kill"))
        self.timer.daemon = True
        self.timer.start()

    @staticmethod
    def _signal(procs, how):
        for proc in procs:
            if proc.poll() is None:  # unreaped: its pid is still the shard's
                try:
                    proc.terminate() if how == "terminate" else proc.kill()
                except OSError:
                    pass

    def close(self):
        if self.timer is not None:
            self.timer.cancel()


def guard(snap):
    """The RED reason of `gitenv.drift` ('' when the repository is as it
    was); the keys a parallel git may write are printed as a NOTE line."""
    drift, note = gitenv.drift(snap)
    if note:
        print("SELFTEST-NOTE %s" % note)
    return drift


def run_single(argv):
    """`argv` as one child in this terminal; SIGTERM/SIGINT reach it, and
    its exit is returned (128 + signal when a signal ended it)."""
    # Dies with the runner like a shard (pre's timeout kills the runner; re-check
    # tb-s2 h2 m5): no orphan keeps a hung test running.
    proc = _popen(argv, preexec_fn=_die_with_runner(os.getpid()))
    old = {}

    def forward(sig, _frame):
        try:
            proc.send_signal(sig)
        except OSError:
            pass

    for sig in (signal.SIGTERM, signal.SIGINT):
        try:
            old[sig] = signal.signal(sig, forward)
        except ValueError:  # not the main thread: the caller owns the signals
            pass
    try:
        rc = proc.wait()
    finally:
        for sig, handler in old.items():
            signal.signal(sig, handler)
    return rc if rc >= 0 else 128 - rc


def run_shard(argv, env, cwd, log, children):
    t0 = time.monotonic()
    with open(log, "w", encoding="utf-8") as fh:
        fh.write("$ %s\n" % " ".join(argv))
        fh.flush()
        try:
            proc = children.start(argv, cwd=cwd, env=env, stdout=fh, stderr=subprocess.STDOUT,
                                  stdin=subprocess.DEVNULL)
            if proc is None:
                fh.write("selftest_shard: not started, the run was stopped\n")
                rc = -int(children.stopped or signal.SIGTERM)
            else:
                rc = proc.wait()
        except OSError as exc:
            fh.write("selftest_shard: cannot start the shard: %s\n" % exc)
            rc = 127
    with open(log, encoding="utf-8", errors="replace") as fh:
        text = fh.read()
    return rc, text, time.monotonic() - t0


def main(argv):
    # Never the caller's repository (pd-1155): every git of this run is its
    # tree's -- the discovery's children included, so the pop comes before the
    # plan (re-check tb-s2 h2 m2).
    for name in gitenv.GIT_REPO_VARS:
        os.environ.pop(name, None)
    ignored = os.environ.get(plan.SELFTEST_SHARDS_ENV + "_IGNORED")
    if ignored is not None:
        print("selftest_shard: override %s=%r ignored, booked %s" % (
            plan.SELFTEST_SHARDS_ENV, ignored, os.environ.get(plan.SELFTEST_SHARDS_ENV)))
    plan_only = False
    while argv and argv[0].startswith("--"):
        if argv[0] == "--plan-only":
            plan_only = True
            argv = argv[1:]
        else:
            return usage("unknown option %s" % argv[0])
    if not argv or argv[0] != "unittest":
        return usage("usage: selftest_shard.py [--plan-only] unittest <module> ...")
    ut_args = argv[1:]
    modules = [a for a in ut_args if not a.startswith("-")]
    if not modules:
        return usage("no modules")
    nproc = _nproc()
    try:
        n = min(plan.selftest_shards(nproc, os.environ), nproc)
    except ValueError as exc:
        return usage(str(exc))
    durations = load_durations(os.environ.get(DURATIONS_ENV) or DURATIONS)
    root = os.getcwd()
    # Before the plan: its discovery imports every heavy module, and an import
    # that writes the repository config is the suite's write (re-check tb-s2
    # h2 M-N1).
    snap = gitenv.snapshot(root)
    if len(modules) == len(ut_args):
        units, weights, ntests = plan_units(modules, n, durations, root)
    else:  # options (`-k` of the isolation): the modules as given, never split
        units, weights, ntests = sorted(set(modules)), durations, {}
    shards = distribute(units, min(n, len(units)), weights if durations else durations)
    if plan_only:
        total = 0
        for k, mods in enumerate(shards, 1):
            tests = sum(ntests.get(m) or count_tests(m, root) for m in mods)
            total += tests
            est = sum(weights.get(m, 0.0) for m in mods)
            print("SHARD %d/%d modules=%d tests=%d est=%.0fs %s"
                  % (k, len(shards), len(mods), tests, est, " ".join(mods)))
        print("SHARD-PLAN shards=%d modules=%d tests=%d units=%d"
              % (len(shards), len(set(modules)), total, len(units)))
        return 0
    # The shards run in the kit's one Python (re-check tv-bn M7m M2): a test
    # that reaches the narrow point in its own process needs `-I`.
    from pipeline import lane
    py = lane.PYTHON
    shuffle = len(modules) == len(ut_args)
    if shuffle:
        raw = os.environ.get(SEED_ENV)
        try:
            seed = int(raw) if raw not in (None, "") else random.SystemRandom().randrange(2 ** 32)
        except ValueError:
            return usage("%s=%r is not an integer" % (SEED_ENV, raw))
        print("SELFTEST-SEED %d" % seed, flush=True)
        rng = random.Random(seed)
        shards = [rng.sample(list(mods), len(mods)) for mods in shards]
    if len(shards) == 1 or not shuffle:
        # The command of before (one shard, or `-k` from the isolation) -- as
        # a child, not `exec`, so the guard compares after it (re-check m-2).
        sys.stdout.flush()
        rc = run_single(hang_dump.argv(py, "unittest")
                        + (unit_argv(shards[0]) if shuffle else ut_args))
        drift = guard(snap)
        if drift:
            print("SELFTEST-VERDICT RED 0/1 shards; %s" % drift)
            sys.stdout.flush()
            return rc or 1
        return rc
    logs = log_paths(len(shards))
    sweep_dead_runs(tempfile.gettempdir())
    base = tempfile.mkdtemp(prefix="meclaw-selftest-")
    with open(os.path.join(base, RUNNER_PID_FILE), "w", encoding="ascii") as fh:
        fh.write("%d\n" % os.getpid())
    t0 = time.monotonic()
    children = Children()
    old = {}
    try:
        for sig in (signal.SIGTERM, signal.SIGINT):
            try:
                old[sig] = signal.signal(sig, children.stop)
            except ValueError:  # not the main thread: the caller owns the signals
                pass
        with concurrent.futures.ThreadPoolExecutor(max_workers=len(shards)) as pool:
            futs = []
            for k, mods in enumerate(shards, 1):
                state = os.path.join(base, "shard-%d" % k)
                futs.append(pool.submit(run_shard, hang_dump.argv(py, "unittest") + unit_argv(mods),
                                        shard_env(os.environ, state), root, logs[k - 1],
                                        children))
            results = [f.result() for f in futs]
    except BaseException:
        # A shard thread that raised (the pool has waited for the others):
        # the guard still names what changed.
        drift = guard(snap)
        if drift:
            print("SELFTEST-VERDICT RED 0/%d shards; %s" % (len(shards), drift))
            sys.stdout.flush()
        raise
    finally:
        children.close()
        for sig, handler in old.items():
            signal.signal(sig, handler)
        shutil.rmtree(base, ignore_errors=True)
    secs = time.monotonic() - t0
    n = len(shards)
    if children.stopped is not None:
        drift = guard(snap)
        print("FAILED (errors=%d)" % n)
        print("SELFTEST-VERDICT RED 0/%d shards; stopped by signal %d, every shard ended%s"
              % (n, children.stopped, "; %s" % drift if drift else ""))
        sys.stdout.flush()
        return 128 + int(children.stopped)
    ran = failures = 0
    red = []
    for k, (rc, text, s) in enumerate(results, 1):
        verdict, r, f, reason = judge(rc, text)
        planned = [ntests.get(u) for u in shards[k - 1]]
        if verdict == "GREEN" and None not in planned and r != sum(planned):
            # Every unit of this shard is counted: it ran exactly that many
            # (re-check tb-s2 h2 m1).
            verdict, f, reason = "RED", 1, "ran %d of %d planned" % (r, sum(planned))
        ran += r
        failures += f
        print("SHARD %d/%d %s tests=%d fail=%d %.1fs -- log %s%s"
              % (k, n, verdict, r, f, s, logs[k - 1], (" -- %s" % reason) if reason else ""))
        if verdict == "RED":
            red.append((k, reason, text))
    for k, reason, text in red:
        print("=== shard %d/%d RED (%s) -- output of %s ===" % (k, n, reason, logs[k - 1]))
        print(text.rstrip("\n"))
        print("=== end of shard %d/%d ===" % (k, n))
    # The guard of pd-1155: a suite that wrote the repository config is RED --
    # after the recount, whose discovery imports the split modules once more.
    counted = recount(units, ntests, root)
    drift = "; ".join([d for d in [guard(snap)] if d] + counted)
    stale = imbalance([s for _rc, _text, s in results])
    if stale:
        print("SELFTEST-NOTE %s" % stale)
    print("-" * 70)
    print("Ran %d tests in %.3fs" % (ran, secs))
    print()
    print("FAILED (errors=%d)" % max(failures, 1) if red or drift else "OK")
    # Kit find C4 (Welle Testing): the line was the only record of a red run
    # (the gate stores it as the station's reason) and named neither the red
    # tests nor the seed, so the order of the run was gone with its logs.
    tail = "".join("; shard %d/%d %s -- log %s%s" % (k, n, reason, os.path.basename(logs[k - 1]),
                                                     red_names(text))
                   for k, reason, text in red)
    tail += ("; seed %d" % seed) if red and shuffle else ""
    tail += "; %s" % drift if drift else ""
    print("SELFTEST-VERDICT %s %d/%d shards%s" % ("RED" if red or drift else "GREEN",
                                                 n - len(red), n, tail))
    sys.stdout.flush()
    return 1 if red or drift else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
