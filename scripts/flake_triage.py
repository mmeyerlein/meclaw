#!/usr/bin/python3 -I
"""Flake triage: one stress run under CPU load, one table row per test.

A flake issue names a test that went red under a loaded suite and stayed green
in the pipeline's isolation. Guessing the cause costs reading; this script
measures it instead and prints a Markdown table of about ten rows:

    test | runs | red | rate | p50 s | max s | failing assert | wall-clock constructs

* The run reuses the pipeline's isolation (`flakes.nextest_isolation_argv`:
  retries 0, `--stress-count`) through ONE
  `scripts/strand.sh test` call on a lane, with every named test in one
  filterset, so the tests load each other on top of the artificial load.
* `--load N` starts N busy loops on the lane (ssh, niced like the tests) for
  the length of the run. They end when this script closes their stdin, and
  `timeout` ends them in any case.
* `--throttle P` (TV-F7) runs the tests under `CPUQuota=P%` (systemd's unit:
  100 % = one core): the lane wraps test-tier.sh in
  `systemd-run --user --scope -p CPUQuota=P%` (`MECLAW_LANE_CPU_QUOTA`,
  lane_sync.sh), when the lane delegates the cpu controller to the user
  manager (probed once per run over ssh). A lane that does not falls back to
  `--load <nproc>` busy loops, with a `NOTE throttle:` line in the header.
  `--runs N` is `--stress-count N`. This is the script's own systemd call;
  `--production-load` (K12, the fix acceptance and T2 by hand) is the lane
  about 30 % busy instead: load1 measured, `--load` up to 0.3 x vCPU, full
  quota; a lane already fuller is refused, exit 2 (wait, or take another);
  the pool guard (`pipeline/pool.py`) stays as it is.
* The static scan reads each test's source file for wall-clock constructs:
  sleeps, short timeouts (< 30 s, the error-marker convention), `Instant::now`,
  deadlines, `elapsed()`, poll loops, and the JS `timeout:` of browser proofs.

* The Python class (`--unittest`): the arguments are unittest ids (module,
  class or test; scripts/tests, the kit). `--runs N` runs them N times, each
  run a fresh process with every named test in it (`flakes.unittest_
  isolation_argv` form, `-v --durations 0`), ON THE LANE `--host` like the
  nextest class (TV-F7 by its words, R-TV-W35): the tree's commit + overlay
  goes to the lane's `wt/<strand>` through the pipeline's own transport
  (`pipeline/lane.py push`, no cargo station -- the path of the Python
  stations), and each run is one ssh call there (`_shell`), under `timeout`
  at the hang marker; bound to a held token like `strand.sh test` (`token
  check --lane`, MECLAW_STRAND_TOKEN_SKIP logged); the variables that change a
  verdict (`LANE_UNITTEST_ENV`, by exact name) travel with each run, and the
  error text of every red run stands under `red runs:` in the result.
  `--host local` runs here (the pins, a quick look; no
  TV-F7 receipt). T3 (VG TV-F2) by default: each
  run is `pipeline/shuffle.py` with seed `--seed + i`, the seeds stand in the
  header and a red run names its seed on stderr (replay: `shuffle.py --seed
  <s> <ids>`); `--no-shuffle` keeps unittest's order. `--load` and
  `--throttle` act on the run's host: `systemd-run --user --scope
  -p CPUQuota=P%` around each run where the user manager holds the cpu
  controller, else `--load <nproc>` busy loops with the NOTE line. The rows
  are the same (one per test the ids name, `Class.test`); the failing assert
  is the last traceback frame outside unittest, the scan looks for Python's
  wall-clock constructs (sleep, time.time/monotonic, `timeout=` < 30 s, poll
  loops). A run that hangs past `--hang-s` (a hang marker, never a verdict
  on speed) is killed with its process group and counts red. A red run
  without a named red test (the summary alone counts it, or there is none,
  or the run's exit code is not 0 -- a hang killed after `OK`, `os._exit`
  after it, the lane's `timeout` 124) is red too: the header says
  `unexplained: N of M runs ...`, exit 1.

Usage:
    scripts/flake_triage.py --host build03 --stress-count 30 --load 12 \\
        'meclaw-cells::gh1001_web_write_cost' 'gh1001_write_cost_does_not_grow_with_untouched_objects' ...
    scripts/flake_triage.py --host build03 --production-load --runs 5 <binary_id> <name> ...   (fix acceptance, OR-TV-13/K12)
    scripts/flake_triage.py --scan-only <binary_id> <name> ...
    scripts/flake_triage.py -C <worktree> ...   (as git -C: run as if started there)
    scripts/flake_triage.py --log <test.log> <binary_id> <name> ...   (table from an existing log)
    scripts/flake_triage.py --unittest --host build03 --production-load --runs 5 scripts.tests.test_kit_time.TestHang ...

Tests come as pairs `<binary_id> <name>`. The exit is 0 when every test ran
green in every iteration, 1 when one went red or ran fewer than
`--stress-count` times (from `--log`: not at all; its row says `NOT RUN`;
a log is green only when it ends in the runner's `TEST [..] GREEN` line, so a
station log without it is red -- triage that through a run),
2 for a wrong call -- a test that did not run is RED, never green and never
the kit's exit 3 (`waits or held`). An `#[ignore]` test runs only with
`--run-ignored` (nextest `--run-ignored all`); without it, it is NOT RUN. An asked `--load` that does not
run (no ssh target, ssh fails, a loop missing at the start or dead at the end)
is exit 2 as well: a run "under load" carries the load in its first line
(loops alive, load1 before and at the end), or it is no result.

Progress and refusals (GH #1199, kit find V2-A8: a series started without
`-I` refused with one stderr line its background wrapper never kept, and a
running series printed nothing until its end). A real run (not `--scan-only`
or `--log`) checks `-I` before anything else. Every refusal before the run --
that one, a wrong `--stress-count`, a LoadError, a LaneError -- is exit 2
with one `flake_triage: REFUSED -- <reason>` line on stderr and stdout (once
when they are one file), in `--out` and at the lease (`token who`). A
`--unittest` series writes one flushed `HH:MM:SS pass i/n done=k red=r` line
per run; a nextest series passes the stderr of `strand.sh test` through, which
carries the same line per stress iteration. With the token pool armed, a lane
run whose holder holds no lease is refused before anything runs; a series in
which no test ran in any pass ends with exit 1 and `flake_triage: NO PASS --
...` -- both name the holder looked for and the branch (V2-A8b).
"""
import argparse
import os
import pathlib
import random
import re
import signal
import shlex
import sqlite3
import statistics
import subprocess
import sys
import threading
import time

REPO = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(REPO))
from scripts.pipeline import flakes  # noqa: E402
from scripts.pipeline import lane as _lanes  # noqa: E402
from scripts.pipeline import progress  # noqa: E402

ERROR_MARKER_S = 30
RESULT = re.compile(r"^\s*(PASS|FAIL|FLAKY|TIMEOUT|LEAK-FAIL|LEAK|SIG[A-Z]+|ABORT)\s+\[\s*([0-9.]+)s\]")
SUMMARY = re.compile(r"^\s*Summary \[[^\]]*\]\s*(.*?)\s*$")
# Each nextest run starts with this line (once per stress run, not per iteration).
START = re.compile(r"^\s*Starting \d+ tests? across ")
# nextest 0.9.146 (measured, `pipeline/flakes.stress_verdict`): `N/N stress run
# iterations: N passed`, a red iteration adds `, k failed` even at rc 0.
# One iteration is singular: `1/1 stress run iteration: 1 passed` (re-check fix8 H1).
_STRESS_OK = re.compile(r"^(\d+)/(\d+) stress run iterations?(?:: (\d+) passed| successful)$")
# `gate.py` `_test` appends this line to every `strand.sh test` log once the run
# is over, GREEN only at rc 0 (with `(n/n iterations green)` for a stress run).
RUNNER = re.compile(r"^TEST \[")
_RUNNER_GREEN = re.compile(r"^TEST \[.*\] \d+/\d+ \d+s GREEN(?: \((\d+)/\1 iterations green\))?"
                           r"(?: \(browsers opt-out: MECLAW_REQUIRE_BROWSERS='.*'\))?$")
