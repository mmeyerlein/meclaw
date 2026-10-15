#!/usr/bin/env python3
"""Stop watch beside a long run (GH #1075): stop it as soon as its target is lost.

A run longer than an hour writes one JSON line per finished unit into a history
file:

    {"unit": "<id>", "class": "<class>", "result": "ok" | "fail"}

This watcher reads the file beside the run and, after every new line, decides
deterministically -- no judgement call -- whether to stop the run:

* **unreachable** -- even if every remaining unit up to `--total` lands in the
  same class and passes, that class ends below `--target`. The bound assumes
  the best case for each class on its own, so it never stops a run that could
  still make it.
* **below** -- a class with at least `--min-units` units (default 20) has the
  upper bound of its 95 % Wilson interval below `--target`: the run is
  statistically below the target even though it could still reach it.

Stopping means: SIGTERM to the process group of `--pid` -- the PID the caller
remembered for its run chain, never a name or a `pgrep` match -- the reason and
the per-class table (class, n, ok, rate, best case, Wilson upper bound) into
`--report`, exit 1. The run starts in its own process group (`setsid <run>`, or
`start_new_session=True`), so the signal reaches every process of the chain --
a shell and the child it waits on alike -- and nothing outside it; a `--pid`
that does not lead its own group, or leads the watcher's, is refused. A verdict
that comes with the last of the `--total` units sends no signal: the run has
produced everything and would only lose its wrap-up; report and exit 1 as usual.
Without a verdict the watcher runs until `--total` units are in (exit 0) or the
target process is gone (exit 3). A broken history line, a duplicate unit or more
units than `--total` is an instrument fault: report + exit 2, the run is not
signalled (a broken history says nothing about the run's rate).

WHY event-driven: a watcher that polls on a second-scale tick is the polling
the project bans (an idle tick costs a wake-up per second for hours and still
lags by up to a tick). On Linux the watcher blocks in `select()` on two kernel
objects -- an inotify watch on the history's directory (it also sees the file
being created) and a pidfd of the target (readable the moment it exits). Where
either is missing it falls back to a `tail -F --pid` child and blocks on its
pipe: the same follow semantics, with tail's own wait inside (GNU tail uses
inotify where it can). `MECLAW_STOP_WATCH_NO_INOTIFY=1` forces the fallback.

Rule and origin: plans/PREAMBLE.md section 6.9 (private) and
docs/development-rules.md section 7.
"""

import argparse
import ctypes
import ctypes.util
import json
import math
import os
import pathlib
import select
import signal
import subprocess
import sys
from fractions import Fraction

# Two-sided 95 % normal quantile.
Z95 = 1.959963984540054

EXIT_COMPLETE, EXIT_ABORT, EXIT_ERROR, EXIT_GONE = 0, 1, 2, 3
RESULTS = ("ok", "fail")

# inotify(7) masks: write, close-after-write, create, rename into the directory.
_IN_MODIFY, _IN_CLOSE_WRITE, _IN_MOVED_TO, _IN_CREATE = 0x2, 0x8, 0x80, 0x100


class HistoryError(Exception):
    """The history file is not what the contract says (instrument fault)."""


class Verdict:
    """A decision to stop: `kind` is `unreachable` or `below`."""

    def __init__(self, kind, cls, reason):
        self.kind, self.cls, self.reason = kind, cls, reason


def wilson_upper(ok, n, z=Z95):
    """Upper bound of the Wilson score interval for `ok` successes in `n`."""
    p = ok / n
    centre = p + z * z / (2 * n)
    spread = z * math.sqrt(p * (1 - p) / n + z * z / (4 * n * n))
    return (centre + spread) / (1 + z * z / n)


def parse_line(raw, lineno):
    """One history line as a dict, or `HistoryError` naming the line."""
    try:
        row = json.loads(raw)
    except ValueError as exc:
        raise HistoryError("line %d: not JSON (%s): %r" % (lineno, exc, raw[:80]))
    if not isinstance(row, dict):
        raise HistoryError("line %d: not a JSON object: %r" % (lineno, raw[:80]))
    for key in ("unit", "class"):
        if not isinstance(row.get(key), str) or not row[key]:
            raise HistoryError("line %d: missing or empty %r: %r" % (lineno, key, raw[:80]))
    if row.get("result") not in RESULTS:
        raise HistoryError("line %d: result must be ok|fail: %r" % (lineno, raw[:80]))
    return row


