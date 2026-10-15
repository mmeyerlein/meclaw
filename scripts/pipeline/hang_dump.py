#!/usr/bin/env python3
"""Dump every Python stack before a station's hang marker (TV-K5, TV-Z5).

    python3 scripts/pipeline/hang_dump.py -m <module> [args...]

The executor kills a station that exceeds its cap (a hang marker, never a
budget) and writes the running test, the log tail and the process tree. A hung
Python process says more than that: where it waits. So the executor passes the
instant 30 s before the cap as `MECLAW_HANG_DUMP_AT` -- a `time.monotonic()`
value, which on Linux is CLOCK_MONOTONIC and the same for every process of the
host, so a unit a pool starts late still dumps at the station's instant --
and a Python station runs its test processes through this wrapper. It arms
`faulthandler.dump_traceback_later` with the seconds left (stderr, so the
station log) and then runs `-m <module>` as `python3 -m <module>` would -- the
current directory at the head of `sys.path`, `sys.argv` the module's own.
Without the variable, or with the instant passed, the wrapper only runs the
module.
"""

import faulthandler
import os
import runpy
import sys
import time

ENV = "MECLAW_HANG_DUMP_AT"
LEAD_S = 30      # the dump comes this many seconds before the cap
SELF = os.path.abspath(__file__)


def at(cap_at):
    """The value of `ENV` for a cap at the monotonic instant `cap_at`."""
    return "%.3f" % (cap_at - LEAD_S)


def arm(env=None, clock=time.monotonic):
    """Arm the dump from `env` (default `os.environ`); the delay, or None."""
    raw = (os.environ if env is None else env).get(ENV)
    try:
        secs = float(raw) - clock() if raw else 0.0
    except ValueError:
        return None
    if secs <= 0:
        return None
    faulthandler.dump_traceback_later(secs, repeat=False, exit=False)
    return secs


def argv(python, module):
    """The argv that runs `python -m <module>` through this wrapper; `python`
    is one word or an argv prefix (`lane.PYTHON`)."""
    return ([python] if isinstance(python, str) else list(python)) + [SELF, "-m", module]


def main(args):
    if len(args) < 2 or args[0] != "-m":
        print("usage: hang_dump.py -m <module> [args...]", file=sys.stderr)
        return 2
    arm()
    if sys.flags.safe_path:
        # `-I` (lane.PYTHON) puts no script directory at the head: the module
        # still imports from the current directory, as `python3 -m` would
        sys.path.insert(0, os.getcwd())
    else:
        sys.path[0] = os.getcwd()
    sys.argv = [args[1]] + list(args[2:])
    runpy.run_module(args[1], run_name="__main__", alter_sys=True)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