_RUN_OK = re.compile(r"^(\d+) tests? run: (\d+) passed"
                     r"(?: \(\d+ (?:slow|leaky)(?:, \d+ (?:slow|leaky))*\))?(?:, \d+ skipped)?$")
PANIC = re.compile(r"thread '([^']+)'(?: \([0-9]+\))? panicked at ([^\s:]+\.rs:[0-9]+)")

# (kind, pattern) -- one line may count for several kinds.
WALL_CLOCK = (
    ("sleep", re.compile(r"\bsleep\s*\(|\.sleep\(|waitForTimeout\(")),
    ("short-timeout", re.compile(
        r"(timeout|deadline|within|wait)\w*\s*\(?[^;\n]*Duration::from_(?:millis\s*\(\s*(?P<ms>[0-9_]+)|secs\s*\(\s*(?P<secs>[0-9_]+))",
        re.I)),
    ("js-timeout", re.compile(r"\btimeout\s*:\s*([0-9_]+)")),
    ("instant", re.compile(r"Instant::now\s*\(")),
    ("elapsed", re.compile(r"\.elapsed\s*\(")),
    ("deadline", re.compile(r"\bdeadline\b", re.I)),
    ("poll-loop", re.compile(r"^\s*(loop\s*\{|while\s+.*\{)\s*$")),
)
# The same marks in a Python test (the `--unittest` class).
PY_WALL_CLOCK = (
    ("sleep", re.compile(r"\bsleep\s*\(")),
    ("py-clock", re.compile(r"\btime\.(?:time|monotonic|perf_counter)\s*\(")),
    ("py-timeout", re.compile(r"\btimeout\s*=\s*([0-9][0-9_.]*)")),
    ("poll-loop", re.compile(r"^\s*while\s+.*:\s*$")),
)
LOCAL = "local"           # the ssh target word for this host (`--unittest --host local`)
HANG_S = 1800             # a Python run past this is a hang (marker, never a verdict on speed)


def test_file(binary_id, name, root=REPO):
    """The source file of a nextest test: `crate::target` + the module path of the name."""
    crate, _, target = binary_id.partition("::")
    tests = root / "crates" / crate / "tests"
    if not target:
        return root / "crates" / crate / "src" / "lib.rs"
    single = tests / f"{target}.rs"
    if single.exists():
        return single
    mods = name.split("::")[:-1]
    for n in range(len(mods), 0, -1):
        cand = tests / target / pathlib.Path(*mods[:n]).with_suffix(".rs")
        if cand.exists():
            return cand
        cand = tests / target / pathlib.Path(*mods[:n]) / "mod.rs"
        if cand.exists():
            return cand
    return tests / target / "main.rs"


def _short(kind, m):
    """True when the match is a wall-clock construct worth a mark (timeouts only below 30 s)."""
    if kind == "short-timeout":
        ms, secs = m.group("ms"), m.group("secs")
        value = int(ms.replace("_", "")) / 1000 if ms else int(secs.replace("_", ""))
        return value < ERROR_MARKER_S
    if kind == "js-timeout":
        return int(m.group(1).replace("_", "")) < ERROR_MARKER_S * 1000
    if kind == "py-timeout":
        try:
            return float(m.group(1).replace("_", "")) < ERROR_MARKER_S
        except ValueError:
            return False
    return True


def scan(text, kinds=WALL_CLOCK, comment="//"):
    """`{kind: [line numbers]}` of the wall-clock constructs in a test source."""
    found = {}
    lines = text.splitlines()
    for i, line in enumerate(lines, 1):
        code = line.split(comment, 1)[0] if "://" not in line else line
        for kind, pat in kinds:
            m = pat.search(code)
            if not m or not _short(kind, m):
                continue
            if kind == "poll-loop":
                body = "\n".join(lines[i:i + 12])
                if not re.search(r"sleep|tick|timeout|recv_timeout", body):
                    continue
            found.setdefault(kind, []).append(i)
    return found


def render_scan(found, max_lines=3, kinds=WALL_CLOCK):
    """`sleep×2 (L10,L40) short-timeout×1 (L77)` -- the scan as one table cell."""
    parts = []
    for kind, _ in kinds:
        nums = found.get(kind)
        if nums:
            shown = ",".join(f"L{n}" for n in nums[:max_lines]) + ("…" if len(nums) > max_lines else "")
            parts.append(f"{kind}×{len(nums)} ({shown})")
    return " ".join(parts) or "none"


def parse_log(text, cases):
    """Per test `{"runs", "red", "secs": [...], "asserts": {loc: n}}` from a nextest (stress) log."""
    stats = {c: {"runs": 0, "red": 0, "secs": [], "asserts": {}} for c in cases}
    seen = set()
    for line in text.splitlines():
        m = RESULT.match(line)
        if m:
            # A stress run repeats a slow test's line after its summary; the
            # iteration marker `[k/n] (i/t)` makes every real line unique.
            if line.strip() in seen:
                continue
            seen.add(line.strip())
            rest = line[m.end():]
            for (bid, name), st in stats.items():
                if re.search(rf"\s{re.escape(bid)}\s+{re.escape(name)}\s*$", rest):
                    st["runs"] += 1
                    st["secs"].append(float(m.group(2)))
                    if m.group(1) not in ("PASS", "LEAK"):
                        st["red"] += 1
                    break
            continue
        m = PANIC.search(line)
        if m:
            for (bid, name), st in stats.items():
                if m.group(1) == name:
                    loc = m.group(2)
                    st["asserts"][loc] = st["asserts"].get(loc, 0) + 1
    return stats


def nextest_summary(text):
    """(iterations, "") when EVERY nextest summary of `text` says success, else (0, why).

    Re-check fix5 M2, the positive oracle (TV-Q1): green needs `N/N stress run
    iterations: N passed` (or `... successful`) with nothing after it, or a plain
    run's `k test(s) run: k passed` (k >= 1, only slow/leaky/skipped besides).
    A red summary, a cut log without one, `0/0` -- all of them are never green.
    Re-check fix6 M2: a log can carry two runs (a rerun appended to the same
    log, a stress run and a plain run); only the last one counted, so a red
    first run before a green rerun (FLAKY, TV-F1) was green. Every summary must
    be positive, the first one that is not names the reason; the count is the
    largest of them (a stress run's N, not the plain rerun's 1).
    Re-check fix7 M3: a second run cut before its summary (by the lane's
    `timeout`) left the positive summaries of the first one green. Every run
    nextest starts (`Starting`) needs its own summary; a result line after the
    last summary alone is no run -- 62 of 750 green real logs print their
    result lines there (final status, measured fix J on plans/*/receipts)."""
    found, open_run, cut = [], False, ""
    for line in (text or "").splitlines():
        m = SUMMARY.match(line)
        if m:
            found.append(m.group(1))
            open_run = False
        elif START.match(line):
            if open_run and not cut:
                cut = "a run without its summary"
            open_run = True
    if open_run and found and not cut:
        cut = "a run after the last summary"
    if not found:
        return 0, "without a nextest summary"
    if cut:
        return 0, cut
    iterations = 0
    for summary in found:
        n = _positive_summary(summary)
        if not n:
            return 0, "summary `%s`" % summary
        iterations = max(iterations, n)
    return iterations, ""


