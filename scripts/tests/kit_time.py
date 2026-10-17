"""Time for the kit's self-tests: a fake clock, one hang marker, one wait.

Tests decide on events, never on the clock (TV-Z1, TV-K1). This module is the
one place under `scripts/tests/` that touches `time.monotonic`/`time.sleep`
(ban `py-time`, allow path): every other self-test takes time from here.

- `FakeClock`: monotonic and wall seconds that move only on `advance()`; hand
  `clock.monotonic` (or `clock.time`) to the kit code under test (`clock=`,
  like `pipeline/pool.py`, `revert.py`, `hygiene.py`). One instance per test,
  no process global (TV-H1: the self-tests run sharded in parallel).
- `HANG`: the hang marker of a real-time wait, 30 s times
  `MECLAW_TEST_TIME_FACTOR` (default 1). A marker, never a speed verdict.
- `until(desc, pred)`: polls `pred` until it returns something truthy and
  returns that; past the marker it fails with `HANG: <desc> (last: ...)`.
- `REAL`: the real clock with the face of `FakeClock` (`monotonic`/`time`/
  `sleep`), the default `clock=` of a helper whose wait crosses a process.
- T4 of the flake triage (VG TV-F2, `pipeline/flakes.py`): the triage reruns a
  test with `MECLAW_TEST_CLOCK_SHIFT` = `+24h` | `month` | `year`; a
  `FakeClock` built then starts a day on, or one second before the end of the
  month or the year of its start (UTC), so one further second crosses it.
"""
import calendar
import datetime
import os
import time

HANG_S = 30.0


def time_factor():
    """`MECLAW_TEST_TIME_FACTOR` (default 1); a value below 1 counts as 1."""
    try:
        factor = float(os.environ.get("MECLAW_TEST_TIME_FACTOR") or 1)
    except ValueError:
        factor = 1.0
    return max(factor, 1.0)


HANG = HANG_S * time_factor()

POLL_S = 0.02

# A deadline in milliseconds for a pruefling that takes one (display-lab's
# `tap.mjs`): the hang marker, and the two deadlines a test hands it to drive
# its expiry branch -- never a wait (re-check tv-bn M7h B-M1). OVER_MS ends
# the wait at the first check past 1 ms (the note names it), PAST_MS is over
# before the first check.
HANG_MS = int(HANG * 1000)
OVER_MS = 1
PAST_MS = -1

# The bound of a lane probe's tool call (`timeout` around it, GH #1224) in whole
# seconds: the hang marker. No call of a probe that answers runs under a short
# wall clock (re-check v2-r3-shell M2); the one that plays a call past its bound
# fires the bound itself (FIRE_BOUND_SH).
BOUND_S = int(HANG)

# Sh lines for a fake that plays a call past its bound: it sends the `timeout`
# above it the SIGALRM that timeout's own timer would send -- the bound's event,
# now, instead of a wait for it -- and notes that pid in $FAKE_BOUND_FILE. A call
# with no `timeout` above it exits 97 at once (no bound, nothing to fire). The
# signal goes by Python's os.kill: the kill builtin is banned in test scripts
# (test_side_effect_guard), and the pid here is the fake's own `timeout`.
FIRE_BOUND_SH = r'''fb=$$
while [ "${fb:-0}" -gt 1 ]; do
    fb=$(ps -o ppid= -p "$fb" 2>/dev/null | tr -d ' ')
    [ "$(ps -o comm= -p "${fb:-0}" 2>/dev/null)" = timeout ] && break
done
[ "${fb:-0}" -gt 1 ] || exit 97
echo "$fb" >>"$FAKE_BOUND_FILE"
python3 -I -c 'import os, signal, sys; os.kill(int(sys.argv[1]), signal.SIGALRM)' "$fb"
'''


def virtual(seconds):
    """`seconds` on a test's own clock (`FakeClock`, a lambda clock): plan
    data or virtual time, never a wait -- the class of a deadline in a test
    that advances its clock itself (re-check tv-bn M7h B-M1/C-M2)."""
    return float(seconds)

