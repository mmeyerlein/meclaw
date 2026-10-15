"""The colony pool: warm scenario colonies that outlive one case (wave dev-tempo W7).

WHY: the scenario labs boot a fresh colony for every case, and round 2 (the
flake isolation) or a second gate of the same tree boots all of them again on
the very same binary. A pool colony is a transient user unit that stays up
between cases, so a `get` on the same binary, suite and slot finds it warm.

Names and life cycle::

    unit       pipe-pool-<sha8>-<suite>-<slot>
    idle       <unit>-idle   (one-shot timer: `systemctl --user stop <unit>`)
    states     created -> warm (health ok) -> collected (idle stop, hygiene)

A `get` is a hit when the unit is active AND the recorded FULL binary sha
equals the asked one AND the health probe answers. Otherwise the old unit is
stopped, any other pool unit recorded on the same port is stopped too (a slot
port belongs to one colony at a time), and a new transient unit starts
(`systemd-run --user --unit <unit> --collect -p RuntimeMaxSec=4h <argv>`).
The binary sha is the right key here (unlike the cache key): a pool colony
belongs to exactly one binary on exactly one lane.

Idle: every `get` re-arms the one-shot timer (stop the old `<unit>-idle.timer`,
failure ignored, then `systemd-run --user --unit <unit>-idle
--on-active=20min systemctl --user stop <unit>`). `RuntimeMaxSec` measures run
time, not idle time, so it is only the hard ceiling.

Guard: every systemd call of the pool goes through `guard_argv` first (in
`Systemd.run` and in front of any injected systemd): only
`systemctl --user is-active|stop <pipe-pool-*>` and
`systemd-run --user --unit <pipe-pool-*>` in its exact form (only
`--collect`, `-p RuntimeMaxSec=`, `--on-active=`; payload meclaw or
`systemctl --user stop <pipe-pool-*>`). A unit without the fixed prefix
(`mm-os-e28`, any other user unit) is a PoolError -- start, stop and status
alike, CLI exit 2 -- before anything runs (coordinator rule after the e28
incident of 09.10.). The prefix stays `pipe-pool-`: the lane hygiene (W2)
keys its busy check and its zombie exemption on it.

Readiness: after a start, `get` waits for the health probe against ONE
deadline (`timeout_s`), with the clock and the wait injectable; in production
that is a bounded readiness wait of a just-started process, not a watcher.

Every move is one `workspace_changed` event (V0 section 3, kind `pool`);
lanes never write the store -- the core applies the event with
`apply_workspace_event`. Stdlib only.
"""

import collections
import hashlib
import json
import os
import pathlib
import re
import subprocess
import time
from datetime import datetime, timezone

from . import ports

IDLE_MIN = 20
RUNTIME_MAX = "4h"
HEALTH_TIMEOUT_S = 120.0
HEALTH_INTERVAL_S = 0.5
UNIT_PREFIX = "pipe-pool-"
REPORT_ENV = "MECLAW_POOL_REPORT"
STATE_ENV = "MECLAW_POOL_STATE"

# The full sha256 `_pin_payload` compares (R2 M1): a short sha never matches.
_SHA_RE = re.compile(r"^[0-9a-f]{64}$")
_SUITE_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_.-]*$")

PoolColony = collections.namedtuple("PoolColony", "unit port root hit")


class PoolError(RuntimeError):
    """A pool colony could not be started or never became healthy."""


_POOL_UNIT = re.compile(r"\A" + re.escape(UNIT_PREFIX) + r"[A-Za-z0-9_.-]+\Z")


def check_unit(unit):
    """Only a pool unit may be touched (review W7 M1, coordinator rule after the
    e28 incident 09.10.): `pool stop|status --unit` on the owner's machine
    would otherwise reach the live colony (`mm-os-e28`) or any other user
    unit. PoolError for anything without the fixed pool prefix."""
    if not isinstance(unit, str) or not _POOL_UNIT.match(unit) or ".." in unit:
        raise PoolError(f"{unit!r} is not a pool unit (prefix {UNIT_PREFIX})")
    return unit


_SYSTEMCTL_VERBS = ("is-active", "stop")


_RUN_OPTION_ONCE = (("--collect", re.compile(r"\A--collect\Z")),
                    ("--on-active", re.compile(r"\A--on-active=[0-9]+(min|s|h)?\Z")))
_RUNTIME_MAX = re.compile(r"\ARuntimeMaxSec=[0-9]+(min|s|h)?\Z")