def runner_verdict(text, iterations):
    """(n, "") when `text` ends in the runner's `TEST [..] p/t Ns GREEN` line, else (0, why).

    Re-check fix8 M1: `--log` read only nextest's lines -- a log the runner
    never closed (cut, no `TEST` line) or closed `RED` after a positive summary
    (rc != 0) was green. The runner's line is written after the run and carries
    its exit, so test output cannot fake it (H2). Green needs exactly one such
    line, the last one, GREEN, and its n (`(n/n iterations green)`) equal to
    the summaries' `iterations`. A runner line without that suffix predates the
    stress grading (GH #1086, 8201f72cd of 2026-10-08: 20 green stress logs of
    plans/*/receipts end in a bare `GREEN`); its GREEN is rc 0, the count is the
    summary's. A station log has no runner line: red here, its own path is a run."""
    lines = [line for line in (text or "").splitlines() if line.strip()]
    marks = [i for i, line in enumerate(lines) if RUNNER.match(line)]
    why = "without the runner's closing `TEST ... GREEN` line"
    if len(marks) != 1 or marks[0] != len(lines) - 1:
        return 0, why
    m = _RUNNER_GREEN.match(lines[-1])
    if not m or int(m.group(1) or iterations) != iterations:
        return 0, why
    return iterations, ""


def _positive_summary(summary):
    """The iterations of one positive nextest summary line, else 0."""
    m = _STRESS_OK.match(summary)
    if m and int(m.group(1)) == int(m.group(2)) >= 1 and \
            (m.group(3) is None or int(m.group(3)) == int(m.group(2))):
        return int(m.group(2))
    m = _RUN_OK.match(summary)
    if m and int(m.group(1)) == int(m.group(2)) >= 1:
        return 1
    return 0


def nextest_label(case):
    return case[1].split("::")[-1]


def table(cases, stats, scans, label=nextest_label):
    """The Markdown table, one row per test."""
    rows = ["| test | runs | red | rate | p50 s | max s | failing assert | wall-clock constructs |",
            "|---|---|---|---|---|---|---|---|"]
    for case in cases:
        st = stats.get(case) or {"runs": 0, "red": 0, "secs": [], "asserts": {}}
        secs = st["secs"]
        # kit find (i): 0 runs is no result -- the row says so (RED, exit 1)
        rate = f"{st['red'] / st['runs']:.2f}" if st["runs"] else "NOT RUN"
        p50 = f"{statistics.median(secs):.1f}" if secs else "-"
        mx = f"{max(secs):.1f}" if secs else "-"
        asserts = " ".join(f"{loc}×{n}" for loc, n in sorted(st["asserts"].items())) or "-"
        rows.append(f"| {label(case)} | {st['runs']} | {st['red']} | {rate} | {p50} | {mx} "
                    f"| {asserts} | {scans.get(case, '-')} |")
    return "\n".join(rows)


def filterset(cases):
    """One nextest filterset naming every test, binary term included (`flakes` form)."""
    terms = []
    for bid, name in cases:
        argv = flakes.nextest_isolation_argv([], bid, name)
        terms.append("(" + argv[argv.index("-E") + 1] + ")")
    return " | ".join(terms)


def strand_test_argv(cases, host, count, wave=None, strand=None, ignored=False, holder=None):
    """`scripts/strand.sh test` with the pipeline's isolation flags for all `cases`;
    `wave`/`strand` go on as `--wave`/`--strand` (R-TV-W17: beside a second wave
    directory the branch alone names no wave, and `strand.sh test` refuses).
    `ignored`: the `#[ignore]` tests run too (`--run-ignored all`, kit find (i))."""
    iso = flakes.nextest_isolation_argv([], "x", "y", count=count, ignored=ignored)
    flags = iso[:iso.index("-E")]
    # test-tier.sh names the profile itself (`isolate` = `default` without
    # retries), so the pair goes and `--retries 0` carries the isolation.
    at = flags.index("--profile")
    del flags[at:at + 2]
    # The default profile prints a PASS only for a slow test (`status-level =
    # "retry"`, `final-status-level = "slow"`): on 10.10. the 10-s WebKit test
    # was the only one with lines although all five ran 30 times. Every result
    # line is printed once, during the run, so the table counts what ran.
    own = [w for flag, v in (("--wave", wave), ("--strand", strand), ("--holder", holder))
           if v for w in (flag, v)]
    return [str(REPO / "scripts" / "strand.sh"), "test", filterset(cases), "--host", host] + own + [
            "--no-fail-fast"] + flags + ["--status-level", "pass", "--final-status-level", "none"]


def lane_target(lane, lanes_file=None):
    """The ssh target of `lane` from the private host file, or None."""
    path = pathlib.Path(lanes_file or os.environ.get("MECLAW_LANES_FILE")
                        or pathlib.Path.home() / ".config" / "meclaw" / "lanes")
    try:
        for line in path.read_text().splitlines():
            words = line.split("#", 1)[0].split()
            if len(words) >= 2 and words[0] == lane:
                return None if words[1] == "local" else words[1]
    except OSError:
        return None
    return None


def load_command(n, cap_s):
    """The remote shell line: n niced busy loops, gone at stdin EOF or after
    cap_s -- the gate triage's own line (`flakes.busy_loop_command`)."""
    return flakes.busy_loop_command(n, cap_s)


def _shell(target, cmd, env=None, unset=()):
    """The argv running `cmd` on `target`: ssh to a lane, `bash -c` for LOCAL
    -- through the kit's narrow point `lane.ssh_argv` (re-check tv-bn M7e
    B-M1: this way built its own ssh argv, and `lane_unittest_argv`'s words
    passed the word check by). LaneError(2) before any process for a word no
    list of `lane_sync.sh` names."""
    return _lanes.ssh_argv(target, cmd, env=env, unset=unset)


class LoadError(Exception):
    """The asked load does not run: the triage refuses (exit 2), never measures without it."""


def lane_probe(target):
    """`(load1, nproc)` of the lane, read over ssh; LoadError when ssh fails."""
    r = subprocess.run(_shell(target, "cut -d' ' -f1 /proc/loadavg; nproc"),
                       capture_output=True, text=True, timeout=60)
    words = r.stdout.split()
    if r.returncode != 0 or len(words) < 2:
        raise LoadError(f"probe of {target} failed (exit {r.returncode}): {r.stderr.strip()[:200]}")
    return float(words[0]), int(words[1])


def cpu_delegated(target):
    """True when the lane's user manager holds the cpu controller (a `CPUQuota`
    in a `systemd-run --user --scope` bites only then); LoadError when ssh fails."""
    r = subprocess.run(_shell(target, "u=$(id -u); cat /sys/fs/cgroup/user.slice/user-$u.slice/"
                              "user@$u.service/cgroup.controllers"),
                       capture_output=True, text=True, timeout=60)
    if r.returncode != 0:
        raise LoadError(f"cgroup probe of {target} failed (exit {r.returncode}): {r.stderr.strip()[:200]}")
    return "cpu" in r.stdout.split()


def _throttle(target, pct, load):
    """`(delegated, load, note)`: the quota where `target` delegates the cpu
    controller, else `nproc` busy loops (at least the asked `load`) and a NOTE."""
    if cpu_delegated(target):
        return True, load, f"throttle: CPUQuota={pct}% on {target} (cpu controller delegated)"
    _, nproc = lane_probe(target)
    return False, max(load, nproc), \
        f"NOTE throttle: cpu controller not delegated on {target} -- fallback --load {nproc}"