START = 1000.0
WALL = 1_760_000_000.0     # 2025-10-09 08:53:20 UTC

CLOCK_SHIFT_ENV = "MECLAW_TEST_CLOCK_SHIFT"
CLOCK_SHIFTS = ("+24h", "month", "year")


def clock_shift(wall, value=None):
    """Seconds T4 adds to a clock whose wall time starts at `wall`.

    `value` defaults to `MECLAW_TEST_CLOCK_SHIFT`; unset or empty is 0. `+24h`
    is one day; `month`/`year` land one second before the next first of the
    month / of January (UTC). Any other value is refused: a triage step that
    shifts nothing would call the test free of time."""
    if value is None:
        value = os.environ.get(CLOCK_SHIFT_ENV, "")
    if not value:
        return 0.0
    if value == "+24h":
        return 86400.0
    at = datetime.datetime.fromtimestamp(wall, datetime.timezone.utc)
    if value == "month":
        year, month = (at.year + 1, 1) if at.month == 12 else (at.year, at.month + 1)
    elif value == "year":
        year, month = at.year + 1, 1
    else:
        raise ValueError("%s=%r: not one of %s" % (CLOCK_SHIFT_ENV, value, ", ".join(CLOCK_SHIFTS)))
    end = calendar.timegm((year, month, 1, 0, 0, 0))
    return float(end - 1) - wall


class FakeClock:
    """Seconds that move only when the test says so.

    `monotonic()` starts at `start`, `time()` at `wall`; `advance(s)` moves
    both. `sleep(s)` is `advance(s)` for code that takes a `wait=` next to its
    `clock=`. A start the test leaves at its default moves by `clock_shift`
    (T4); a start the test names is an argument and stays."""

    def __init__(self, start=None, wall=None):
        shift = clock_shift(WALL if wall is None else float(wall))
        self._mono = START + shift if start is None else float(start)
        self._wall = WALL + shift if wall is None else float(wall)

    def monotonic(self):
        return self._mono

    def time(self):
        return self._wall

    def advance(self, seconds):
        if seconds < 0:
            raise ValueError("a clock does not run backwards: %r" % (seconds,))
        self._mono += seconds
        self._wall += seconds
        return self._mono

    sleep = advance

    def __call__(self):
        return self._mono


class _RealClock:
    """The process's clock, shaped like `FakeClock`."""

    monotonic = staticmethod(time.monotonic)
    sleep = staticmethod(time.sleep)
    time = staticmethod(time.time)

    def __call__(self):
        return time.monotonic()


REAL = _RealClock()


def flock_waiters(path):
    """The PIDs blocked on a lock of the file `path` (`/proc/locks`, a `->`
    line is a waiter): the event "it waits on the lock", never a pause that
    gives it time to (TV-Z3)."""
    ino = str(os.stat(path).st_ino)
    out = []
    with open("/proc/locks", encoding="ascii") as fh:
        for line in fh:
            f = line.split()
            if "->" in f and len(f) > f.index("->") + 5:
                i = f.index("->")
                if f[i + 5].rsplit(":", 1)[-1] == ino:
                    out.append(int(f[i + 4]))
    return out


class Hang(AssertionError):
    """A real-time wait passed its hang marker."""


def until(desc, pred, deadline=None, *, state=None, poll=POLL_S):
    """Poll `pred()` until it returns something truthy; return that value.

    `deadline` (default `HANG`) is a hang marker in real seconds, never a
    verdict. Past it: `Hang("HANG: <desc> (last: <state>)")`, the state from
    `state()` when given, else the last value of `pred`."""
    limit = HANG if deadline is None else deadline
    end = time.monotonic() + limit
    while True:
        got = pred()
        if got:
            return got
        if time.monotonic() >= end:
            last = state() if state is not None else got
            raise Hang("HANG: %s after %.0f s (last: %r)" % (desc, limit, last))
        time.sleep(poll)