def _refuse_run(argv, why):
    raise PoolError(f"refused systemd call {argv!r}: {why}; only `systemd-run "
                    "--user --unit <pool unit> [--collect] [-p RuntimeMaxSec=..] "
                    "[--on-active=..] <meclaw ...|systemctl --user stop <pool unit>>`")


def _guard_systemd_run(argv):
    """The exact `systemd-run` form of the pool (re-check W7 N1): head
    `systemd-run --user --unit <pool unit>`, then at most once each
    `--collect`, `-p RuntimeMaxSec=..` and `--on-active=..`, then a payload
    that is either exactly `systemctl --user stop <pool unit>` or a command
    whose argv[0] has the basename `meclaw`. Any other option (`-p
    Conflicts=..`, `--unit=..`, `--wait`, ...) and any other payload (`sh -c`,
    an absolute systemctl path, a stop of a foreign unit) is refused."""
    if argv[1:3] != ["--user", "--unit"] or len(argv) < 4:
        _refuse_run(argv, "head is not `systemd-run --user --unit <unit>`")
    check_unit(argv[3])
    seen = set()
    i = 4
    while i < len(argv) and argv[i].startswith("-"):
        opt = argv[i]
        if opt == "-p" and i + 1 < len(argv) and _RUNTIME_MAX.match(argv[i + 1]):
            key, step = "-p", 2
        else:
            key = next((k for k, rx in _RUN_OPTION_ONCE if rx.match(opt)), None)
            step = 1
        if key is None or key in seen:
            _refuse_run(argv, f"option {opt!r} not allowed here")
        seen.add(key)
        i += step
    payload = argv[i:]
    if payload[:1] == ["systemctl"]:
        if len(payload) != 4 or payload[1:3] != ["--user", "stop"]:
            _refuse_run(argv, "scheduled systemctl is not `systemctl --user stop <unit>`")
        check_unit(payload[3])
        return
    if not payload or os.path.basename(payload[0]) != "meclaw":
        _refuse_run(argv, "payload is neither meclaw nor a pool stop")


def guard_argv(argv):
    """The one gate every systemd call of the pool passes, before it runs.

    Allowed: `systemctl --user is-active|stop <pool unit>` and
    `systemd-run` in its exact pool form (`_guard_systemd_run`): pool unit,
    only `--collect`/`-p RuntimeMaxSec=`/`--on-active=`, payload meclaw or
    `systemctl --user stop <pool unit>`. Everything else -- another verb, the system manager, a
    unit without the pool prefix, another tool -- is a PoolError, and nothing
    has run."""
    argv = [str(a) for a in argv]
    if argv[:1] == ["systemctl"]:
        if (len(argv) != 4 or argv[1] != "--user"
                or argv[2] not in _SYSTEMCTL_VERBS):
            raise PoolError(f"refused systemd call {argv!r}: only "
                            f"`systemctl --user {'|'.join(_SYSTEMCTL_VERBS)} <pool unit>`")
        check_unit(argv[3])
        return argv
    if argv[:1] == ["systemd-run"]:
        _guard_systemd_run(argv)
        return argv
    raise PoolError(f"refused call {argv!r}: the pool runs only systemctl/systemd-run")


class Systemd:
    """The real `systemctl`/`systemd-run`: run(argv) -> (rc, out, err).
    `guard_argv` runs first, so no foreign unit ever reaches a subprocess."""

    def run(self, argv):
        argv = guard_argv(argv)
        res = subprocess.run(argv, capture_output=True, text=True, check=False)
        return res.returncode, res.stdout, res.stderr


class Guarded:
    """Puts `guard_argv` in front of any systemd (the real one or a fake), so
    the API refuses a foreign unit the same way whoever executes."""

    def __init__(self, inner):
        self.inner = inner

    def run(self, argv):
        return self.inner.run(guard_argv(argv))


def _systemd(systemd):
    if isinstance(systemd, Guarded):
        return systemd
    return Guarded(systemd if systemd is not None else Systemd())


def unit_name(binary_sha, suite, slot):
    sha = str(binary_sha).lower()
    if not _SHA_RE.match(sha):
        raise ValueError(f"binary sha {binary_sha!r} is not a full sha256 (64 hex)")
    if not _SUITE_RE.match(str(suite)):
        raise ValueError(f"suite {suite!r} is not a unit-safe name")
    ports.band(slot)  # validates the slot
    return f"{UNIT_PREFIX}{sha[:8]}-{suite}-{slot}"


def idle_unit(unit):
    return unit + "-idle"


def default_state_dir():
    env = os.environ.get(STATE_ENV)
    if env:
        return pathlib.Path(env)
    return pathlib.Path.home() / ".local/state/meclaw/pool"