def plan_production_load(target, asked_load=0, throttle=0, factor=None):
    """`(n, note)` of `--production-load` (K12): the busy loops that bring the
    lane to about 30 % busy -- load1 measured over ssh, `flakes.lane_loops`,
    the measurement in the header. A lane already fuller is a LoadError (wait,
    or take another lane), and so is any other load or a quota beside it."""
    if asked_load or throttle or factor is not None:
        raise LoadError("--production-load sets the load itself: no --load, --throttle "
                        "or --time-factor beside it (K12: full quota)")
    load1, nproc = lane_probe(target)
    n = flakes.lane_loops(load1, nproc)
    pct = round(flakes.T2_LANE_BUSY * 100)
    if n is None:
        raise LoadError(f"lane {target} is busier than {pct} %: load1 {load1:.1f} of {nproc} vCPU "
                        f"-- wait, or take another lane (K12)")
    return n, (f"production load: lane about {pct} % busy -- load1 {load1:.1f} of {nproc} vCPU "
               f"measured, {n} busy loops (K12)")


def time_factor_env(raw, throttle):
    """`{MECLAW_TEST_TIME_FACTOR: raw}` of a quota run, `{}` without one
    (OR-T12-13). The factor belongs to a run under `--throttle` only; a value
    that is no finite number >= 1 is a LoadError (fail-closed, the test side
    refuses it the same way)."""
    if raw is None:
        return {}
    if not throttle:
        raise LoadError("--time-factor belongs to a --throttle run only")
    top = max_time_factor()
    try:
        # Python reads `3_0` as 30, Rust refuses it: no underscores (re-check H1).
        f = float("nan") if "_" in raw else float(raw)
    except ValueError:
        f = float("nan")
    if not (1.0 <= f <= top):
        raise LoadError(f"--time-factor {raw!r}: a finite number from 1 to {top:g}")
    return {"MECLAW_TEST_TIME_FACTOR": raw}


def max_time_factor():
    """`MAX_TIME_FACTOR` of `meclaw_testing::deadline` -- read from that one
    line, so the bound is stated once (tv-br M10o, re-check B-M3). A tree
    without the line is a LoadError, never an open bound."""
    src = pathlib.Path(__file__).resolve().parent.parent / "crates/meclaw-testing/src/deadline.rs"
    m = re.search(r"^pub const MAX_TIME_FACTOR: f64 = ([0-9.]+);$", src.read_text(), re.M)
    if not m:
        raise LoadError(f"MAX_TIME_FACTOR not found in {src}")
    return float(m.group(1))


def plan_throttle(lane, pct, load):
    """`(env, load, note)` of a throttled lane run (`MECLAW_LANE_CPU_QUOTA`, lane_sync.sh)."""
    target = lane_target(lane)
    if not target:
        raise LoadError(f"no ssh target for lane {lane}")
    delegated, load, note = _throttle(target, pct, load)
    return ({"MECLAW_LANE_CPU_QUOTA": str(pct)} if delegated else {}), load, note


def plan_throttle_local(pct, load, target=LOCAL):
    """`(argv prefix, load, note)` of a throttled Python run on `target` (the run's host)."""
    delegated, load, note = _throttle(target, pct, load)
    prefix = ["systemd-run", "--user", "--scope", "--quiet", "-p", f"CPUQuota={pct}%", "--"]
    return (prefix if delegated else []), load, note


def alive(target, pids):
    """How many of `pids` still run on the lane."""
    r = subprocess.run(_shell(target, "ps -o pid= -p " + ",".join(pids)),
                       capture_output=True, text=True, timeout=60)
    return len(set(r.stdout.split()) & set(pids))


def start_load(lane, n, cap_s, target=None):
    """The running load as a dict (proc, target, pids, load before), or None when none is asked.

    `target` LOCAL runs the loops on this host. Fail-closed: no ssh target, a
    dead ssh or fewer loops than asked is a LoadError, not a run without load."""
    if n <= 0:
        return None
    target = target or lane_target(lane)
    if not target:
        raise LoadError(f"no ssh target for lane {lane}")
    before, nproc = lane_probe(target)
    proc = subprocess.Popen(_shell(target, load_command(n, cap_s)),
                            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                            text=True)
    line = proc.stdout.readline().split()
    pids = line[1:] if line[:1] == ["LOAD"] else []
    load = {"proc": proc, "target": target, "n": n, "pids": pids, "before": before, "nproc": nproc}
    if proc.poll() is not None or len(pids) != n or alive(target, pids) != n:
        stop_load(load)
        raise LoadError(f"load on {target} did not start: {len(pids)} of {n} loops "
                        f"(ssh exit {proc.returncode})")
    return load


def check_load(load):
    """The header line of a loaded run, read just before the load stops; LoadError when a loop died."""
    if load is None:
        return "load: none"
    during, _ = lane_probe(load["target"])
    up = alive(load["target"], load["pids"])
    if load["proc"].poll() is not None or up != load["n"]:
        raise LoadError(f"load on {load['target']} died during the run: {up} of {load['n']} loops alive")
    return (f"load: {load['n']} busy loops on {load['target']} ({load['nproc']} cores), alive {up}/{load['n']} "
            f"at the end; load1 before {load['before']:.1f}, at the end {during:.1f}")


def stop_load(load):
    if load is None:
        return
    proc = load["proc"]
    try:
        proc.stdin.close()
        proc.wait(timeout=30)
    except (OSError, subprocess.TimeoutExpired):
        proc.terminate()


def head_branch(repo=None):
    """The tree's branch: "" on a detached HEAD, None without a readable HEAD.

    Read from the tree's HEAD file, no git process: the run's own subprocess
    calls stay the only ones."""
    git = pathlib.Path(repo or REPO) / ".git"
    try:
        if git.is_file():
            git = (git.parent / git.read_text().split("gitdir:", 1)[1].strip()).resolve()
        head = (git / "HEAD").read_text().strip()
    except (OSError, IndexError):
        return None
    return head[len("ref: refs/heads/"):] if head.startswith("ref: refs/heads/") else ""


def strand_of(strand=None, repo=None):
    """THE strand of the call (`gitenv.strand_of`, GH #1199 f): `--strand` >
    the branch `<wave>/<strand>` > none, with the way out in `.why`."""
    from scripts.pipeline import gitenv
    return gitenv.strand_of(head_branch(repo), strand or "")


def strand_of_branch(repo=None):
    """The strand of the tree's branch (`<wave>/<strand>`, as `strand.sh`), else None."""
    return strand_of(None, repo).name or None


RUN_LOG = re.compile(r"^strand: test log (\S.*?)\s*$", re.M)


def run_log(output):
    """The log `strand.sh test` names for its own run (`strand: test log <path>`), else None.

    Kit find tv-s1-legacy-a2 (Welle Testing, fix6): the output of a red run is
    only the log's 20-line tail (run 040123Z: "ran 29 of 60" for nextest's
    60/60), a green run prints no result line, and the fallback "newest
    `test.log` of the strand after the start" took a parallel or an earlier
    run (044609Z: 61 runs, 1 red for nextest's 40/40 green). Only the log
    the run names is its log; there is no guess. Re-check fix7 H1: the run
    names it before any output of the lane (`gate.py` `_test`), so the first
    such line is the run's -- a later one is output of a test."""
    found = RUN_LOG.findall(output or "")
    return pathlib.Path(found[0]) if found else None


def short_runs(cases, stats, count):
    """The cases that ran fewer than `count` times: a wrong ID or a lost line, never a result."""
    return [c for c in cases if (stats.get(c) or {}).get("runs", 0) < count]