class Watch:
    """The per-class tally and the two stop rules."""

    def __init__(self, total, target, min_units=20):
        target = Fraction(str(target))
        if total < 1 or not 0 < target <= 1 or min_units < 1:
            raise ValueError("need --total >= 1, 0 < --target <= 1, --min-units >= 1")
        self.total, self.target, self.min_units = total, target, min_units
        self.counts = {}  # class -> [n, ok]
        self.units = set()

    @property
    def seen(self):
        return len(self.units)

    def add(self, row):
        """Count one parsed row; return a `Verdict` or None."""
        if row["unit"] in self.units:
            raise HistoryError("unit %r appears twice" % row["unit"])
        if self.seen >= self.total:
            raise HistoryError("more than --total %d units" % self.total)
        self.units.add(row["unit"])
        tally = self.counts.setdefault(row["class"], [0, 0])
        tally[0] += 1
        tally[1] += row["result"] == "ok"
        return self.judge()

    def best(self, cls):
        """The highest rate `cls` can still end with (exact)."""
        n, ok = self.counts[cls]
        rest = self.total - self.seen
        return Fraction(ok + rest, n + rest)

    def judge(self):
        # Every class, not only the one just counted: each new unit shrinks
        # the rest, so another class's best case can drop below the target.
        for cls in sorted(self.counts):
            n, ok = self.counts[cls]
            best = self.best(cls)
            if best < self.target:
                return Verdict("unreachable", cls, (
                    "class %s: even if all %d remaining units pass in it, its rate "
                    "is at most %.3f < target %.3f" % (
                        cls, self.total - self.seen, float(best), float(self.target))))
            if n >= self.min_units and wilson_upper(ok, n) < self.target:
                return Verdict("below", cls, (
                    "class %s: %d/%d, 95%% Wilson upper bound %.3f < target %.3f" % (
                        cls, ok, n, wilson_upper(ok, n), float(self.target))))
        return None

    def table(self):
        lines = ["%-24s %6s %6s %7s %7s %8s" % ("class", "n", "ok", "rate", "best", "upper95")]
        for cls in sorted(self.counts):
            n, ok = self.counts[cls]
            lines.append("%-24s %6d %6d %7.3f %7.3f %8.3f" % (
                cls, n, ok, ok / n, float(self.best(cls)), wilson_upper(ok, n)))
        return "\n".join(lines)


def _start_time(pid):
    """Field 22 of /proc/<pid>/stat (start time in ticks), or None if gone."""
    try:
        raw = pathlib.Path("/proc/%d/stat" % pid).read_text()
    except OSError:
        return None
    # The command name (field 2) may hold spaces and parens; split after it.
    return raw.rsplit(")", 1)[1].split()[19]


class Target:
    """The remembered PID of the run, leader of the run's own process group.
    Signals that group only, and never after a PID reuse."""

    def __init__(self, pid):
        if pid <= 1 or pid == os.getpid():
            raise ValueError("refusing --pid %d: it would not name exactly one "
                             "foreign process" % pid)
        self.pid = pid
        self.start = _start_time(pid)
        if self.start is None:
            raise ValueError("--pid %d: no such process" % pid)
        try:
            pgid = os.getpgid(pid)
        except ProcessLookupError:
            raise ValueError("--pid %d: no such process" % pid)
        if pgid != pid or pgid == os.getpgrp():
            raise ValueError("refusing --pid %d: not the leader of its own process group "
                             "(start the run with setsid)" % pid)
        self.pidfd = None
        if hasattr(os, "pidfd_open"):
            try:
                self.pidfd = os.pidfd_open(pid)
            except OSError:
                self.pidfd = None

    def same(self):
        return _start_time(self.pid) == self.start

    def terminate(self):
        """SIGTERM the run's process group if its leader is still the process we
        were given. While the leader lives its PID cannot name another group."""
        if not self.same():
            return False
        try:
            os.killpg(self.pid, signal.SIGTERM)
        except ProcessLookupError:
            return False
        return True


def _inotify(directory):
    """An inotify fd watching `directory`, or None where there is none."""
    if os.environ.get("MECLAW_STOP_WATCH_NO_INOTIFY") == "1":
        return None
    try:
        libc = ctypes.CDLL(ctypes.util.find_library("c") or "libc.so.6", use_errno=True)
        fd = libc.inotify_init1(os.O_CLOEXEC)
    except (OSError, AttributeError):
        return None
    if fd < 0:
        return None
    mask = _IN_MODIFY | _IN_CLOSE_WRITE | _IN_MOVED_TO | _IN_CREATE
    if libc.inotify_add_watch(fd, os.fsencode(directory), mask) < 0:
        os.close(fd)
        return None
    return fd


