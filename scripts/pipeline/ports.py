"""The port register of a lane: one port band per slot (wave dev-tempo W7).

WHY a register: the scenario and browser labs boot real colonies, and until
now every runner found its ports on its own (`--port N` searching upwards,
`bind(:0)`). That works for one runner at a time and collides the moment two
workers boot side by side. Here every slot owns a fixed band of 100 ports,
and a worker only ever opens ports inside the band of the slot it was given.

The band table (locked by `scripts/tests/test_pipeline_ports.py`)::

    band             range          who
    SATELLITE_BAND    7850 -  7999  satellite labs on the coordinator host
    RUST_BAND        20000 - 29999  meclaw_testing::free_port() (per process)
    SLOT_BAND        31000 - 31799  slots 0..7, 100 ports each (this file)
    EPHEMERAL_BAND   32768 - 60999  the kernel's outgoing connections

The slot band is the one free range: below the ephemeral range (otherwise any
outgoing connection may take a port a colony is about to bind), above the
Rust band and far from the satellite labs.

Inside a slot the ports have roles (`OFFSETS`, inclusive offsets from the
slot's base): `api` 0, `mounts` 1..9, `stub_model` 10..19, `peers` 20..99.

A claim (`claim`) checks the listeners (`ss -ltnH`) and writes
`<ws>/slots/<n>` as JSON `{slot, lo, hi, run, pid, at}`; a second claimer of
the same slot is refused (`SlotBusy`) until `release`, unless the recorded
pid is gone (a crashed claimer does not block the slot forever). The whole
check-and-write runs under `flock` on `<ws>/slots/.lock`, so two claimers who
both find the same stale record cannot both take it over (review W7 M2).

The register lives in `default_ws()`: `$MECLAW_PORT_REGISTER`, else
`DEFAULT_WS` -- one directory per host, because the ports are the host's.
Tests point the variable at a TempDir. Stdlib only.
"""

import fcntl
import json
import os
import pathlib
import subprocess
from datetime import datetime, timezone

SLOT_BASE = 31000
SLOT_WIDTH = 100
SLOT_COUNT = 8
SLOT_BAND = (SLOT_BASE, SLOT_BASE + SLOT_WIDTH * SLOT_COUNT - 1)
RUST_BAND = (20000, 29999)
EPHEMERAL_BAND = (32768, 60999)
SATELLITE_BAND = (7850, 7999)

# The host-wide register directory (claims under `<ws>/slots/`).
DEFAULT_WS = "/tmp/meclaw-port-register"

# Roles inside one slot band: inclusive (first, last) offset from the base.
OFFSETS = {
    "api": (0, 0),
    "mounts": (1, 9),
    "stub_model": (10, 19),
    "peers": (20, 99),
}


class SlotBusy(Exception):
    """The slot is taken: a port of its band listens, or a live claim exists."""

    def __init__(self, slot, ports=None, claim=None):
        self.slot = slot
        self.ports = sorted(ports or [])
        self.claim = claim
        why = f"ports {self.ports} listen" if self.ports else f"claimed by {claim}"
        super().__init__(f"slot {slot} busy: {why}")


def _check_slot(slot):
    if isinstance(slot, bool) or not isinstance(slot, int) or not 0 <= slot < SLOT_COUNT:
        raise ValueError(f"slot {slot!r} is not one of 0..{SLOT_COUNT - 1}")
    return slot


def band(slot):
    """(lo, hi) of a slot, both inclusive."""
    lo = SLOT_BASE + SLOT_WIDTH * _check_slot(slot)
    return lo, lo + SLOT_WIDTH - 1


def port_base(slot):
    return band(slot)[0]


def slot_of(port):
    """The slot whose band holds `port`, or None outside SLOT_BAND."""
    if SLOT_BAND[0] <= port <= SLOT_BAND[1]:
        return (port - SLOT_BASE) // SLOT_WIDTH
    return None


def check_base(port):
    """The slot whose base `port` is; ValueError for anything else.

    `--port-base` takes only a slot base (31000, 31100, ... 31700): a band
    elsewhere could not be claimed and would overlap two slots."""
    if isinstance(port, bool) or not isinstance(port, int):
        raise ValueError(f"port base {port!r} is not a number")
    slot = slot_of(port)
    if slot is None or port != port_base(slot):
        raise ValueError(f"port base {port} is not a slot base "
                         f"({SLOT_BASE}, {SLOT_BASE + SLOT_WIDTH}, ... "
                         f"{SLOT_BASE + SLOT_WIDTH * (SLOT_COUNT - 1)})")
    return slot


def default_ws():
    """The register directory: `$MECLAW_PORT_REGISTER` or DEFAULT_WS."""
    return os.environ.get("MECLAW_PORT_REGISTER") or DEFAULT_WS


def port_at(slot, role, index=0):
    """The `index`-th port of `role` in a slot (`port_at(2, "mounts", 0)`)."""
    if role not in OFFSETS:
        raise ValueError(f"unknown port role {role!r}; one of {sorted(OFFSETS)}")
    first, last = OFFSETS[role]
    if not 0 <= index <= last - first:
        raise ValueError(f"{role} holds {last - first + 1} ports, not index {index}")
    return port_base(slot) + first + index