# --- the Python class (`--unittest`) -------------------------------------------------

_PY_DURATION = re.compile(r"^\s*([0-9.]+)s\s+(\w+) \(([\w.-]+)\)\s*$")
_PY_FRAME = re.compile(r'^\s*File "([^"]+)", line (\d+)')
_PY_BLOCK_END = re.compile(r"^(?:={20,}|-{20,})$")


def unittest_argv(ids, seed=None):
    """One run of all `ids` in one process: T3 (`shuffle.py --seed`) or unittest's order."""
    if seed is not None:
        return flakes.t3_unittest_argv(ids[0], seed) + list(ids[1:])
    return flakes.unittest_base_argv() + ["-v", "--durations", "0"] + list(ids)


def unittest_file(tid, roots=None):
    """The source file of a unittest id: the longest module prefix found under
    the repo, else None. The repo is the run's cwd; the kit's Python (`-I`)
    takes no PYTHONPATH, so an id from elsewhere does not run (M7p)."""
    parts = tid.split(".")
    roots = roots or [REPO]
    for n in range(len(parts), 0, -1):
        for root in roots:
            cand = root.joinpath(*parts[:n]).with_suffix(".py")
            if cand.is_file():
                return cand
    return None


def _frame(path, line):
    """`path:line` of a traceback frame, relative to the repo where it lies in it."""
    p = pathlib.Path(path)
    try:
        return f"{p.resolve().relative_to(REPO)}:{line}"
    except ValueError:
        return f"{p.name}:{line}"


def unittest_asserts(text):
    """`{test id: path:line}` -- the last traceback frame outside unittest of each FAIL/ERROR block."""
    out, tid, last = {}, None, None
    for line in text.splitlines():
        h = flakes._UT_HEAD.match(line)
        if h:
            if tid and last:
                out[tid] = last
            tid, last = flakes.unittest_id(h.group(2), h.group(3)), None
            continue
        if tid is None:
            continue
        f = _PY_FRAME.match(line)
        if f and "/unittest/" not in f.group(1):
            last = _frame(f.group(1), f.group(2))
        elif _PY_BLOCK_END.match(line) and last:
            out[tid], tid, last = last, None, None
    if tid and last:
        out[tid] = last
    return out


def unittest_errors(text):
    """`{test id: error text}` -- what follows the last traceback frame of each
    FAIL/ERROR block (the exception and its message, all of its lines); the
    blocks of one id (its subtests) one after the other (re-check fix15 H4)."""
    out, tid, body = {}, None, []

    def close():
        if tid is None:
            return
        last = max((i for i, ln in enumerate(body) if _PY_FRAME.match(ln)), default=None)
        rest = body if last is None else body[last + 1:]
        while rest and rest[0].startswith("    "):   # the frame's code and carets
            rest = rest[1:]
        text = "\n".join(rest).strip("\n") or "(no error text)"
        out[tid] = out[tid] + "\n" + text if tid in out else text

    head_sep = False
    for line in text.splitlines():
        h = flakes._UT_HEAD.match(line)
        if h:
            close()
            tid, body, head_sep = flakes.unittest_id(h.group(2), h.group(3)), [], True
            continue
        if tid is None:
            continue
        if _PY_BLOCK_END.match(line):
            if head_sep:
                head_sep = False
                continue
            close()
            tid, body = None, []
            continue
        head_sep = False
        body.append(line.rstrip())
    close()
    return out


def red_run_entry(label, text, ids, rc):
    """One red run of the result: its label, then the error text of each red
    test the `ids` name, else the end of its output (kit find C4)."""
    errors = {t: e for t, e in unittest_errors(text).items()
              if any(t == i or t.startswith(i + ".") for i in ids)}
    if errors:
        return [f"- {label}: {t}" + "".join("\n" + ("    " + ln).rstrip() for ln in e.splitlines())
                for t, e in sorted(errors.items())]
    tail = [ln for ln in text.strip().splitlines() if ln.strip()][-RED_TAIL_LINES:] or ["no output"]
    return [f"- {label}, exit {rc} without a named red test (last lines of its output)"
            + "".join("\n" + ("    " + ln).rstrip() for ln in tail)]


# A red run with no named red test keeps the end of its output: the hang
# marker, the summary, the exit -- what names why it was red.
RED_TAIL_LINES = 10


def merge_unittest_run(stats, text, ids, rc=0):
    """Count one run's output into `stats` ({test id: runs/red/secs/asserts}) for the
    tests the `ids` name; True when the run is red (a red test, or a summary
    that counts more failures than the output names, or none, or an exit code
    other than 0 -- a hang killed after `OK`, `os._exit` after it, the lane's
    `timeout` 124: fail-closed, re-check fix3 M1). A skipped test counts in
    `skipped`, never in `runs`: a skip is no green (GH #1199 vor-batch, a lane
    without `rustc` skipped a test its class then called green)."""
    parsed = flakes.parse_unittest_output(text)
    summary = flakes.unittest_summary(text)
    secs = {flakes.unittest_id(m.group(2), m.group(3)): float(m.group(1))
            for m in map(_PY_DURATION.match, text.splitlines()) if m}
    asserts = unittest_asserts(text)
    for kind in ("passed", "failed", "skipped"):
        for tid in parsed[kind]:
            if not any(tid == i or tid.startswith(i + ".") for i in ids):
                continue
            st = stats.setdefault(tid, {"runs": 0, "red": 0, "secs": [], "asserts": {}})
            if kind == "skipped":
                st["skipped"] = st.get("skipped", 0) + 1
                continue
            st["runs"] += 1
            if tid in secs:
                st["secs"].append(secs[tid])
            if kind == "failed":
                st["red"] += 1
                loc = asserts.get(tid, "-")
                st["asserts"][loc] = st["asserts"].get(loc, 0) + 1
    return (bool(parsed["failed"]) or summary is None or summary[1] > len(parsed["failed"])
            or rc != 0)


def bind_lane(host, wave, strand, holder=None):
    """The lane run binds to a held token like `strand.sh test` (`token check
    --lane`; MECLAW_STRAND_TOKEN_SKIP passes there, logged); LoadError otherwise."""
    argv = [str(REPO / "scripts" / "strand.sh"), "token", "check", "--pid", str(os.getpid()), "--lane", host]
    argv += (["--wave", wave] if wave else []) + (["--strand", strand] if strand else [])
    argv += ["--holder", holder] if holder else []
    r = subprocess.run(argv, cwd=REPO, capture_output=True, text=True, timeout=120)
    if r.stdout.strip():
        print(r.stdout.strip(), file=sys.stderr)
    if r.returncode != 0:
        raise LoadError(f"lane {host} not bound (token check exit {r.returncode}): "
                        f"{(r.stderr or r.stdout).strip()[-300:]}")


def push_tree(host, strand):
    """The lane's `wt/<strand>` holding this tree's commit + overlay: the
    pipeline's own transport (`pipeline/lane.py push`, `cargo=False` -- where
    a gate runs its Python stations); LoadError when the lane refuses."""
    from scripts.pipeline import lane as lanes
    try:
        return lanes.push(lanes.find_lane(host), REPO, strand, cargo=False)["tree"]
    except lanes.LaneError as e:
        raise LoadError(f"push to lane {host} failed (code {e.code}): {e}") from None