class _Reader:
    """Complete new lines of the history since the last call."""

    def __init__(self, path):
        self.path, self.offset, self.ino, self.buf = path, 0, None, b""

    def read(self):
        try:
            st = os.stat(self.path)
        except FileNotFoundError:
            return []
        if self.ino is not None and (st.st_ino != self.ino or st.st_size < self.offset):
            raise HistoryError("history was replaced or truncated under the watch")
        self.ino = st.st_ino
        with open(self.path, "rb") as fh:
            fh.seek(self.offset)
            data = fh.read()
        self.offset += len(data)
        *done, self.buf = (self.buf + data).split(b"\n")
        return [d.decode("utf-8", "replace") for d in done]


def follow(path, target):
    """Yield history lines as they appear, then `None` once the target is gone."""
    ifd = _inotify(os.path.dirname(os.path.abspath(path)))
    if ifd is not None and target.pidfd is not None:
        reader = _Reader(path)
        try:
            while True:
                yield from reader.read()  # watch set up first: nothing is missed
                ready, _, _ = select.select([ifd, target.pidfd], [], [])
                if ifd in ready:
                    os.read(ifd, 65536)
                if target.pidfd in ready:
                    yield from reader.read()
                    yield None
                    return
        finally:
            os.close(ifd)
    if ifd is not None:
        os.close(ifd)
    tail = subprocess.Popen(["tail", "-n", "+1", "-F", "--pid=%d" % target.pid, path],
                            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
    try:
        for raw in tail.stdout:  # blocks on the pipe; EOF = target gone
            yield raw.decode("utf-8", "replace").rstrip("\n")
        yield None
    finally:
        if tail.poll() is None:
            tail.terminate()
        tail.wait()


def write_report(path, head, args, watch, extra=""):
    text = "%s\nhistory: %s\nunits: %d/%d  target: %s  min-units: %d  pid: %d%s\n\n%s\n" % (
        head, args.history, watch.seen, watch.total, args.target, watch.min_units,
        args.pid, extra, watch.table())
    tmp = "%s.tmp" % path
    with open(tmp, "w", encoding="utf-8") as fh:
        fh.write(text)
    os.replace(tmp, path)


def run(args):
    try:
        watch = Watch(args.total, args.target, args.min_units)
        target = Target(args.pid)
    except ValueError as exc:
        print("stop_watch: %s" % exc, file=sys.stderr)
        return EXIT_ERROR
    lineno = 0
    try:
        for line in follow(args.history, target):
            if line is None:
                head = "stop_watch: TARGET GONE before --total was reached"
                write_report(args.report, head, args, watch)
                print(head)
                return EXIT_GONE
            lineno += 1
            verdict = watch.add(parse_line(line, lineno))
            if verdict is not None:
                head = "stop_watch: ABORT %s\nreason: %s" % (verdict.kind, verdict.reason)
                if watch.seen == watch.total:
                    note = " (not signalled: the run has produced all --total units)"
                elif target.terminate():
                    note = " (SIGTERM sent to the run's process group)"
                else:
                    note = " (not signalled: gone or PID reused)"
                write_report(args.report, head, args, watch, note)
                print(head)
                return EXIT_ABORT
            if watch.seen == watch.total:
                head = "stop_watch: COMPLETE, no stop rule fired"
                write_report(args.report, head, args, watch)
                print(head)
                return EXIT_COMPLETE
    except HistoryError as exc:
        write_report(args.report, "stop_watch: ERROR %s" % exc, args, watch)
        print("stop_watch: %s -- the run was not signalled" % exc, file=sys.stderr)
        return EXIT_ERROR
    return EXIT_GONE  # unreachable: follow() always ends with None


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--history", required=True, help="JSONL file, one line per unit")
    ap.add_argument("--total", required=True, type=int, help="units the run will produce")
    ap.add_argument("--target", required=True, help="minimum rate per class, e.g. 0.85")
    ap.add_argument("--min-units", type=int, default=20,
                    help="units per class before the Wilson rule applies (default 20)")
    ap.add_argument("--pid", required=True, type=int, help="remembered PID of the run chain, leader of its own "
                    "process group (start the run with setsid)")
    ap.add_argument("--report", required=True, help="where reason and table go")
    return run(ap.parse_args(argv))


if __name__ == "__main__":
    sys.exit(main())