def _runner(runner):
    from . import lane   # the one source of the own lane name (re-check tv-bn M7q)
    return lane.own_name(runner)


def _now():
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def _record_path(state_dir, unit):
    return pathlib.Path(state_dir) / f"{unit}.json"


def _load(path):
    try:
        return json.loads(path.read_text())
    except (OSError, ValueError):
        return None


def _save(path, rec):
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(".json.tmp")
    tmp.write_text(json.dumps(rec, sort_keys=True))
    os.replace(tmp, path)


def _changed(emit, runner, unit, root, frm, to):
    if emit is None:
        return
    emit("workspace_changed", "note",
         {"runner": runner, "id": f"{runner}:pool:{unit}", "kind": "pool",
          "path": root, "from": frm, "to": to, "bytes": None,
          "ttl_s": IDLE_MIN * 60})


def _rearm_idle(systemd, unit):
    idle = idle_unit(unit)
    systemd.run(["systemctl", "--user", "stop", idle + ".timer"])  # may not exist
    rc, _, err = systemd.run(["systemd-run", "--user", "--unit", idle,
                              f"--on-active={IDLE_MIN}min",
                              "systemctl", "--user", "stop", unit])
    if rc != 0:
        raise PoolError(f"idle timer {idle} not armed (rc {rc}): {err.strip()}")


def _wait_healthy(health, port, timeout_s, clock, wait):
    """A bounded readiness wait at boot, one deadline (OR-DT.W7.5): the meclaw
    daemon sends no readiness event (no sd_notify, no ready file), so the
    first answer of /health is the event; an event-based start needs
    `Type=notify` in the daemon first."""
    deadline = clock() + timeout_s
    while True:
        if health(port):
            return True
        left = deadline - clock()
        if left <= 0:
            return False
        wait(min(HEALTH_INTERVAL_S, left))


def _collect(unit, systemd, emit, state_dir, runner):
    """Stop a unit and its idle timer; a recorded colony moves to collected."""
    check_unit(unit)
    systemd.run(["systemctl", "--user", "stop", unit])
    systemd.run(["systemctl", "--user", "stop", idle_unit(unit) + ".timer"])
    if state_dir is None:
        return None
    path = _record_path(state_dir, unit)
    rec = _load(path)
    if rec is not None:
        _changed(emit, runner, unit, rec.get("root"), "warm", "collected")
    path.unlink(missing_ok=True)
    return rec