# Kit find C4 (Welle Testing, tv-bn-layout-c): the lane run did not carry
# MECLAW_TESTING_RULES_RULING; a test the ruling turns green here was red there
# in 20 of 20 runs (build03), at the same assert as a local run without it. The
# variables that change a verdict of the kit's own checks travel with the run
# (testing_bans RULING_ENV, precheck's MECLAW_ALLOW_BUMP and MECLAW_GATE_MODE).
# An ALLOWLIST by exact name, never a pattern: a pattern over the environment
# is how a key would leave this machine (the rule of `lane.GOLDEN_SWITCHES`).
# GH #1210 (tv-bn M5): the kit's ONE list, `lane_sync.sh VERDICT_ENV` -- this
# copy of three names lacked MECLAW_REQUIRE_BROWSERS.
LANE_UNITTEST_ENV = _lanes.verdict_env()


def lane_unittest_argv(target, tree, argv, hang_s, environ=None):
    """One Python run on the lane: an ssh call into the pushed tree; `timeout`
    ends it there at the hang marker (killing the local ssh alone would not).
    Paths of this repo (T3's `shuffle.py`) become relative to the lane's tree;
    the verdict's environment (LANE_UNITTEST_ENV) goes along through `env`."""
    top = str(REPO) + "/"
    argv = [w[len(top):] if w.startswith(top) else w for w in argv]
    # GH #1209 (re-check tv-bn M7 M4): this run sets the browser switch itself
    # unless the caller opted out (`lane.test_run_env`); before, it only handed
    # it on, and a driver on a lane without its engine SKIPped green.
    environ, _ = _lanes.test_run_env(environ)
    # A name unset here is unset there too: the lane's login environment does
    # not decide the verdict (re-check tv-kit fix15 H5). The narrow point
    # builds both from data (orchestrator M7g), never from composed words.
    env = {n: environ[n] for n in LANE_UNITTEST_ENV if n in environ}
    unset = [n for n in LANE_UNITTEST_ENV if n not in environ]
    return _shell(target, f"cd {shlex.quote(tree)} && timeout -k 10 {int(hang_s)} {shlex.join(argv)}",
                  env=env, unset=unset)


def _run_unittest(argv, hang_s):
    """One run in its own process group from the repo root; past `hang_s` the
    group is killed and the run returns with HANG on stderr (red, no summary)."""
    proc = subprocess.Popen(argv, cwd=REPO, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                            text=True, errors="replace", start_new_session=True)
    try:
        out, err = proc.communicate(timeout=hang_s)
    except subprocess.TimeoutExpired:
        os.killpg(proc.pid, signal.SIGKILL)
        out, err = proc.communicate()
        err += f"\nHANG: run killed after {hang_s}s\n"
    return subprocess.CompletedProcess(argv, proc.returncode, out, err)


def unittest_label(tid):
    return ".".join(tid.split(".")[-2:])


def isolated():
    """Is this the kit's Python (`-I`)? The lane's narrow point refuses any other."""
    return bool(sys.flags.isolated)


def lane_of(a):
    """The lane this run is on ("" for `--host local`): it picks the member
    lease of a batch holder (re-check RC-K3 M4)."""
    host = getattr(a, "host", "") or ""
    return "" if host == LOCAL else host


def holder_of(a):
    """The lease key of this run (`<wave>/<strand>[:<holder>]`): the one chain
    `gitenv.strand_of` (GH #1199 f) over the tree's HEAD file, without the git
    process of `gate.holder_key`."""
    from scripts.pipeline import gitenv
    key = gitenv.strand_of(head_branch(), a.strand or "", a.wave or "").key
    return "%s:%s" % (key, a.holder) if key and a.holder else key


def refused(a, msg):
    """GH #1199 (kit find V2-A8): a refusal before the run is exit 2 and one
    line on stderr AND stdout (one log or two), in `--out`, at the lease --
    never only a stderr line a background wrapper may not keep."""
    return progress.refuse("flake_triage", msg, out=sys.stdout, err=sys.stderr,
                           holder=holder_of(a), also=a.out, lane=lane_of(a))


def store_or_reason(path):
    """(the pipeline store at `path` open, "") or (None, why it cannot be
    read). Re-check v2-fix-e M1: a store error before the run was a
    traceback; it is a refusal line like every other."""
    from scripts.pipeline import store
    try:
        return store.open(path), ""
    except (sqlite3.Error, OSError, RuntimeError, store.MigrationDrift) as e:
        return None, "pipeline store %s: %s" % (path, e)


def lease_missing(a, env=None):
    """V2-A8b (re-check test-cc2, 14.10.): `--strand v2-fix-b` from a tree on
    branch `.../v2-fix-b-hd1` found no lease, ran 0 passes and said nothing.
    With the token pool armed, a lane run whose holder (`holder_of`) holds no
    lease is refused before anything runs; the reason names the holder looked
    for and the branch. "" when it may run: the lease is there, the pool is
    not armed, there is no store, or MECLAW_STRAND_TOKEN_SKIP is set (logged
    by the kit's own token check)."""
    env = os.environ if env is None else env
    if env.get("MECLAW_STRAND_TOKEN_SKIP"):
        return ""
    from scripts.pipeline import schedule, store
    holder, path = holder_of(a), pathlib.Path(env.get("MECLAW_PIPELINE_DB") or store.default_path())
    if not path.exists():
        return ""
    db, bad = store_or_reason(path)
    if db is None:
        return bad
    try:
        if not schedule._armed(db):
            return ""
        st = schedule._load(db)
        # a batch holder `<wave>/S1+S2` runs on a member's lease, as in the kit
        if holder and schedule._holder(st, schedule._batch_speaker(st, holder, a.host)):
            return ""
    except sqlite3.Error as e:
        return "pipeline store %s: %s" % (path, e)
    finally:
        store.close(db)
    return ("no lease for holder %r (branch %r) in the armed token pool -- take one "
            "(`strand.sh token take`) or name the holder of the lane (--wave/--strand/--holder)"
            % (holder, head_branch() or ""))


def no_pass(a, n):
    """V2-A8b: a series in which no test ran in any of its `n` passes is no
    result -- one line on stderr and stdout, in `--out`, at the lease, naming
    the holder and the branch; exit 1 (not run is RED, kit find (i))."""
    return progress.refuse("flake_triage", "no test ran in any of %d passes -- holder %r (branch %r)"
                           % (n, holder_of(a), head_branch() or ""), out=sys.stdout, err=sys.stderr,
                           holder=holder_of(a), also=a.out, code=1, word="NO PASS", lane=lane_of(a))


def say_pass(a, i, n, done, red):
    """One pass line per run into the log at once (flushed) and at the lease."""
    line = progress.pass_line(i, n, done, red)
    progress.emit(line, sys.stderr, sys.stdout)
    progress.beat(holder_of(a), line, lane=lane_of(a))