def parse_slots(text):
    """`"0,3,5"` -> [0, 3, 5]; every entry in 0..SLOT_COUNT-1 and unique."""
    out = []
    for part in str(text).split(","):
        part = part.strip()
        if not part.isdigit():
            raise ValueError(f"slot list {text!r}: {part!r} is not a slot number")
        n = _check_slot(int(part))
        if n in out:
            raise ValueError(f"slot list {text!r}: slot {n} twice")
        out.append(n)
    return out


def format_slots(slots):
    return ",".join(str(_check_slot(n)) for n in slots)


def listeners(run=None):
    """Every local TCP port in LISTEN state, from `ss -ltnH`.

    `run` defaults to `subprocess.run` (looked up at call time, so a test can
    pass its own); a failing `ss` raises -- a claim never goes ahead blind.
    """
    run = run or subprocess.run
    res = run(["ss", "-ltnH"], capture_output=True, text=True, check=False)
    if res.returncode != 0:
        raise RuntimeError(f"ss -ltnH failed ({res.returncode}): {res.stderr.strip()}")
    found = set()
    for line in res.stdout.splitlines():
        parts = line.split()
        if len(parts) < 4:
            continue
        try:
            found.add(int(parts[3].rsplit(":", 1)[1]))
        except (IndexError, ValueError):
            continue
    return found


def _now():
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def _slot_file(slot, ws):
    return pathlib.Path(ws) / "slots" / str(_check_slot(slot))


def _pid_alive(pid):
    if not isinstance(pid, int) or isinstance(pid, bool) or pid <= 0:
        return False
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    return True


def _live_claim(path):
    """The claim record at `path` if its claimer still lives, else None."""
    try:
        rec = json.loads(path.read_text())
    except FileNotFoundError:
        return None
    except (OSError, ValueError):
        return {"unreadable": str(path)}  # never take over what we cannot read
    return rec if _pid_alive(rec.get("pid")) else None


def _busy_ports(slot, listening):
    lo, hi = band(slot)
    return sorted(p for p in listening if lo <= p <= hi)


def claim(slot, ws, run_id=None, listening=None):
    """Claim a slot for this process; raises SlotBusy, returns the record."""
    busy = _busy_ports(slot, listeners() if listening is None else listening)
    if busy:
        raise SlotBusy(slot, ports=busy)
    path = _slot_file(slot, ws)
    path.parent.mkdir(parents=True, exist_ok=True)
    lo, hi = band(slot)
    rec = {"slot": slot, "lo": lo, "hi": hi, "run": run_id, "pid": os.getpid(),
           "at": _now()}
    data = json.dumps(rec, sort_keys=True).encode()
    with open(path.parent / ".lock", "a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)  # released when the file closes
        held = _live_claim(path)
        if held is not None:
            raise SlotBusy(slot, claim=held)
        path.unlink(missing_ok=True)  # no claim, or its claimer is gone
        fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o644)
        with os.fdopen(fd, "wb") as fh:
            fh.write(data)
    return rec


def holder(slot, ws):
    """The live claim on `slot` (its record), or None when the slot is free."""
    return _live_claim(_slot_file(slot, ws))


def release(slot, ws):
    """Drop the claim file of a slot (idempotent)."""
    _slot_file(slot, ws).unlink(missing_ok=True)


def free_slots(ws, k, listening=None):
    """Up to `k` free slots, lowest first (fewer when fewer are free).

    Free = no live claim file and no listener in the band. Nothing is claimed
    here; the caller claims what it takes.
    """
    if k < 0:
        raise ValueError(f"k {k} < 0")
    listening = listeners() if listening is None else listening
    out = []
    for slot in range(SLOT_COUNT):
        if len(out) >= k:
            break
        if _busy_ports(slot, listening) or _live_claim(_slot_file(slot, ws)):
            continue
        out.append(slot)
    return out


# --- CLI -------------------------------------------------------------------

def _cmd_ports(args):
    if args.action == "bands":
        print(json.dumps({"slot": SLOT_BAND, "rust": RUST_BAND,
                          "ephemeral": EPHEMERAL_BAND, "satellite": SATELLITE_BAND,
                          "slots": {n: band(n) for n in range(SLOT_COUNT)},
                          "offsets": OFFSETS}, indent=2))
        return 0
    print(format_slots(free_slots(args.ws or default_ws(), args.k)))
    return 0


def register(sub):
    """`pipeline ports bands|free --ws <dir> [-k N]` (wave dev-tempo W7)."""
    p = sub.add_parser("ports", help="the slot port register: bands, free slots")
    p.add_argument("action", choices=("bands", "free"))
    p.add_argument("--ws", default=None,
                   help="register holding slots/<n> claims (default: default_ws())")
    p.add_argument("-k", type=int, default=SLOT_COUNT, help="how many free slots")
    p.set_defaults(func=_cmd_ports)