def file_sha256(path):
    """Hex sha256 of the file at `path`."""
    h = hashlib.sha256()
    with open(path, "rb") as fh:
        for chunk in iter(lambda: fh.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def _pin_payload(argv, binary_sha, binary_hash):
    """#1132: the payload is the binary that keys the unit, not any `meclaw`."""
    try:
        got = (binary_hash or file_sha256)(argv[0])
    except OSError as exc:
        raise PoolError(f"payload {argv[0]} cannot be hashed: {exc}") from exc
    if str(got).lower() != str(binary_sha).lower():
        raise PoolError(f"payload {argv[0]} is not the binary {str(binary_sha)[:8]}")


def get(binary_sha, suite, slot, argv, root, port, health, emit=None, systemd=None,
        state_dir=None, runner=None, timeout_s=HEALTH_TIMEOUT_S, clock=time.monotonic,
        wait=time.sleep, binary_hash=None):
    """The pool colony of (binary, suite, slot): warm if possible, else booted.

    A boot first checks that argv[0] hashes to `binary_sha` (`binary_hash`,
    default sha256 of the file) and raises `PoolError` otherwise (#1132)."""
    unit = unit_name(binary_sha, suite, slot)
    check_unit(unit)
    root = str(root)
    systemd = _systemd(systemd)
    state_dir = pathlib.Path(state_dir) if state_dir else default_state_dir()
    runner = _runner(runner)
    path = _record_path(state_dir, unit)
    rec = _load(path)
    active = systemd.run(["systemctl", "--user", "is-active", unit])[0] == 0
    if (active and rec is not None and rec.get("sha") == binary_sha
            and rec.get("port") == port and rec.get("root") == root and health(port)):
        _rearm_idle(systemd, unit)
        return PoolColony(unit, port, root, True)

    _pin_payload(argv, binary_sha, binary_hash)
    # Miss: the unit itself, and whoever else holds this port, go first.
    prev = None
    if _collect(unit, systemd, emit, state_dir, runner) is not None:
        prev = "collected"
    for other in sorted(state_dir.glob(f"{UNIT_PREFIX}*.json")):
        other_rec = _load(other)
        if other_rec is not None and other_rec.get("port") == port:
            _collect(other.name[:-len(".json")], systemd, emit, state_dir, runner)

    rc, _, err = systemd.run(["systemd-run", "--user", "--unit", unit, "--collect",
                              "-p", f"RuntimeMaxSec={RUNTIME_MAX}", *argv])
    if rc != 0:
        raise PoolError(f"{unit} did not start (rc {rc}): {err.strip()}")
    _changed(emit, runner, unit, root, prev, "created")
    if not _wait_healthy(health, port, timeout_s, clock, wait):
        systemd.run(["systemctl", "--user", "stop", unit])
        _changed(emit, runner, unit, root, "created", "collected")
        raise PoolError(f"{unit} not healthy on port {port} within {timeout_s}s")
    _save(path, {"sha": binary_sha, "suite": suite, "slot": slot, "port": port,
                 "root": root, "created_at": _now()})
    _changed(emit, runner, unit, root, "created", "warm")
    _rearm_idle(systemd, unit)
    return PoolColony(unit, port, root, False)


def stop(unit, systemd=None, emit=None, state_dir=None, runner=None):
    """Stop a pool colony now (hygiene, eviction): its row moves to collected."""
    check_unit(unit)
    systemd = _systemd(systemd)
    state_dir = pathlib.Path(state_dir) if state_dir else default_state_dir()
    _collect(unit, systemd, emit, state_dir, _runner(runner))
    return True


def status(unit, systemd=None):
    """`systemctl --user is-active` of a pool unit: "active", "inactive", ..."""
    check_unit(unit)
    rc, out, _ = _systemd(systemd).run(["systemctl", "--user", "is-active", unit])
    return (out.strip() or ("active" if rc == 0 else "inactive"))


def apply_workspace_event(db, data, at=None):
    """Core side: one pool `workspace_changed` into the `workspaces` table."""
    from . import store
    store.workspace_set(db, data["id"], data["runner"], data["kind"], data["path"],
                        data["to"], run=data.get("run"), bytes=data.get("bytes"),
                        ttl_s=data.get("ttl_s"), at=at)


class PoolReport:
    """What a runner reports at exit when `$MECLAW_POOL_REPORT` names a file:
    `{"suite", "hits", "boots", "cases": {<case id>: "pool"|"fresh"}}`."""

    def __init__(self, suite):
        self.suite = suite
        self.hits = 0
        self.boots = 0
        self.cases = {}

    def case(self, case_id, colony):
        """`colony` is the PoolColony the case ran on, None for a fresh boot."""
        if colony is None:
            self.cases[case_id] = "fresh"
            return
        self.cases[case_id] = "pool"
        if colony.hit:
            self.hits += 1
        else:
            self.boots += 1

    def write(self, path=None, environ=None):
        environ = os.environ if environ is None else environ
        path = path or environ.get(REPORT_ENV)
        if not path:
            return None
        _save(pathlib.Path(path), {"suite": self.suite, "hits": self.hits,
                                   "boots": self.boots,
                                   "cases": dict(sorted(self.cases.items()))})
        return str(path)


# --- CLI -------------------------------------------------------------------

def _cmd_pool(args):
    state_dir = pathlib.Path(args.state_dir) if args.state_dir else default_state_dir()
    if args.action == "list":
        for path in sorted(state_dir.glob(f"{UNIT_PREFIX}*.json")):
            print(path.name[:-len(".json")], json.dumps(_load(path), sort_keys=True))
        return 0
    if not args.unit:
        print(f"pool {args.action}: --unit is required")
        return 2
    try:
        check_unit(args.unit)
        if args.action == "status":
            state = status(args.unit, Systemd())
            print(f"{args.unit} {state}")
            return 0 if state == "active" else 3
        stop(args.unit, Systemd(), state_dir=state_dir,
             emit=lambda e, k, d: print(json.dumps({"event": e, "kind": k, "data": d})))
    except PoolError as e:
        print(f"pool {args.action}: {e}")
        return 2
    return 0


def register(sub):
    """`pipeline pool list|status|stop --unit <unit>` (wave dev-tempo W7); a
    unit without the pool prefix exits 2 before any systemd call."""
    p = sub.add_parser("pool", help="the colony pool: list recorded units, status or stop one")
    p.add_argument("action", choices=("list", "status", "stop"))
    p.add_argument("--unit")
    p.add_argument("--state-dir", default=None, help=f"default ${STATE_ENV}")
    p.set_defaults(func=_cmd_pool)