def main_unittest(a):
    """`--unittest`: the Python class on the lane `--host` (see the module docstring)."""
    ids = list(a.tests)
    if a.log:
        # re-check v2-fix-e M1: a usage refusal is one on both streams too
        return refused(a, "--log reads nextest logs; the Python class runs")
    n = a.stress_count
    stats, header, unexplained, red_runs = {}, [], 0, []
    if not a.scan_only:
        seed = None if a.no_shuffle else (a.seed if a.seed is not None else random.randrange(1, 10 ** 6))
        prefix, n_load, note, tree = [], a.load, "", None
        try:
            if a.throttle < 0:
                raise LoadError(f"--throttle {a.throttle}: a positive percent")
            target = LOCAL if a.host == LOCAL else lane_target(a.host)
            if not target:
                raise LoadError(f"no ssh target for lane {a.host}")
            if target != LOCAL:
                # A detached tree has no branch to name it (GH #1199 f):
                # `--strand` wins, without it the way out, exit 2.
                who = strand_of(a.strand)
                if not who.name:
                    raise LoadError("the lane's tree needs a strand name: " + who.why)
                strand = a.strand or who.name
                bind_lane(a.host, a.wave, a.strand, a.holder)
                tree = push_tree(a.host, strand)
                header.append(f"host: {a.host} ({target}), tree {tree}")
                opt_out = _lanes.test_run_env()[1]
                if opt_out:
                    header.append(opt_out)
            if a.production_load:
                n_load, note = plan_production_load(target, a.load, a.throttle, a.time_factor)
            elif a.throttle:
                prefix, n_load, note = plan_throttle_local(a.throttle, a.load, target)
            load = start_load(a.host, n_load, a.load_cap, target=target)
        except (LoadError, _lanes.LaneError) as e:
            return refused(a, str(e))
        reds = 0
        try:
            for i in range(n):
                s = None if seed is None else seed + i
                argv = prefix + unittest_argv(ids, s)
                if tree:
                    argv = lane_unittest_argv(target, tree, argv, a.hang_s)
                run = _run_unittest(argv, a.hang_s)
                text = run.stdout + run.stderr
                if tree:   # the assert column names repo paths, not the lane's
                    text = text.replace(tree.rstrip("/") + "/", str(REPO) + "/")
                named = sum(st["red"] for st in stats.values())
                run_red = merge_unittest_run(stats, text, ids, run.returncode)
                if run_red:
                    # Re-check fix2 M1: a run only its summary calls red (or that
                    # has none) named no red test -- it is red all the same.
                    unexplained += sum(st["red"] for st in stats.values()) == named
                    tail = (text.strip().splitlines() or ["no output"])[-1]
                    label = f"run {i + 1}/{n} red" + ("" if s is None else f", seed {s}")
                    print(f"flake_triage: {label} -- {tail}", file=sys.stderr)
                    # Kit find C4: the error text of a red run is the finding;
                    # it goes into the result, not only its last line to stderr.
                    red_runs += red_run_entry(label, text, ids, run.returncode)
                reds += run_red
                say_pass(a, i + 1, n, i + 1, reds)
            header.append(check_load(load))
        except (LoadError, _lanes.LaneError) as e:
            return refused(a, str(e))
        finally:
            stop_load(load)
        if note:
            header.insert(0, note)
        if unexplained:
            header.append(f"unexplained: {unexplained} of {n} runs red without a named red test")
        header.append("shuffle: off (unittest order)" if seed is None else
                      f"shuffle: seed {seed}..{seed + n - 1} (T3, scripts/pipeline/shuffle.py)")
    if a.scan_only:
        # Re-check fix6 H1: the same line as the nextest path -- exit 0 here is no verdict.
        header.append("scan only: no run, no verdict")
    cases = sorted(stats) or ids
    scans = {}
    for tid in cases:
        path = unittest_file(tid)
        scans[tid] = (render_scan(scan(path.read_text(), PY_WALL_CLOCK, "#"), kinds=PY_WALL_CLOCK)
                      if path else f"no file for {tid}")
    out = table(cases, stats, scans, label=unittest_label)
    if red_runs:
        out += "\n\nred runs:\n" + "\n".join(red_runs)
    if header:
        out = "\n".join(header) + "\n\n" + out
    print(out)
    if a.out:
        pathlib.Path(a.out).write_text(out + "\n")
    if a.scan_only:
        return 0
    short = [i for i in ids if not any(t == i or t.startswith(i + ".") for t in stats)]
    short += [t for t, st in stats.items() if st["runs"] < n]
    for name, st in sorted(stats.items()):
        if st.get("skipped"):   # GH #1199 vor-batch: not evidenced, the series stays RED
            print(f"flake_triage: {name} skipped in {st['skipped']} of {n} runs -- RED, not "
                  f"evidenced (a skip is no green)", file=sys.stderr)
    for name in short:
        # re-check M1: a skip has its own line above -- its id is right, no hint at it
        hint = "" if stats.get(name, {}).get("skipped") else ": check the id (module.Class.test)"
        print(f"flake_triage: {name} ran {stats.get(name, {}).get('runs', 0)} of {n} times "
              f"-- RED, not run{hint}", file=sys.stderr)
    if not any(st["runs"] for st in stats.values()):
        return no_pass(a, n)
    if short:
        return 1   # kit find (i): not run is RED, never exit 3 (`waits or held`)
    return 1 if unexplained or any(s["red"] for s in stats.values()) else 0


def pairs(words):
    """The `<binary_id> <name>` pairs; an odd list is a LoadError (re-check
    v2-fix-e M1: it was a SystemExit, one stderr line, and `main` turns it
    into a refusal on both streams)."""
    if len(words) % 2:
        raise LoadError("tests come as pairs <binary_id> <name>")
    return [(words[i], words[i + 1]) for i in range(0, len(words), 2)]


def run_relayed(argv, env):
    """`strand.sh test` (`argv`) with its stdout read as it comes, the result
    like `subprocess.run(..., stdout=PIPE)`. Re-check v2-fix-e I1 (GH #1199,
    kit find V2-A8): the child writes each pass line on its stdout and its
    stderr; the stdout copy stayed in the pipe until the end, so a wrapper
    keeping only this run's stdout (`flake_triage.py ... > log`) held no pass
    line while the series ran. A pass or refusal line goes on to this stdout
    at once when it is another file than stderr (there the child's stderr
    copy has landed already)."""
    r, w = os.pipe()
    got, relay = [], len(progress.distinct(sys.stderr, sys.stdout)) == 2

    def pump():
        with os.fdopen(r, "r", errors="replace") as fh:
            for ln in fh:
                got.append(ln)
                if relay and progress.RELAY.match(ln.rstrip("\n")):
                    progress.emit(ln.rstrip("\n"), sys.stdout)
    t = threading.Thread(target=pump, daemon=True)
    t.start()
    try:
        run = subprocess.run(argv, stdout=w, text=True, errors="replace", env=env)
    finally:
        os.close(w)
        t.join()
    if run.stdout is None:
        run.stdout = "".join(got)
    return run


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("tests", nargs="+", help="pairs <binary_id> <name>; with --unittest unittest ids")
    ap.add_argument("-C", dest="chdir", action="append", metavar="DIR",
                    help="run as if started in DIR (as git -C; GH #1199)")
    ap.add_argument("--unittest", action="store_true",
                    help="the Python class: unittest ids, run on the lane --host (or --host local)")
    ap.add_argument("--seed", type=int, help="--unittest: first T3 seed (run i: seed + i)")
    ap.add_argument("--no-shuffle", action="store_true", help="--unittest: unittest's own order")
    ap.add_argument("--hang-s", type=int, default=HANG_S, help="--unittest: hang marker of one run")
    ap.add_argument("--host", default="build03")
    ap.add_argument("--wave", help="passed on to strand.sh test (the wave of the strand)")
    ap.add_argument("--strand", help="passed on to strand.sh test (the strand holding the lane)")
    ap.add_argument("--holder", help="passed on to strand.sh test (the strand's second lease, GH #1146)")
    ap.add_argument("--stress-count", "--runs", type=int, default=30)
    ap.add_argument("--run-ignored", action="store_true",
                    help="run #[ignore] tests too (nextest --run-ignored all); without it they run 0 times: RED")
    ap.add_argument("--throttle", type=int, default=0,
                    help="CPUQuota percent of the lane's test run (100 = one core)")
    ap.add_argument("--time-factor", dest="time_factor",
                    help="MECLAW_TEST_TIME_FACTOR of a --throttle run (OR-T12-13): every hang "
                         "mark x F, never a verdict; a finite number from 1 to MAX_TIME_FACTOR "
                         "(deadline.rs), named in the header")
    ap.add_argument("--load", type=int, default=0, help="busy loops on the lane during the run")
    ap.add_argument("--production-load", action="store_true",
                    help="the lane about 30 %% busy (K12): load1 measured, busy loops up to "
                         "0.3 x vCPU, full quota; a fuller lane is refused (wait or take another)")
    ap.add_argument("--load-cap", type=int, default=3600, help="seconds the load lives at most")
    ap.add_argument("--scan-only", action="store_true")
    ap.add_argument("--log", help="build the table from this log instead of running")
    ap.add_argument("--out", help="also write the table to this file")
    a = ap.parse_args(argv)
    # GH #1199 (b): `strand.sh test` below reads strand and lease from the
    # cwd's branch; `-C` is the `cd` an agent shell cannot keep.
    from scripts.pipeline import gitenv
    gitenv.enter_dirs(a.chdir, "flake_triage")
    from scripts.pipeline import schedule
    if schedule.strand_colon_error(a.strand):
        # kit find tv-br M5 (M7d2): the old form went whole into the lane push
        ap.error(schedule.strand_colon_error(a.strand))
    if a.stress_count < 1 and not (a.scan_only or a.log):
        # Re-check fix5 M2: zero runs prove nothing -- `short_runs(.., 0)` named no case.
        return refused(a, f"--stress-count {a.stress_count}: at least one run")
    if not (a.scan_only or a.log) and a.host != LOCAL and not isolated():
        # V2-A8: started as `python3 scripts/flake_triage.py`, the run reached
        # the lane's narrow point (LaneError, exit 2) after the token check.
        # `--host local` reaches no lane, so no narrow point (re-check v2-fix-e M4).
        return refused(a, "started without -I: the lane's narrow point runs only in the kit's "
                          "Python -- start scripts/flake_triage.py itself (#!/usr/bin/python3 -I) "
                          "or /usr/bin/python3 -I scripts/flake_triage.py")
    if not (a.scan_only or a.log) and a.host and a.host != LOCAL:
        why = lease_missing(a)
        if why:
            return refused(a, why)
    if a.unittest:
        return main_unittest(a)
    try:
        cases = pairs(a.tests)
    except LoadError as e:
        return refused(a, str(e))
    scans = {}
    for case in cases:
        path = test_file(*case)
        scans[case] = render_scan(scan(path.read_text())) if path.exists() else f"no file {path}"
    stats, text, header, run_exit, no_log = {}, "", "", 0, ""
    if a.log:
        text = pathlib.Path(a.log).read_text(errors="replace")
    elif not a.scan_only:
        env, n_load, note = None, a.load, ""
        try:
            if a.throttle < 0:
                raise LoadError(f"--throttle {a.throttle}: a positive percent")
            if a.production_load:
                target = lane_target(a.host)
                if not target:
                    raise LoadError(f"no ssh target for lane {a.host}")
                n_load, note = plan_production_load(target, a.load, a.throttle, a.time_factor)
            factor = time_factor_env(a.time_factor, a.throttle)
            if a.throttle:
                quota, n_load, note = plan_throttle(a.host, a.throttle, a.load)
                # tv-br M10q-a (re-check R K3): the fallback load holds no
                # quota, so a factor there would stretch every mark unseen;
                # refused before any load starts (the Rust marks refuse it too:
                # no quota mark from the lane transport).
                if factor and "MECLAW_LANE_CPU_QUOTA" not in quota:
                    # GH #1199: this refusal now reaches stdout too (the log); it
                    # never says "quota run" there -- that marks a header with one.
                    raise LoadError("--time-factor needs a CPU quota, and %s has none "
                                    "(cpu controller not delegated): a factor belongs only "
                                    "to a run under a CPU quota (OR-T12-13)" % a.host)
                env = dict(os.environ, **quota, **factor)
                if factor:
                    note += "\ntime factor: MECLAW_TEST_TIME_FACTOR=%s (quota run)" % a.time_factor
            load = start_load(a.host, n_load, a.load_cap)
        except (LoadError, _lanes.LaneError) as e:
            return refused(a, str(e))
        try:
            # GH #1199 (V2-A8): stderr passes through, stdout is read as it
            # comes -- the pass lines `strand.sh test` writes per stress
            # iteration (`gate.TestProgress`) reach either stream's log at once.
            run = run_relayed(strand_test_argv(cases, a.host, a.stress_count, a.wave, a.strand,
                                               ignored=a.run_ignored, holder=a.holder), env)
            header = check_load(load)
            if note:
                header = note + "\n" + header
        except (LoadError, _lanes.LaneError) as e:
            return refused(a, str(e))
        finally:
            stop_load(load)
        text = (run.stdout or "") + (run.stderr or "")
        run_exit = run.returncode
        log = run_log(text)
        if log is not None and log.is_file():
            text = log.read_text(errors="replace")
        else:
            no_log = "unexplained: run names no test log" + ("" if log is None else f" ({log} is gone)")
        print(text.strip().splitlines()[-1] if text.strip() else "flake_triage: no output", file=sys.stderr)
    if text:
        stats = parse_log(text, cases)
    iterations, why = (0, "") if a.scan_only else nextest_summary(text)
    if a.log and not a.scan_only and not why:
        iterations, why = runner_verdict(text, iterations)   # fix8 M1: the runner's line decides
    named_red = any(s["red"] for s in stats.values())
    unexplained = (run_exit != 0 or bool(why)) and not named_red or bool(no_log)
    if no_log:
        # fix6: without its own log the output is a tail or nothing -- no verdict, red.
        header = (header + "\n" if header else "") + no_log
    elif run_exit != 0 and not named_red:
        # Re-check fix4 M1: a run whose exit is not 0 is red, even with green
        # output (lane `timeout` 124, dropped ssh 255, a test-tier.sh error 1,
        # exit 3) -- the same rule as the Python class (TV-F7).
        header = (header + "\n" if header else "") + f"unexplained: run exit {run_exit}"
    elif why and not named_red:
        # Re-check fix5 M2: the log's summary stands in for the exit -- green
        # only with a positive one (a red iteration without a RESULT line, a
        # log cut by the lane's `timeout`).
        header = (header + "\n" if header else "") + \
            f"unexplained: {'log' if a.log else 'run'} {why}"
    if text and _lanes.opt_out_in(text):
        # Re-check tv-bn M7c M1: the Rust/TV-F7 report named no opt-out (only
        # the Python class did); the run's log carries lane_sync.sh's NOTE.
        header = (header + "\n" if header else "") + "browsers: " + _lanes.opt_out_in(text) + \
            " -- a browser driver without an engine SKIPs green, no browser proof"
    if a.scan_only:
        header = (header + "\n" if header else "") + "scan only: no run, no verdict"
    out = table(cases, stats, scans)
    if header:
        out = header + "\n\n" + out
    print(out)
    if a.out:
        pathlib.Path(a.out).write_text(out + "\n")
    if not a.scan_only:
        want = max(iterations, 1) if a.log else a.stress_count   # fix5 M2/fix8 M1: summary = runner n
        short = short_runs(cases, stats, want)
        for bid, name in short:
            print(f"flake_triage: {bid} {name} ran {stats.get((bid, name), {}).get('runs', 0)} "
                  f"of {want} times -- RED, not run: check the ID against the run's "
                  f"`Starting` line" + ("" if a.run_ignored else
                                        "; an #[ignore] test runs only with --run-ignored"),
                  file=sys.stderr)
        if not a.log and not any((stats.get(c) or {}).get("runs", 0) for c in cases):
            return no_pass(a, a.stress_count)
        if short:
            return 1   # kit find (i): not run is RED, never exit 3 (`waits or held`)
    return 1 if unexplained or any(s["red"] for s in stats.values()) else 0


if __name__ == "__main__":
    sys.exit(main())
