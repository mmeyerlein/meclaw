#!/usr/bin/python3 -I
"""Lane hygiene: workspaces with a life cycle, quotas, precheck, watchdog.

Runs in three shapes, all from this one file (stdlib only, Python >= 3.10):

  * on a lane as a file:      python3 hygiene.py check --root /srv/target
  * on a lane over ssh stdin: ssh lane-a python3 - check ... < hygiene.py
    (no script path then, so this module never imports a sibling at the top)
  * imported by the core:     check(ctx, ...), Watcher(...), generations(...),
    apply_events(store, lines) -- the executor calls functions, not a process.

Every command writes the contract events (`hygiene_checked`,
`workspace_changed`, `watchdog_fired`, `pass_ended`) as JSONL objects on
stdout, `{"event", "kind", "at", "run", "data"}`, and diagnostics on stderr.
Exit 0 = ok or collected, 2 = aborted.

WHY a lifecycle and not a threshold: until now `gate.sh --lane` checked one
free-space number and then wiped the WHOLE target (a cold build), while
bench leftovers, five target generations and dead scopes filled the disks
unnoticed until a linker died with a bus error. Here every workspace moves
`created -> in_use|warm -> closed -> collected|evicted`, every move is one
event, and only this module makes the moves.

Hard rules kept here: nothing ever kills by process name (a stray colony on a
slot port is only reported -- it may be a person's lab colony), processes
are found through the scope's cgroup, and while a legacy gate holds the host
cargo lock nothing is removed at all (the legacy gate shares `target/`).
"""

import argparse
import datetime
import fcntl
import json
import math
import os
import re
import shlex
import shutil
import socket
import stat
import subprocess
import sys
import threading
import time

GIB = 1024 ** 3
DEFAULT_ROOT = "/srv/target"
DEFAULT_RESERVE_G = 60.0
DEFAULT_MAX_TREES = 4
DEFAULT_EXPORT_G = 40.0
DEFAULT_LEGACY_LOCK = "/tmp/meclaw-w26-cargo.lock"
LEFTOVER_AGE_S = 3600
STRAY_FINDING_AGE_S = 24 * 3600
# `coverage/{tree,target}`: the instrumented build of `lane_sync.sh coverage`
# (W6 review I1) -- a warm target like a slot's, never a stray.
LAYOUT_TARGET_DIRS = ("target", "slot", "bench", "export", "tree", "coverage")
# The kit's one Python (`lane.PYTHON`, re-check tv-bn M7m M2). Named here
# again because this file also runs alone on a host (`lane_sync.sh hygiene
# --kit` puts `<hash8>/hygiene.py` there, re-check tv-bn M7o M1) where
# pipeline.lane cannot be imported; a test pins the two to be the same.
PYTHON = ("/usr/bin/python3", "-I")


def _python_argv(*args):
    """The argv that runs `args` (a script and its words) in PYTHON."""
    return list(PYTHON) + [str(a) for a in args]
GEN_DIRS = ("deps", ".fingerprint", "build", "incremental")
NEVER_TOUCH = ("wt", "meclaw.git", "sccache")
PORT_BANDS = ((31000, 31799), (7930, 7949), (7970, 7989), (7860, 7879),
              (20000, 29999))
UNIT_NAME_RE = re.compile(r"^[A-Za-z0-9_.@:-]+$")
GEN_NAME_RE = re.compile(r"^(?P<stem>.+)-(?P<hash>[0-9a-f]{16})(?P<ext>\..*)?$")
HASH_IN_PATH_RE = re.compile(r"-([0-9a-f]{16})(?=$|[./])")
PASS_STATIONS = ("green", "red", "cached", "flaky", "skip", "note", "ask",
                 "aborted")


# --------------------------------------------------------------------------
# Context: every side effect goes through here so tests can inject fakes.
# --------------------------------------------------------------------------

def _default_free_g(path):
    st = os.statvfs(path)
    return st.f_bavail * st.f_frsize / GIB


def _default_read_text(path):
    try:
        with open(path, "r", encoding="utf-8", errors="replace") as fh:
            return fh.read()
    except OSError:
        return None


def _default_run(argv, timeout=20):
    try:
        cp = subprocess.run(argv, stdin=subprocess.DEVNULL,
                            capture_output=True, text=True, timeout=timeout)
        return cp.returncode, cp.stdout, cp.stderr
    except (OSError, subprocess.TimeoutExpired) as exc:
        return 127, "", str(exc)


def _lanes_file_root(runner):
    """The root of `runner`'s `local` line in the host file (Z6) -- the root
    lane.load_lanes gives the same lane, so the hygiene never clears under
    `/srv/target` while the runs of that lane build elsewhere. None when the
    file is missing or names no local root for `runner`."""
    path = os.environ.get("MECLAW_LANES_FILE") or os.path.expanduser("~/.config/meclaw/lanes")
    try:
        with open(path, encoding="utf-8") as fh:
            text = fh.read()
    except OSError:
        return None
    for line in text.splitlines():
        words = line.split("#", 1)[0].split()
        if len(words) >= 3 and words[0] == runner and words[1] == "local":
            positional = [w for w in words[2:] if "=" not in w]
            if positional:
                return positional[0].rstrip("/") or "/"
    return None


class Ctx:
    """Where hygiene runs and how it touches the world.

    Injectable: `free_g(path)` (statvfs in G), `clock()` (epoch seconds),
    `monotonic()` (the seconds a check books as `secs`),
    `run(argv)` -> (rc, out, err), `read_text(path)` -> str|None,
    `loadavg()`, `out`/`err` streams, `proc_root`,
    `cgroup_root`, `lock_path` (legacy gate lock).
    """

    def __init__(self, root=None, runner=None, run_id=None, dry_run=False,
                 free_g=None, clock=None, run=None, read_text=None,
                 loadavg=None, out=None, err=None,
                 proc_root="/proc", cgroup_root="/sys/fs/cgroup",
                 lock_path=None, nproc=None, state_file=None, monotonic=None):
        self.runner = runner or os.environ.get("MECLAW_LANE_NAME") or \
            socket.gethostname().split(".")[0]
        self.root = os.path.abspath(
            root or os.environ.get("MECLAW_LANE_ROOT") or _lanes_file_root(self.runner)
            or DEFAULT_ROOT)
        self.run_id = run_id
        self.dry_run = dry_run
        self.free_g = free_g or _default_free_g
        self.clock = clock or time.time
        self.monotonic = monotonic or time.monotonic
        self.run = run or _default_run
        self.read_text = read_text or _default_read_text
        self.loadavg = loadavg or (lambda: os.getloadavg()[0])
        self.out = out
        self.err = err
        self.proc_root = proc_root
        self.cgroup_root = cgroup_root
        self.lock_path = lock_path or os.environ.get("MECLAW_GATE_LOCK") or \
            DEFAULT_LEGACY_LOCK
        self.nproc = nproc or os.cpu_count() or 1
        self.state_file = state_file or os.path.join(self.root,
                                                     ".hygiene-state.json")

    # -- output ------------------------------------------------------------
    def emit(self, event, kind, data, run=None):
        obj = {"event": event, "kind": kind, "at": iso(self.clock()),
               "run": run if run is not None else self.run_id, "data": data}
        stream = self.out or sys.stdout
        stream.write(json.dumps(obj, sort_keys=True) + "\n")
        stream.flush()
        return obj

    def diag(self, msg):
        stream = self.err or sys.stderr
        stream.write("hygiene: " + msg + "\n")
        stream.flush()


def iso(ts):
    return datetime.datetime.fromtimestamp(
        ts, datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def parse_iso(text):
    if not text:
        return None
    t = str(text).replace("Z", "+00:00")
    try:
        dt = datetime.datetime.fromisoformat(t)
    except ValueError:
        return None
    if dt.tzinfo is None:
        dt = dt.replace(tzinfo=datetime.timezone.utc)
    return dt.timestamp()


def _g(nbytes):
    return round(nbytes / GIB, 2)


def _rel(ctx, path):
    try:
        return os.path.relpath(path, ctx.root)
    except ValueError:
        return path


def _ws_id(ctx, kind, path):
    return "%s:%s:%s" % (ctx.runner, kind, _rel(ctx, path))


def _mtime(path):
    try:
        return os.stat(path).st_mtime
    except OSError:
        return 0.0


def _ctime(path):
    try:
        return os.stat(path).st_ctime
    except OSError:
        return 0.0


def tree_bytes(path):
    """Size of a tree by lstat sum (used at evict/collect, never in check)."""
    total = 0
    try:
        st = os.lstat(path)
    except OSError:
        return 0
    if not os.path.isdir(path) or os.path.islink(path):
        return st.st_size
    stack = [path]
    while stack:
        cur = stack.pop()
        try:
            with os.scandir(cur) as it:
                for e in it:
                    try:
                        if e.is_dir(follow_symlinks=False):
                            stack.append(e.path)
                        else:
                            total += e.stat(follow_symlinks=False).st_size
                    except OSError:
                        pass
        except OSError:
            pass
    return total


def flat_bytes(path):
    """Size of the files directly in `path` (one scandir, no recursion)."""
    total = 0
    try:
        with os.scandir(path) as it:
            for e in it:
                try:
                    if e.is_file(follow_symlinks=False):
                        total += e.stat(follow_symlinks=False).st_size
                except OSError:
                    pass
    except OSError:
        pass
    return total


def _sidecar_bytes(path):
    try:
        with open(os.path.join(path, ".hygiene-bytes")) as fh:
            return int(fh.read().strip())
    except (OSError, ValueError):
        return None


def _remove(ctx, path):
    if ctx.dry_run:
        ctx.diag("dry-run: would remove %s" % path)
        return
    if os.path.isdir(path) and not os.path.islink(path):
        shutil.rmtree(path, ignore_errors=True)
    else:
        try:
            os.unlink(path)
        except OSError:
            pass


def _inside(child, parent):
    child = os.path.realpath(child)
    parent = os.path.realpath(parent)
    return child == parent or child.startswith(parent.rstrip("/") + "/")


# --------------------------------------------------------------------------
# Probes
# --------------------------------------------------------------------------

def legacy_lock_held(ctx):
    """Probe the legacy host cargo lock without ever holding it.

    Opens read-only, tries LOCK_EX|LOCK_NB and releases at once -- the same
    semantics as `flock -n`. A missing file means nobody holds it.
    """
    return lock_held(ctx.lock_path)


def _take_build_path_lock(target):
    """`flock -n` on the build path lock beside `target` (GH #1140), kept by
    the caller over the removal (review Zb R1 M4): the fd, or None when it is
    held or cannot be taken. A lane's target (`target/`, `slot/<n>/target`,
    `coverage/target`) sits beside its build path `tree/`; a gate or a pre run
    holds `tree.lock` there from the push to its last step (lane.hold).
    O_NOFOLLOW (GH #1154): a symlinked lock is not taken and a dangling
    link's target is never created -- the slot counts as held."""
    path = os.path.join(os.path.dirname(target.rstrip("/")), "tree.lock")
    try:
        fd = os.open(path, os.O_RDONLY | os.O_CREAT | os.O_NOFOLLOW, 0o644)
    except OSError:
        return None
    try:
        fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except OSError:
        os.close(fd)
        return None
    return fd


def lock_held(path):
    """`flock -n` on `path` as a probe: True when someone else holds it.

    Fail closed (review Zb I3): only a missing lock file is free; a lock the
    probe cannot open or flock counts as held, so nothing is evicted. So does
    anything but a regular file (GH #1154): flock on a directory succeeds
    and would read as free."""
    try:
        fd = os.open(path, os.O_RDONLY)
    except FileNotFoundError:
        return False
    except OSError:
        return True
    try:
        if not stat.S_ISREG(os.fstat(fd).st_mode):
            return True
        fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except OSError:
        return True
    else:
        fcntl.flock(fd, fcntl.LOCK_UN)
        return False
    finally:
        os.close(fd)


def list_scope_units(ctx):
    """`pipe-run-*` units as [(unit, active_state)] (pool units filtered)."""
    rc, out, _ = ctx.run(["systemctl", "--user", "list-units", "pipe-run-*",
                          "--all", "--plain", "--no-legend"])
    units = []
    if rc != 0:
        return units
    for line in out.splitlines():
        parts = line.split()
        if not parts:
            continue
        if parts[0] in ("●", "*") and len(parts) > 1:
            parts = parts[1:]
        unit = parts[0]
        if not unit.startswith("pipe-") or unit.startswith("pipe-pool-"):
            continue
        if not unit.startswith("pipe-run-"):
            continue
        active = parts[2] if len(parts) > 2 else "active"
        units.append((unit, active))
    return units


def unit_run_id(unit):
    name = unit[len("pipe-"):] if unit.startswith("pipe-") else unit
    return name.rsplit(".", 1)[0] if "." in name else name


def scope_pids(ctx, scope):
    """PIDs of a scope via its cgroup; None when the scope is gone."""
    rc, out, _ = ctx.run(["systemctl", "--user", "show", "-p",
                          "ControlGroup", "--value", scope])
    cg = out.strip() if rc == 0 else ""
    if cg.startswith("ControlGroup="):
        cg = cg.split("=", 1)[1]
    if not cg:
        return None
    text = ctx.read_text(ctx.cgroup_root + cg + "/cgroup.procs")
    if text is None:
        return None
    return [int(x) for x in text.split() if x.isdigit()]


def mem_total_g(ctx):
    text = ctx.read_text(os.path.join(ctx.proc_root, "meminfo")) or ""
    m = re.search(r"^MemTotal:\s+(\d+)\s*kB", text, re.M)
    return int(m.group(1)) * 1024 / GIB if m else None


def listening_ports(ctx):
    """[(port, [pid...])] for TCP listeners in the slot bands."""
    rc, out, _ = ctx.run(["ss", "-ltnpH"])
    found = []
    if rc != 0:
        return found
    for line in out.splitlines():
        parts = line.split()
        if len(parts) < 4:
            continue
        local = parts[3]
        try:
            port = int(local.rsplit(":", 1)[1])
        except (IndexError, ValueError):
            continue
        if not any(lo <= port <= hi for lo, hi in PORT_BANDS):
            continue
        pids = [int(p) for p in re.findall(r"pid=(\d+)", line)]
        found.append((port, pids))
    return found


def _pid_in_pipe_unit(ctx, pid):
    text = ctx.read_text(os.path.join(ctx.proc_root, str(pid), "cgroup"))
    return bool(text) and "/pipe-" in text


# --------------------------------------------------------------------------
# Dispatch and settings
# --------------------------------------------------------------------------

def load_dispatch(spec):
    if not spec:
        return None
    if spec == "-":
        text = sys.stdin.read()
    else:
        with open(spec, "r", encoding="utf-8") as fh:
            text = fh.read()
    return json.loads(text) if text.strip() else None


def env_reserve_g():
    """(reserve, note) from MECLAW_GATE_LANE_MIN_FREE_G (GH #1154 re-check
    m-1/m-2): unset -> (None, None); negative, not finite or unreadable ->
    the default with a note -- a negative reserve switched the disk guard off
    without a word; 0 stays the explicit off switch, with a note."""
    raw = os.environ.get("MECLAW_GATE_LANE_MIN_FREE_G") or ""
    if not raw:
        return None, None
    try:
        v = float(raw)
    except ValueError:
        v = float("nan")
    if not math.isfinite(v) or v < 0:
        return DEFAULT_RESERVE_G, ("MECLAW_GATE_LANE_MIN_FREE_G=%s ignored, floor %dG"
                                   % (raw, DEFAULT_RESERVE_G))
    if v == 0:
        return 0.0, "MECLAW_GATE_LANE_MIN_FREE_G=%s: disk guard off" % raw
    return v, None


def settings(dispatch, reserve_g=None, expected_g=None, live_runs=None,
             own_tree=None, max_trees=None, export_g=None, own_target=None):
    d = dispatch or {}
    budget = d.get("budget") or {}
    env_reserve, floor_note = env_reserve_g()
    s = {
        "reserve_g": float(reserve_g if reserve_g is not None else
                           budget.get("reserve_g", DEFAULT_RESERVE_G
                                      if env_reserve is None else env_reserve)),
        "expected_g": float(expected_g if expected_g is not None else
                            d.get("expected_g", 0) or 0),
        "max_trees": int(max_trees if max_trees is not None else
                         budget.get("max_trees", DEFAULT_MAX_TREES)),
        "export_g": float(export_g if export_g is not None else
                          budget.get("export_g", DEFAULT_EXPORT_G)),
        "fingerprint_expected": d.get("fingerprint_expected") or {},
        "own": [p for p in (d.get("target"), d.get("tree"), own_tree,
                            own_target) if p],
        # the NOTE of the env floor, only where it decided the reserve
        "floor_note": floor_note if reserve_g is None and
        "reserve_g" not in budget else None,
    }
    if own_tree:
        s["own"].append(os.path.join(own_tree, "target"))
    # Liveness is authoritative only when someone told us who is alive: a
    # dispatch or an explicit --live-run. Without it (legacy lane_sync call)
    # a scope is never stopped -- that would kill a run we cannot see.
    if live_runs is not None or dispatch is not None:
        s["live_runs"] = list(live_runs if live_runs is not None else
                              d.get("live_runs") or [])
        s["authoritative"] = True
    else:
        s["live_runs"] = []
        s["authoritative"] = False
    return s


# --------------------------------------------------------------------------
# Collect: leftovers, zombies, benches, export caches
# --------------------------------------------------------------------------

def _ws_changed(ctx, kind, path, frm, to, nbytes, **extra):
    data = {"runner": ctx.runner, "id": _ws_id(ctx, kind, path),
            "kind": kind, "path": path, "from": frm, "to": to,
            "bytes": nbytes}
    data.update(extra)
    if ctx.dry_run:
        ctx.diag("dry-run: %s %s %s -> %s" % (kind, path, frm, to))
        return None
    return ctx.emit("workspace_changed", "note", data)


def collect_leftovers(ctx, s, active_runs, notes):
    """ws/<run> without a live run: > 1 h old -> collected, younger -> NOTE."""
    n = 0
    base = os.path.join(ctx.root, "ws")
    live = set(s["live_runs"]) | set(active_runs)
    for name in sorted(_listdir(base)):
        path = os.path.join(base, name)
        if not os.path.isdir(path) or name in live:
            continue
        age = ctx.clock() - _mtime(path)
        if age <= LEFTOVER_AGE_S:
            notes.append("young-ws %s" % name)
            continue
        nbytes = _sidecar_bytes(path)
        if nbytes is None:
            nbytes = tree_bytes(path)
        _remove(ctx, path)
        _ws_changed(ctx, "ws", path, "closed", "collected", nbytes,
                    run=name)
        n += 1
    return n


def stop_zombies(ctx, s, units, notes):
    """pipe-run-* scopes whose run is not live -> systemctl --user stop."""
    n = 0
    for unit, active in units:
        rid = unit_run_id(unit)
        if rid in s["live_runs"] or active == "inactive":
            continue
        n += 1
        if not s["authoritative"]:
            notes.append("zombie-unverified %s" % unit)
            continue
        if ctx.dry_run:
            ctx.diag("dry-run: would stop %s" % unit)
            continue
        if not UNIT_NAME_RE.match(unit):
            continue
        ctx.run(["systemctl", "--user", "stop", unit])
        if active == "failed":
            ctx.run(["systemctl", "--user", "reset-failed", unit])
        ctx.diag("stopped zombie scope %s" % unit)
    return n


def _bench_meta(path):
    try:
        with open(os.path.join(path, ".hygiene.json")) as fh:
            return json.load(fh)
    except (OSError, ValueError):
        return None


def collect_bench(ctx, name):
    path = os.path.join(ctx.root, "bench", name)
    if not os.path.isdir(path) or "/" in name or name in ("", ".", ".."):
        return 0
    nbytes = tree_bytes(path)
    _remove(ctx, path)
    _ws_changed(ctx, "bench", path, "in_use", "collected", nbytes)
    return 1


def collect_expired_benches(ctx):
    n = 0
    base = os.path.join(ctx.root, "bench")
    for name in sorted(_listdir(base)):
        meta = _bench_meta(os.path.join(base, name))
        if not meta:
            continue
        created = parse_iso(meta.get("created_at"))
        ttl = meta.get("ttl_s")
        if created is None or ttl is None:
            continue
        if ctx.clock() >= created + float(ttl):
            n += collect_bench(ctx, name)
    return n


def collect_exports(ctx, quota_g):
    base = os.path.join(ctx.root, "export")
    entries = []
    for name in _listdir(base):
        path = os.path.join(base, name)
        if not os.path.isdir(path):
            continue
        nbytes = _sidecar_bytes(path)
        if nbytes is None:
            nbytes = tree_bytes(path)
        entries.append((_mtime(path), path, nbytes))
    total = sum(e[2] for e in entries)
    n = 0
    for _, path, nbytes in sorted(entries):
        if total <= quota_g * GIB:
            break
        _remove(ctx, path)
        _ws_changed(ctx, "export", path, "warm", "evicted", nbytes)
        total -= nbytes
        n += 1
    return n


def _listdir(path):
    try:
        return os.listdir(path)
    except OSError:
        return []


# --------------------------------------------------------------------------
# Kits: `<root>/kit/` (GH #1147, KF5)
# --------------------------------------------------------------------------
# Two keyings share the directory: lane.push_kit puts the core's pipeline kit
# at `<sha8>/` (git revision) with the marker `<sha8>.ok` written last, and
# lane_sync.sh `hygiene --kit` writes `<hash8>/hygiene.py` (content hash) for
# a timer on the host that calls it hours later. Neither was ever removed.

KIT_NAME_RE = re.compile(r"^[0-9a-f]{8}$")
KIT_MARKER_RE = re.compile(r"^([0-9a-f]{8})\.ok$")
KIT_KEEP_AGE_S = 24 * 3600


def _kit_refs(ctx):
    """`(units, cmdlines)`: `[(unit, ExecStart)]` of the user's loaded
    services and `[(pid, cmdline)]` of the processes -- or None when the
    service list cannot be read (then nothing may go)."""
    rc, out, _ = ctx.run(["systemctl", "--user", "list-units", "--all",
                          "--plain", "--no-legend"])
    if rc != 0:
        return None
    units = []
    for line in out.splitlines():
        parts = line.split()
        if parts and parts[0] in ("●", "*"):
            parts = parts[1:]
        if not parts or not parts[0].endswith(".service") \
                or not UNIT_NAME_RE.match(parts[0]):
            continue
        rc, text, _ = ctx.run(["systemctl", "--user", "show", "-p",
                               "ExecStart", "--value", parts[0]])
        if rc != 0:
            return None
        units.append((parts[0], text))
    procs = []
    for pid in _listdir(ctx.proc_root):
        if pid.isdigit():
            text = ctx.read_text(os.path.join(ctx.proc_root, pid, "cmdline"))
            if text:
                procs.append((pid, text.replace("\0", " ")))
    return units, procs


def _kit_used(name, refs):
    needle = "/kit/%s/" % name
    for unit, text in refs[0]:
        if needle in text:
            return "unit %s" % unit
    for pid, text in sorted(refs[1], key=lambda p: int(p[0])):
        if needle in text:
            return "pid %s" % pid
    return None


def kits(ctx, notes=None):
    """One row per kit under `<root>/kit/`, `{name, keying, path, age_s,
    bytes, verdict, why}`, sorted by name. keying: `rev` (with its marker),
    `hygiene` (a hygiene copy), `partial` (a push without its marker),
    `marker` (a marker without its directory). A kit stays while a unit or a
    process names it (`unit <name>`, `pid <n>`), while it is younger than
    KIT_KEEP_AGE_S (`young`), or when the references cannot be read
    (`refcheck-failed`); an orphan marker always goes: push_kit trusts the
    marker alone and would skip the push of a kit that is not there."""
    base = os.path.join(ctx.root, "kit")
    names = _listdir(base)
    if not names:
        return []
    refs = _kit_refs(ctx)
    if refs is None and notes is not None:
        notes.append("kit-refcheck-failed")
    markers = {m.group(1) for m in map(KIT_MARKER_RE.match, names) if m}
    rows = []
    for name in sorted(set(n for n in names if KIT_NAME_RE.match(n)) | markers):
        path = os.path.join(base, name)
        marker = os.path.join(base, name + ".ok")
        if not os.path.isdir(path) or os.path.islink(path):
            if name in markers:
                rows.append({"name": name, "keying": "marker", "path": marker,
                             "age_s": int(ctx.clock() - _mtime(marker)), "bytes": 0,
                             "verdict": "remove", "why": "orphan-marker"})
            continue
        if name in markers:
            keying = "rev"
        elif os.path.isfile(os.path.join(path, "hygiene.py")):
            keying = "hygiene"
        else:
            keying = "partial"
        used = max(_mtime(path), _mtime(marker), _mtime(os.path.join(path, "hygiene.py")))
        if keying == "partial":
            # Review pd-1147 M-1 (b): `rsync -a` gives the directory the core
            # tree's mtime, a push between copy and marker looks old by it;
            # rsync cannot set the ctime back.
            used = max(used, _ctime(path))
        age = ctx.clock() - used
        why = "refcheck-failed" if refs is None else _kit_used(name, refs)
        if why is None and age < KIT_KEEP_AGE_S:
            why = "young"
        rows.append({"name": name, "keying": keying, "path": path,
                     "age_s": int(age), "bytes": tree_bytes(path),
                     "verdict": "keep" if why else "remove", "why": why or "old"})
    return rows


def collect_kits(ctx, notes):
    """Remove the kits `kits` gives up; the marker before its directory, so an
    interrupted removal leaves a kit the next push rewrites. Returns the count."""
    n = 0
    for row in kits(ctx, notes):
        if row["verdict"] != "remove":
            continue
        if row["keying"] != "marker":
            _remove(ctx, os.path.join(os.path.dirname(row["path"]), row["name"] + ".ok"))
        _remove(ctx, row["path"])
        _ws_changed(ctx, "kit", row["path"], "closed", "collected", row["bytes"],
                    keying=row["keying"])
        n += 1
    return n


def collect(ctx, dispatch=None, bench=None, live_runs=None, notes=None):
    """Collect what is dead. Returns the number of changes (0 = idempotent)."""
    notes = notes if notes is not None else []
    s = settings(dispatch, live_runs=live_runs)
    if legacy_lock_held(ctx):
        notes.append("legacy-run")
        ctx.diag("legacy gate lock held: collect skipped")
        return 0
    if bench:
        return collect_bench(ctx, bench)
    units = list_scope_units(ctx)
    active = [unit_run_id(u) for u, a in units if a != "inactive"]
    n = collect_leftovers(ctx, s, [] if s["authoritative"] else active, notes)
    stop_zombies(ctx, s, units, notes)
    n += collect_expired_benches(ctx)
    n += collect_exports(ctx, s["export_g"])
    n += collect_kits(ctx, notes)
    return n


# --------------------------------------------------------------------------
# Evict: LRU over warm targets
# --------------------------------------------------------------------------

def warm_targets(ctx):
    found = []
    t = os.path.join(ctx.root, "target")
    if os.path.isdir(t):
        found.append(t)
    for n in sorted(_listdir(os.path.join(ctx.root, "slot"))):
        p = os.path.join(ctx.root, "slot", n, "target")
        if os.path.isdir(p):
            found.append(p)
    c = os.path.join(ctx.root, "coverage", "target")
    if os.path.isdir(c):
        found.append(c)
    return found


def _target_used_at(path):
    gt = os.path.join(path, ".gate-tree")
    return _mtime(gt) if os.path.exists(gt) else _mtime(path)


def _is_main_pass(ctx, path):
    if "main-pass" in os.path.relpath(path, ctx.root).split(os.sep):
        return True
    text = ctx.read_text(os.path.join(path, ".gate-tree"))
    return bool(text) and text.strip() == "main-pass"


def _is_own(path, own):
    for o in own:
        if not o:
            continue
        if _inside(path, o) or _inside(o, path):
            return True
    return False


def evict(ctx, dispatch=None, need_g=None, max_trees=None, own_tree=None,
          live_runs=None, notes=None, reserve_g=None, own_target=None):
    """LRU-evict warm targets while free < reserve + need or trees > max.

    `own_target` is the target the run will build into (a lane's
    `target/` or `slot/<n>/target`, beside the build path `tree/`): never
    evicted -- wiping it is the runner's call, on MECLAW_LANE_WIPE_OK only.
    Nor is a neighbour slot's target while its build path lock is held: a
    check without the gate lock would wipe it under a running gate (#1140).
    """
    notes = notes if notes is not None else []
    s = settings(dispatch, reserve_g=reserve_g, expected_g=need_g,
                 live_runs=live_runs, own_tree=own_tree, max_trees=max_trees,
                 own_target=own_target)
    if legacy_lock_held(ctx):
        notes.append("legacy-run")
        ctx.diag("legacy gate lock held: evict skipped")
        return 0
    want = s["reserve_g"] + s["expected_g"]
    targets = warm_targets(ctx)
    warm = len(targets)
    n = 0
    for path in sorted(targets, key=_target_used_at):
        if ctx.free_g(ctx.root) >= want and warm <= s["max_trees"]:
            break
        if _is_own(path, s["own"]):
            continue
        if s["live_runs"] and _is_main_pass(ctx, path):
            notes.append("main-pass-protected %s" % _rel(ctx, path))
            continue
        fd = _take_build_path_lock(path)
        if fd is None:
            notes.append("busy-slot-protected %s" % _rel(ctx, path))
            continue
        try:  # held over the removal: a gate asking meanwhile gets busy (R1 M4)
            nbytes = _sidecar_bytes(path)
            if nbytes is None:
                nbytes = tree_bytes(path)
            _remove(ctx, path)
        finally:
            os.close(fd)
        _ws_changed(ctx, "target", path, "warm", "evicted", nbytes)
        warm -= 1
        n += 1
    return n


# --------------------------------------------------------------------------
# Generations (Posten 9, OR-DT-5 b)
# --------------------------------------------------------------------------

def keep_hashes_from_cargo_json(path):
    """Metadata hashes a cargo --message-format=json stream names."""
    keep = set()
    with open(path, "r", encoding="utf-8", errors="replace") as fh:
        for line in fh:
            line = line.strip()
            if not line.startswith("{"):
                continue
            try:
                msg = json.loads(line)
            except ValueError:
                continue
            paths = list(msg.get("filenames") or [])
            for key in ("executable", "out_dir"):
                if msg.get(key):
                    paths.append(msg[key])
            for p in paths:
                for comp in str(p).split("/"):
                    m = GEN_NAME_RE.match(comp)
                    if m:
                        keep.add(m.group("hash"))
    return keep


def _scan_generation_groups(target):
    """{(stem, hash): {"entries": [(path, bytes)], "mtime", "shape"}}."""
    groups = {}
    debug = os.path.join(target, "debug")
    for sub in GEN_DIRS:
        base = os.path.join(debug, sub)
        try:
            it = os.scandir(base)
        except OSError:
            continue
        with it:
            for e in it:
                m = GEN_NAME_RE.match(e.name)
                if not m:
                    continue
                try:
                    st = e.stat(follow_symlinks=False)
                    isdir = e.is_dir(follow_symlinks=False)
                except OSError:
                    continue
                nbytes = tree_bytes(e.path) if isdir else st.st_size
                key = (m.group("stem"), m.group("hash"))
                g = groups.setdefault(key, {"entries": [], "mtime": 0.0,
                                            "shape": set()})
                g["entries"].append((e.path, nbytes))
                g["mtime"] = max(g["mtime"], st.st_mtime)
                mark = sub + ":" + (m.group("ext") or "")
                if sub == "build" and isdir:
                    mark += "out" if os.path.isdir(
                        os.path.join(e.path, "out")) else "script"
                g["shape"].add(mark)
    return groups


def _debug_bytes(target, groups):
    total = 0
    for g in groups.values():
        total += sum(b for _, b in g["entries"])
    # Unmatched files (lock files, base36 incremental dirs) count as well.
    debug = os.path.join(target, "debug")
    for sub in GEN_DIRS:
        base = os.path.join(debug, sub)
        for name in _listdir(base):
            if not GEN_NAME_RE.match(name):
                total += tree_bytes(os.path.join(base, name))
    return total


def generations(ctx, target=None, keep_from=None, generation_g=None,
                quota_factor=1.2):
    """Evict old cargo generations from `<target>/debug` down to the quota.

    Groups are `(stem, 16-hex hash)` from `<stem>-<hash>[.ext]`. Kept: the
    hashes a cargo JSON stream names (exact), else per stem and shape the
    newest hash (heuristic). Non-kept groups go oldest first until the debug
    size is <= generation_g * quota_factor (generation_g defaults to the
    kept set's size). Returns [{target, evicted_g, groups, bytes}].
    """
    targets = [target] if target else warm_targets(ctx)
    results = []
    for t in targets:
        t = os.path.abspath(t)
        if not _generation_target_allowed(ctx, t):
            ctx.diag("refused: %s is not a target under the lane layout" % t)
            continue
        groups = _scan_generation_groups(t)
        if not groups:
            continue
        if keep_from:
            keep = keep_hashes_from_cargo_json(keep_from)
        else:
            newest = {}
            for (stem, h), g in groups.items():
                k = (stem, tuple(sorted(g["shape"])))
                if k not in newest or g["mtime"] > newest[k][1]:
                    newest[k] = (h, g["mtime"])
            keep = {h for h, _ in newest.values()}
        kept_bytes = sum(sum(b for _, b in g["entries"])
                         for (_, h), g in groups.items() if h in keep)
        total = _debug_bytes(t, groups)
        gen = generation_g * GIB if generation_g is not None else kept_bytes
        quota = gen * quota_factor
        freed, n = 0, 0
        victims = sorted(((g["mtime"], key) for key, g in groups.items()
                          if key[1] not in keep))
        for _, key in victims:
            if total - freed <= quota:
                break
            for path, nbytes in groups[key]["entries"]:
                if not _inside(path, os.path.join(t, "debug")):
                    continue
                _remove(ctx, path)
                freed += nbytes
            n += 1
        if n:
            _ws_changed(ctx, "target", t, "warm", "warm", total - freed,
                        evicted_g=_g(freed), groups=n)
        results.append({"target": t, "evicted_g": _g(freed), "groups": n,
                        "bytes": total - freed,
                        "generation_g": _g(gen)})
    return results


def _generation_target_allowed(ctx, t):
    real = os.path.realpath(t)
    for name in NEVER_TOUCH:
        if _inside(real, os.path.join(ctx.root, name)):
            return False
    if os.path.basename(real) in NEVER_TOUCH:
        return False
    return os.path.isdir(os.path.join(real, "debug"))


# --------------------------------------------------------------------------
# Check: the precheck the executor runs first
# --------------------------------------------------------------------------

def stray_targets(ctx):
    """[(path, size_g|None, age_s)] for debug dirs outside the layout."""
    found = []
    for top in sorted(_listdir(ctx.root)):
        p1 = os.path.join(ctx.root, top)
        if top == "debug" and os.path.isdir(p1):
            found.append(p1)
            continue
        if top in LAYOUT_TARGET_DIRS or not os.path.isdir(p1) or \
                os.path.islink(p1):
            continue
        p2 = os.path.join(p1, "debug")
        if os.path.isdir(p2):
            found.append(p2)
    out = []
    for p in found:
        deps = os.path.join(p, "deps")
        size = _g(flat_bytes(deps)) if os.path.isdir(deps) else None
        out.append((p, size, ctx.clock() - _mtime(p)))
    return out


def _bump_reserve_hits(ctx):
    state = _read_state(ctx)
    state["reserve_hits"] = int(state.get("reserve_hits", 0)) + 1
    if ctx.dry_run:
        return
    try:
        with open(ctx.state_file, "w") as fh:
            json.dump(state, fh)
    except OSError:
        pass


def _read_state(ctx):
    try:
        with open(ctx.state_file) as fh:
            return json.load(fh)
    except (OSError, ValueError):
        return {}


def check(ctx, dispatch=None, reserve_g=None, expected_g=None, own_tree=None,
          live_runs=None, local=False, own_target=None, abort_below_g=None):
    """Precheck. Emits workspace_changed per change and one hygiene_checked.

    Returns the hygiene_checked data dict; `data["verdict"] == "aborted"`
    means: stop before any station (exit 2). The check clears towards
    reserve + expected; with `abort_below_g` (a lane's one floor,
    MECLAW_LANE_FLOOR_G, handed over by lane_sync) only less than that
    aborts, otherwise less than reserve + expected does.
    """
    t0 = ctx.monotonic()
    d = dispatch or {}
    if d.get("run") and not ctx.run_id:
        ctx.run_id = d["run"]
    s = settings(dispatch, reserve_g=reserve_g, expected_g=expected_g,
                 live_runs=live_runs, own_tree=own_tree, own_target=own_target)
    need = s["reserve_g"] + s["expected_g"]
    notes, findings, reasons = [], [], []
    if s["floor_note"]:
        notes.append(s["floor_note"])
        ctx.diag("NOTE " + s["floor_note"])
    changes = 0
    leftovers = zombies = ports_busy = 0

    if local:
        state_dir = os.path.expanduser("~/.local/state")
        paths = ["/"] + ([state_dir] if os.path.isdir(state_dir) else [])
        free = min(ctx.free_g(p) for p in paths)
        if free < need:
            reasons.append("disk: free %.1f G < reserve %.0f + expected %.0f"
                           % (free, s["reserve_g"], s["expected_g"]))
        verdict = "aborted" if reasons else "ok"
        return _checked(ctx, s, free, 0, 0, 0, verdict, notes, findings,
                        reasons, t0)

    legacy = legacy_lock_held(ctx)
    if legacy:
        notes.append("legacy-run")

    # Root filesystem next to the lane root: a full / kills ssh and journald.
    root_free = ctx.free_g("/")
    if root_free < 5:
        findings.append("root-fs-low %.1f" % root_free)

    units = list_scope_units(ctx)
    active = [unit_run_id(u) for u, a in units if a != "inactive"]

    # Leftovers and zombies.
    if legacy:
        for name in _listdir(os.path.join(ctx.root, "ws")):
            if name not in s["live_runs"]:
                leftovers += 1
    else:
        leftovers = collect_leftovers(
            ctx, s, [] if s["authoritative"] else active, notes)
        changes += leftovers
    zombies = stop_zombies(ctx, s, units, notes) if not legacy else \
        len([u for u, a in units
             if unit_run_id(u) not in s["live_runs"] and a != "inactive"])

    # Slot ports: a listener outside every pipe- unit is a stray colony.
    for port, pids in listening_ports(ctx):
        ports_busy += 1
        if not pids or not any(_pid_in_pipe_unit(ctx, p) for p in pids):
            notes.append("stray-colony %d" % port)

    # The lane's fingerprint is not measured here (re-check tv-bn M7q M1,
    # OR-T11-22 "one fact, one source"): `lane_fingerprint.sh` is its one
    # producer, the executor judges a drift against the register right after
    # this check. A third producer here measured its own forms (the Python of
    # whatever `python3` started it, `ext4` beside `ext2/ext3`) and noted a
    # `lane-drift` the executor did not. Linger is read from the register.
    if str(s["fingerprint_expected"].get("linger")).lower() in ("no", "false"):
        notes.append("linger-off")

    # Foreign load.
    load1 = ctx.loadavg()
    others = [r for r in s["live_runs"] if r != ctx.run_id]
    if load1 > ctx.nproc / 2.0 and not others:
        notes.append("foreign-load %.1f" % load1)

    # Stray targets.
    for path, size, age in stray_targets(ctx):
        label = "stray-target %s %s" % (
            path, "%.1f" % size if size is not None else "?")
        notes.append(label)
        if age > STRAY_FINDING_AGE_S:
            findings.append(label)

    # Expired benches.
    if not legacy:
        changes += collect_expired_benches(ctx)

    # Space.
    free = ctx.free_g(ctx.root)
    if free < need:
        _bump_reserve_hits(ctx)
        if legacy:
            ctx.diag("legacy gate lock held: nothing removed")
        else:
            changes += collect_exports(ctx, s["export_g"])
            free = ctx.free_g(ctx.root)
            if free < need:
                for r in generations(ctx):
                    changes += 1 if r["groups"] else 0
                free = ctx.free_g(ctx.root)
            if free < need:
                changes += evict(ctx, dispatch, need_g=s["expected_g"],
                                 reserve_g=s["reserve_g"],
                                 own_tree=own_tree, live_runs=live_runs,
                                 notes=notes, own_target=own_target)
                free = ctx.free_g(ctx.root)
        if abort_below_g is not None:
            if free < abort_below_g:
                reasons.append("disk: free %.1f G < floor %.0f"
                               % (free, abort_below_g))
            elif free < need:
                notes.append("short %.1f < reserve %.0f + expected %.0f"
                             % (free, s["reserve_g"], s["expected_g"]))
        elif free < need:
            reasons.append("disk: free %.1f G < reserve %.0f + expected %.0f"
                           % (free, s["reserve_g"], s["expected_g"]))
    if reasons:
        verdict = "aborted"
    elif changes:
        verdict = "collected"
    else:
        verdict = "ok"
    return _checked(ctx, s, free, leftovers, zombies, ports_busy,
                    verdict, notes, findings, reasons, t0)


def _checked(ctx, s, free, leftovers, zombies, ports_busy, verdict,
             notes, findings, reasons, t0):
    data = {"run": ctx.run_id, "runner": ctx.runner, "free_g": round(free, 2),
            "reserved_g": s["expected_g"], "reserve_g": s["reserve_g"],
            "leftovers": leftovers, "zombies": zombies,
            "ports_busy": ports_busy, "verdict": verdict,
            "notes": notes, "findings": findings, "reasons": reasons,
            "secs": round(ctx.monotonic() - t0, 3)}
    if ctx.dry_run:
        data["dry_run"] = True
    kind = "finding" if verdict == "aborted" or findings else "note"
    ctx.emit("hygiene_checked", kind, data)
    return data


# --------------------------------------------------------------------------
# Watchdog
# --------------------------------------------------------------------------

class Watcher(threading.Thread):
    """The watchdog DURING a run (a thread in the executor, tick 30 s).

    Fires once on: disk below reserve, a scope process over the RSS cap,
    one process > 95 % of all cores for > 10 min without `touch()`, or the
    run's deadline. Order is fixed: emit `watchdog_fired`, call
    `on_abort(kind)` (the executor writes station_ended/pass_ended ABORTED),
    THEN stop the scope -- the executor itself lives in that scope.
    """

    def __init__(self, ctx, run, scope, reserve_g=DEFAULT_RESERVE_G,
                 deadline_at=None, interval=30.0, rss_cap_frac=0.5,
                 cpu_frac=0.95, cpu_window_s=600.0, on_abort=None,
                 stop_scope=None, clk_tck=None):
        super().__init__(name="hygiene-watch-%s" % run, daemon=True)
        self.ctx = ctx
        self.run_id = run
        self.scope = scope
        self.reserve_g = float(reserve_g)
        self.deadline = parse_iso(deadline_at) if isinstance(
            deadline_at, str) else deadline_at
        self.interval = float(interval)
        self.rss_cap_frac = rss_cap_frac
        self.cpu_frac = cpu_frac
        self.cpu_window_s = cpu_window_s
        self.on_abort = on_abort
        self.stop_scope = stop_scope or self._systemctl_stop
        self.clk_tck = clk_tck or os.sysconf("SC_CLK_TCK")
        self.fired = None
        self.gone = False
        self._stop_evt = threading.Event()
        self._last_touch = ctx.clock()
        self._cpu = {}

    def touch(self):
        """The executor saw progress (an event): resets the runaway clock."""
        self._last_touch = self.ctx.clock()

    def stop(self):
        self._stop_evt.set()

    def _systemctl_stop(self, scope):
        if UNIT_NAME_RE.match(scope):
            self.ctx.run(["systemctl", "--user", "stop", scope])

    def _read_rss_g(self, pid):
        text = self.ctx.read_text(
            os.path.join(self.ctx.proc_root, str(pid), "status")) or ""
        m = re.search(r"^VmRSS:\s+(\d+)\s*kB", text, re.M)
        return int(m.group(1)) * 1024 / GIB if m else 0.0

    def _read_cpu_ticks(self, pid):
        text = self.ctx.read_text(
            os.path.join(self.ctx.proc_root, str(pid), "stat")) or ""
        rest = text.rsplit(")", 1)[-1].split()
        try:
            return int(rest[11]) + int(rest[12])
        except (IndexError, ValueError):
            return None

    def tick(self):
        """One check. Returns the fired kind, "gone", or None."""
        if self.fired:
            return self.fired
        now = self.ctx.clock()
        free = self.ctx.free_g(self.ctx.root)
        if free < self.reserve_g:
            return self._fire("disk", round(free, 2), self.reserve_g)
        if self.deadline is not None and now >= self.deadline:
            return self._fire("deadline", now, self.deadline)
        pids = scope_pids(self.ctx, self.scope)
        if pids is None:
            self.gone = True
            return "gone"
        mem = mem_total_g(self.ctx)
        if mem:
            cap = mem * self.rss_cap_frac
            for pid in pids:
                rss = self._read_rss_g(pid)
                if rss > cap:
                    return self._fire("process", round(rss, 2), round(cap, 2))
        seen = set()
        for pid in pids:
            seen.add(pid)
            ticks = self._read_cpu_ticks(pid)
            if ticks is None:
                continue
            prev = self._cpu.get(pid)
            if prev is None:
                self._cpu[pid] = {"ticks": ticks, "at": now, "high": None}
                continue
            dt = now - prev["at"]
            if dt <= 0:
                continue
            frac = (ticks - prev["ticks"]) / float(self.clk_tck) / dt / \
                self.ctx.nproc
            prev["ticks"], prev["at"] = ticks, now
            if frac > self.cpu_frac:
                if prev["high"] is None:
                    prev["high"] = now - dt
                since = max(prev["high"], self._last_touch)
                if now - since > self.cpu_window_s:
                    return self._fire("process", round(frac, 3),
                                      self.cpu_frac)
            else:
                prev["high"] = None
        for pid in list(self._cpu):
            if pid not in seen:
                del self._cpu[pid]
        return None

    def _fire(self, kind, value, threshold):
        self.fired = kind
        self.ctx.emit("watchdog_fired", "finding",
                      {"run": self.run_id, "runner": self.ctx.runner,
                       "kind": kind, "value": value, "threshold": threshold,
                       "action": "aborted"}, run=self.run_id)
        if self.on_abort:
            try:
                self.on_abort(kind)
            except Exception as exc:  # the stop must still happen
                self.ctx.diag("on_abort failed: %s" % exc)
        self.stop_scope(self.scope)
        return kind

    def run(self):
        while not self._stop_evt.is_set():
            res = self.tick()
            if res:
                return
            self._stop_evt.wait(self.interval)


def minimal_pass_ended(ctx, run, started):
    data = {"run": run, "verdict": "ABORTED",
            "secs": round(ctx.clock() - started, 3),
            "lane_seconds": round(ctx.clock() - started, 3),
            "critical_path_s": 0, "rounds": 1, "cache_hits": 0,
            "stations": {k: 0 for k in PASS_STATIONS}}
    return ctx.emit("pass_ended", "status", data, run=run)


def watch(ctx, run, scope, dispatch=None, interval=30.0):
    d = dispatch or {}
    s = settings(dispatch)
    started = ctx.clock()
    w = Watcher(ctx, run, scope, reserve_g=s["reserve_g"],
                deadline_at=d.get("deadline_at"), interval=interval,
                on_abort=lambda kind: minimal_pass_ended(ctx, run, started))
    w.run()
    return 2 if w.fired else 0


# --------------------------------------------------------------------------
# Status
# --------------------------------------------------------------------------

def status(ctx, reserve_g=None):
    units = list_scope_units(ctx)
    ws_dirs = _listdir(os.path.join(ctx.root, "ws"))
    zombies = 0
    for u, a in units:
        if a == "failed" or unit_run_id(u) not in ws_dirs:
            zombies += 1
    busy = any(a not in ("inactive", "failed") for _, a in units) or \
        legacy_lock_held(ctx)
    deps = os.path.join(ctx.root, "target", "debug", "deps")
    gens, gens_avg, target_g = 0, 0.0, None
    if os.path.isdir(deps):
        target_g = _g(flat_bytes(deps))
        per_stem = {}
        for name in _listdir(deps):
            m = GEN_NAME_RE.match(name)
            if m and not m.group("ext"):
                per_stem.setdefault(m.group("stem"), set()).add(
                    m.group("hash"))
        if per_stem:
            gens = max(len(v) for v in per_stem.values())
            gens_avg = round(sum(len(v) for v in per_stem.values()) /
                             float(len(per_stem)), 2)
    sccache = os.path.join(ctx.root, "sccache")
    line = {
        "runner": ctx.runner,
        "state": "busy" if busy else "idle",
        "free_g": round(ctx.free_g(ctx.root), 2),
        "trees": len(warm_targets(ctx)),
        "ws": len(ws_dirs),
        "bench": len([n for n in _listdir(os.path.join(ctx.root, "bench"))
                      if os.path.isdir(os.path.join(ctx.root, "bench", n))]),
        "load1": round(ctx.loadavg(), 2),
        "zombies": zombies,
        "reserve_hits": int(_read_state(ctx).get("reserve_hits", 0)),
        "target_g": target_g,
        "generations": gens,
        "generations_avg": gens_avg,
        "sccache_g": _g(tree_bytes(sccache)) if os.path.isdir(sccache)
        else 0.0,
        # W2 re-check: only strays past the finding age count -- the alert
        # rule reads this field, and a fresh stray is a note, not an alarm.
        "stray_targets": sum(1 for _p, _s, age in stray_targets(ctx)
                             if age > STRAY_FINDING_AGE_S),
    }
    return line


def _influx_escape(v):
    return str(v).replace(",", r"\,").replace("=", r"\=").replace(" ", r"\ ")


def influx_line(line, ts=None):
    fields = ("free_g", "trees", "ws", "bench", "load1", "zombies",
              "reserve_hits", "stray_targets")
    parts = ",".join("%s=%s" % (k, float(line.get(k) or 0)) for k in fields)
    ns = int((ts if ts is not None else time.time()) * 1e9)
    return "pipe_lane,runner=%s,state=%s %s %d" % (
        _influx_escape(line["runner"]), _influx_escape(line["state"]),
        parts, ns)


# --------------------------------------------------------------------------
# Bench areas
# --------------------------------------------------------------------------

def _script_path():
    p = globals().get("__file__")
    if p and os.path.isfile(p):
        return os.path.abspath(p)
    return None


def bench_new(ctx, name, ttl_h):
    if not re.match(r"^[A-Za-z0-9_.-]+$", name) or name in (".", ".."):
        raise ValueError("bad bench name: %r" % name)
    path = os.path.join(ctx.root, "bench", name)
    os.makedirs(path, exist_ok=True)
    ttl_s = int(round(float(ttl_h) * 3600))
    meta = {"created_at": iso(ctx.clock()), "ttl_s": ttl_s}
    with open(os.path.join(path, ".hygiene.json"), "w") as fh:
        json.dump(meta, fh)
    _ws_changed(ctx, "bench", path, None, "created", 0, ttl_s=ttl_s)
    _ws_changed(ctx, "bench", path, "created", "in_use", 0, ttl_s=ttl_s)
    unit = "pipe-bench-%s" % name
    ctx.run(["systemctl", "--user", "stop", unit + ".timer",
             unit + ".service"])
    script = _script_path()
    if script:
        cmd = _python_argv(script, "collect", "--bench", name,
                           "--root", ctx.root)
    else:
        # Piped over ssh stdin: no script to call later. A bare remove keeps
        # the TTL honest; check's TTL sweep is the second line of defence.
        ctx.diag("NOTE no script path (stdin): timer removes %s directly, "
                 "check's TTL sweep covers the rest" % path)
        cmd = ["rm", "-rf", "--", path]
    rc, _, err = ctx.run(["systemd-run", "--user", "--on-active=%ds" % ttl_s,
                          "--unit=" + unit, "--collect"] + cmd)
    if rc != 0:
        # Re-check tv-bn M7p H3: a bench without its TTL timer says so and the
        # command fails (as idle-off arm does); check's TTL sweep stays the second line.
        ctx.diag("NOTE systemd-run failed (%s): bench %s has no TTL timer, "
                 "check's TTL sweep covers it" % (err.strip(), path))
        raise BenchTimerError(path)
    return path


class BenchTimerError(RuntimeError):
    """`bench new` made the bench but could not arm its TTL timer."""

    def __init__(self, path):
        super().__init__(path)
        self.path = path


def bench_list(ctx):
    rows = []
    base = os.path.join(ctx.root, "bench")
    for name in sorted(_listdir(base)):
        path = os.path.join(base, name)
        if not os.path.isdir(path):
            continue
        meta = _bench_meta(path) or {}
        created = parse_iso(meta.get("created_at"))
        row = {"name": name, "path": path}
        if created is not None and meta.get("ttl_s") is not None:
            row["rest_s"] = int(created + float(meta["ttl_s"]) - ctx.clock())
        sb = _sidecar_bytes(path)
        if sb is not None:
            row["bytes"] = sb
        rows.append(row)
    return rows


# --------------------------------------------------------------------------
# Idle-off (R-DT-8): runs on the owner machine
# --------------------------------------------------------------------------

def read_lanes(path=None):
    path = path or os.path.expanduser("~/.config/meclaw/lanes")
    lanes = {}
    text = _default_read_text(path) or ""
    for line in text.splitlines():
        line = line.split("#", 1)[0].strip()
        if not line:
            continue
        parts = line.split()
        lane = {"name": parts[0], "ssh": parts[1] if len(parts) > 1 else ""}
        for tok in parts[2:]:
            if "=" in tok:
                k, v = tok.split("=", 1)
                lane[k] = v
            elif "bmc" not in lane:
                lane["bmc"] = tok
        if lane["ssh"] == "local":
            lane.setdefault("transport", "local")
        lanes[lane["name"]] = lane
    return lanes


def idle_off_arm(ctx, lane, after_h, lanes_file=None, holder_cmd=None,
                 power_cmd=None):
    lanes = read_lanes(lanes_file)
    info = lanes.get(lane)
    if info is None:
        ctx.diag("unknown lane %s" % lane)
        return 2
    if info.get("class") == "egon" or info.get("transport") == "local":
        ctx.diag("refused: lane %s is class=egon or local, no idle-off"
                 % lane)
        return 2
    script = _script_path()
    if not script:
        ctx.diag("idle-off arm needs the script as a file")
        return 2
    unit = "pipe-idle-%s" % lane
    secs = int(round(float(after_h) * 3600))
    ctx.run(["systemctl", "--user", "stop", unit + ".timer",
             unit + ".service"])
    cmd = _python_argv(script, "idle-off", "fire", "--lane", lane,
                                   "--after", "%g" % float(after_h)) + _idle_opts(
               lanes_file, holder_cmd, power_cmd)
    rc, _, err = ctx.run(["systemd-run", "--user", "--on-active=%ds" % secs,
                          "--unit=" + unit, "--collect"] + _idle_setenv()
                         + cmd)
    if rc != 0:
        ctx.diag("systemd-run failed: %s" % err.strip())
        return 2
    ctx.diag("idle-off armed for %s in %ds" % (lane, secs))
    return 0


def idle_rearm_on(ctx, event, lane, after_h=None, **kw):
    """Re-arm the lane's idle-off timer when a pass ended on it (W3 hook)."""
    ev = json.loads(event) if isinstance(event, str) else event
    if not ev or ev.get("event") != "pass_ended":
        return None
    if after_h is None:
        after_h = float(os.environ.get("MECLAW_IDLE_OFF_H", "2"))
    if float(after_h) <= 0:
        return None
    return idle_off_arm(ctx, lane, after_h, **kw)


# The lane's own word before a power-off (I1 of the W2 review): leases and
# `token who` only see what went through this wave's kit; a gate from a main
# tree, a direct lane_sync run or another session's build does not. So the
# host is asked over ssh, and only an explicit OK switches it off -- an error,
# a timeout or any other answer keeps it on (fail closed). Busy means: the
# legacy gate lock is held (`flock -n`, $1), a pipe-run-*/pipe-pool-* unit is
# active, or a cargo/rustc/cargo-nextest/gate.sh/test-tier.sh process of the
# login user runs (a gate between two cargo steps has no cargo process). Read
# from the command line (`ps -o args=`): the two scripts start with
# `#!/usr/bin/env bash`, their comm is always "bash" (review I1). cargo, rustc
# and cargo-nextest count as the first word, the scripts as any word ending in
# `/gate.sh` or `/test-tier.sh`. The script text never spells either name
# with a plain dot, so the probe's own command line does not count; never by
# name of the actor binary.
IDLE_PROBE_OK = "idle-probe-ok"
IDLE_PROBE_SH = r"""
lock="$1"
if [ -e "$lock" ]; then
  flock -n "$lock" true || { echo "busy: gate lock $lock held"; exit 1; }
fi
root="${2:-}"
if [ -n "$root" ]; then
  for l in "$root"/tree.lock "$root"/slot/*/tree.lock "$root"/wt/*.lock; do
    [ -e "$l" ] || continue
    flock -n "$l" true || { echo "busy: build path lock $l held"; exit 1; }
  done
fi
units=$(systemctl --user list-units --no-legend --plain \
  --state=active,activating,deactivating,reloading \
  'pipe-run-*' 'pipe-pool-*') || { echo "busy: systemctl failed"; exit 1; }
[ -z "$units" ] || { echo "busy: units $(echo $units | cut -c1-200)"; exit 1; }
procs=$(ps -u "$(id -un)" -o args=) || { echo "busy: ps failed"; exit 1; }
n=$(printf '%s\n' "$procs" | grep -cE \
  '^([^ ]*/)?(cargo|rustc|cargo-nextest)( |$)|(^|[ /])(gate|test-tier)[.]sh( |$)')
[ "$n" = 0 ] || { echo "busy: $n build processes (cargo, rustc, a gate or tier script)"; exit 1; }
echo """ + IDLE_PROBE_OK + "\n"
IDLE_PROBE_TIMEOUT_S = 30


def _engstelle():
    """`pipeline/lane.py`, the kit's narrow point for every ssh to a lane
    (re-check tv-bn M7e B-M1: the probe and the power-off built their own
    argv). Imported late: this file also runs alone on a host (`python3 -`)."""
    try:
        from pipeline import lane
    except ImportError:
        sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
        from pipeline import lane
    return lane


def idle_probe(ctx, ssh_target, root=DEFAULT_ROOT):
    """(idle?, reason) from the lane itself; anything unclear is busy. The
    build path locks under `root` count too (Delta C M-neu-1): a push+run
    holds `<tree>.lock` before any pipe-run scope or cargo process exists."""
    argv = _engstelle().ssh_argv(
        ssh_target, "sh -c %s idle-probe %s %s"
        % (shlex.quote(IDLE_PROBE_SH), shlex.quote(DEFAULT_LEGACY_LOCK),
           shlex.quote(root or DEFAULT_ROOT)), connect_timeout=10)
    try:
        rc, out, err = ctx.run(argv, timeout=IDLE_PROBE_TIMEOUT_S)
    except TypeError:  # an injected run without a timeout argument
        rc, out, err = ctx.run(argv)
    lines = (out or "").strip().splitlines()
    if rc == 0 and lines and lines[-1].strip() == IDLE_PROBE_OK:
        return True, "idle"
    why = (lines[-1] if lines else (err or "").strip()[:200]) or "no answer"
    return False, "rc=%d %s" % (rc, why)


def _idle_after_h(after_h):
    if after_h is None:
        try:
            after_h = float(os.environ.get("MECLAW_IDLE_OFF_H", "2"))
        except ValueError:
            after_h = 2.0
    return float(after_h)


def idle_rearm_later(ctx, lane, after_h, lanes_file=None, holder_cmd=None,
                     power_cmd=None):
    """After a skip: arm the lane's timer again, from a unit of its own.

    The fire runs inside `pipe-idle-<lane>.service`; `arm` stops that unit
    and starts a new one of the same name, which it cannot do from within.
    So a short one-shot timer `pipe-idle-rearm-<lane>` runs the arm once this
    fire has ended."""
    after_h = _idle_after_h(after_h)
    script = _script_path()
    if after_h <= 0 or not script:
        ctx.diag("NOTE idle-off for %s not armed again (after %s h, script %s)"
                 % (lane, after_h, script or "none"))
        return None
    cmd = _python_argv(script, "idle-off", "arm", "--lane", lane,
                                   "--after", "%g" % after_h) + _idle_opts(lanes_file, holder_cmd,
                                                   power_cmd)
    rc, _, err = ctx.run(["systemd-run", "--user", "--on-active=2s",
                          "--unit=pipe-idle-rearm-%s" % lane, "--collect"]
                         + _idle_setenv() + cmd)
    if rc != 0:
        ctx.diag("NOTE idle-off re-arm of %s failed: %s"
                 % (lane, (err or "").strip()))
        return 2
    ctx.diag("idle-off for %s armed again (%g h)" % (lane, after_h))
    return 0


def _idle_setenv():
    """systemd-run hands the unit no environment of the caller: a store
    the caller chose (MECLAW_PIPELINE_DB, BUILD-PREAMBLE 1) goes along
    explicitly; without one the unit uses the shared default store."""
    db = os.environ.get("MECLAW_PIPELINE_DB")
    return ["--setenv=MECLAW_PIPELINE_DB=%s" % db] if db else []


def _idle_opts(lanes_file, holder_cmd, power_cmd):
    """The timer's options. A command goes as ONE `--x=<shell words>`
    argument (split again with shlex): as separate words its own options
    (`ssh -o ...`) were taken by argparse and the unit died at start."""
    opts = []
    if lanes_file:
        opts += ["--lanes-file", lanes_file]
    for flag, cmd in (("--power-cmd", power_cmd), ("--holder-cmd", holder_cmd)):
        if cmd:
            cmd = list(cmd)
            opts.append("%s=%s" % (flag, cmd[0] if len(cmd) == 1
                                   else shlex.join(cmd)))
    return opts


def lease_held(store, lane, now):
    row = store.lease_get("lane:%s" % lane)
    if not row:
        return False
    taken = parse_iso(row.get("taken_at"))
    ttl = row.get("ttl_s")
    if taken is None or ttl in (None, 0):
        return True
    return now < taken + float(ttl)


def idle_off_fire(ctx, lane, lanes_file=None, holder_cmd=None,
                  power_cmd=None, store=None, after_h=None):
    info = read_lanes(lanes_file).get(lane) or {}
    if not info.get("ssh") or info.get("transport") == "local" or \
            info.get("class") == "egon":
        ctx.diag("refused: no ssh target / protected lane %s" % lane)
        return 2

    def skip(why):
        ctx.diag("NOTE idle-off skip: %s" % why)
        idle_rearm_later(ctx, lane, after_h, lanes_file, holder_cmd,
                         power_cmd)
        return 0

    store = store if store is not None else _store()
    if lease_held(store, lane, ctx.clock()):
        return skip("lease lane:%s held" % lane)
    if holder_cmd:
        argv = shlex.split(holder_cmd[0]) if len(holder_cmd) == 1 else \
            list(holder_cmd)
        rc, out, err = ctx.run(argv)
        if rc != 0:
            # Fail closed (OR-DT.W2.5): a holder command that does not run
            # cannot say the lane is free, so the lane counts as held.
            return skip("holder command failed rc=%d: %s"
                        % (rc, (err or "").strip()[:200]))
        if re.search(r"(^|[^A-Za-z0-9_.-])%s($|[^A-Za-z0-9_.-])"
                     % re.escape(lane), out or ""):
            return skip("holder names lane %s" % lane)
    idle, why = idle_probe(ctx, info["ssh"], info.get("root") or DEFAULT_ROOT)
    if not idle:
        return skip("lane probe on %s says busy or gave no answer (%s)"
                    % (lane, why))
    if power_cmd:
        argv = shlex.split(power_cmd[0]) if len(power_cmd) == 1 else \
            list(power_cmd)
    else:
        argv = _engstelle().ssh_argv(info["ssh"], ["sudo", "-n", "systemctl", "poweroff"], bare=True)
    rc, _, err = ctx.run(argv)
    if rc != 0:
        ctx.diag("power-off of %s failed rc=%d: %s" % (lane, rc,
                                                       (err or "").strip()))
        idle_rearm_later(ctx, lane, after_h, lanes_file, holder_cmd,
                         power_cmd)
        return 2
    store.runner_set_state(lane, "off", iso(ctx.clock()))
    ctx.diag("lane %s powered off (idle), runners.state=off" % lane)
    return 0


# --------------------------------------------------------------------------
# Store (core side)
# --------------------------------------------------------------------------

def _core():
    """W0a's `store` and `events` modules, next to this file. Core side only:
    piped onto a lane hygiene has no script path and never opens the store,
    so there is no fallback schema here (BUILD-PREAMBLE 1, M1 of the review)."""
    here = _script_path()
    if not here or not os.path.isfile(os.path.join(os.path.dirname(here),
                                                   "store.py")):
        raise RuntimeError("hygiene: no pipeline store next to this script "
                           "(scripts/pipeline/store.py); the store is core-side")
    repo = os.path.dirname(os.path.dirname(os.path.dirname(here)))
    if repo not in sys.path:
        sys.path.insert(0, repo)
    from scripts.pipeline import events, store  # noqa: E402 -- W0a
    return store, events


class _Store:
    """The few store operations hygiene needs, on W0a's store.py: leases via
    `lease_who`, workspaces via `workspace_set`, the journal via
    `events.ingest` (V0 validation), `runners.state` via `runner_set_state`
    (GH #1129; the schedule reads it: an `off` lane is woken, not probed)."""

    def __init__(self, conn, w0, ev):
        self.conn = conn
        self._w0 = w0
        self._ev = ev

    def lease_get(self, resource):
        return self._w0.lease_who(self.conn, resource)

    def runner_set_state(self, name, state, at):
        self._w0.runner_set_state(self.conn, name, state, at=at)

    def runner_state(self, name):
        return self._w0.runner_state(self.conn, name)

    def ingest(self, obj):
        """One stream event in ONE transaction: the journal row (validated
        once, by `events.ingest`) and, for `workspace_changed`, the move of
        its `workspaces` row commit together or not at all."""
        with self._w0.tx(self.conn):
            seq = self._ev.ingest(self.conn, obj)
            if obj["event"] == "workspace_changed":
                d = obj["data"]
                self._w0.workspace_set(self.conn, d["id"], d["runner"],
                                       d["kind"], d["path"], d["to"],
                                       run=d.get("run"), bytes=d.get("bytes"),
                                       ttl_s=d.get("ttl_s"), at=obj.get("at"))
        return seq

    def workspace_get(self, wid):
        row = self.conn.execute("SELECT * FROM workspaces WHERE id = ?",
                                (wid,)).fetchone()
        return dict(row) if row else None

    def journal_rows(self):
        return [dict(r) for r in self.conn.execute(
            "SELECT * FROM journal ORDER BY seq")]


def _store(path=None):
    """W0a's store at `path`, else `$MECLAW_PIPELINE_DB`, else its default."""
    w0, ev = _core()
    return _Store(w0.open(path), w0, ev)


def apply_events(store, lines):
    """Core side: every event of a hygiene stream into the store (see
    `_Store.ingest`). `lines` are JSONL strings or dicts; non-JSON lines are
    passed over, an event V0 refuses raises `events.EventError`. Returns the
    count."""
    n = 0
    for line in lines:
        if isinstance(line, str):
            line = line.strip()
            if not line.startswith("{"):
                continue
            try:
                obj = json.loads(line)
            except ValueError:
                continue
        else:
            obj = line
        if not isinstance(obj, dict) or not obj.get("event"):
            continue
        store.ingest(obj)
        n += 1
    return n


# --------------------------------------------------------------------------
# CLI
# --------------------------------------------------------------------------

def _add_root(p):
    p.add_argument("--root", default=None)


def _build(sp):
    p = sp.add_parser("check", help="precheck before a run")
    p.add_argument("--dispatch")
    _add_root(p)
    p.add_argument("--local", action="store_true")
    p.add_argument("--reserve-g", type=float)
    p.add_argument("--expected-g", type=float)
    p.add_argument("--own-tree")
    p.add_argument("--own-target")
    p.add_argument("--abort-below-g", type=float)
    p.add_argument("--live-run", nargs="+", action="extend")
    p.add_argument("--dry-run", action="store_true")
    p.add_argument("--runner")

    p = sp.add_parser("collect", help="collect dead workspaces")
    p.add_argument("--dispatch")
    _add_root(p)
    p.add_argument("--bench")
    p.add_argument("--dry-run", action="store_true")
    p.add_argument("--runner")

    p = sp.add_parser("evict", help="LRU-evict warm targets")
    p.add_argument("--dispatch")
    _add_root(p)
    p.add_argument("--need-g", type=float)
    p.add_argument("--max-trees", type=int)
    p.add_argument("--dry-run", action="store_true")
    p.add_argument("--runner")

    p = sp.add_parser("generations", help="evict old cargo generations")
    _add_root(p)
    p.add_argument("--target")
    p.add_argument("--keep-from")
    p.add_argument("--generation-g", type=float)
    p.add_argument("--quota-factor", type=float, default=1.2)
    p.add_argument("--dry-run", action="store_true")
    p.add_argument("--runner")

    p = sp.add_parser("kits", help="list <root>/kit/ with each kit's verdict")
    _add_root(p)
    p.add_argument("--runner")

    p = sp.add_parser("watch", help="watchdog for one run")
    p.add_argument("--run", required=True)
    p.add_argument("--scope", required=True)
    p.add_argument("--dispatch")
    _add_root(p)
    p.add_argument("--interval", type=float, default=30.0)
    p.add_argument("--runner")

    p = sp.add_parser("status", help="one JSON line about the lane")
    _add_root(p)
    p.add_argument("--runner")
    p.add_argument("--influx", action="store_true")

    p = sp.add_parser("bench", help="managed bench areas")
    bsp = p.add_subparsers(dest="bench_cmd", required=True)
    b = bsp.add_parser("new")
    b.add_argument("name")
    b.add_argument("--ttl", type=float, required=True)
    _add_root(b)
    b = bsp.add_parser("collect")
    b.add_argument("name")
    _add_root(b)
    b = bsp.add_parser("list")
    _add_root(b)

    p = sp.add_parser("idle-off", help="power a lane off when idle")
    isp = p.add_subparsers(dest="idle_cmd", required=True)
    a = isp.add_parser("arm")
    a.add_argument("--lane", required=True)
    a.add_argument("--after", type=float, required=True)
    a.add_argument("--lanes-file")
    a.add_argument("--power-cmd", nargs="+")
    a.add_argument("--holder-cmd", nargs="+")
    f = isp.add_parser("fire")
    f.add_argument("--lane", required=True)
    f.add_argument("--lanes-file")
    f.add_argument("--holder-cmd", nargs="+")
    f.add_argument("--power-cmd", nargs="+")
    f.add_argument("--after", type=float)


def register(sub):
    """argparse hook for scripts/pipeline/cli.py: `pipeline hygiene <cmd>`."""
    p = sub.add_parser("hygiene", help="lane hygiene")
    sp = p.add_subparsers(dest="hygiene_cmd", required=True)
    _build(sp)
    p.set_defaults(func=lambda args: dispatch_args(args))
    return p


def dispatch_args(args, ctx=None):
    cmd = getattr(args, "hygiene_cmd", None) or args.cmd
    dispatch = load_dispatch(getattr(args, "dispatch", None))
    ctx = ctx or Ctx(root=getattr(args, "root", None),
                     runner=getattr(args, "runner", None) or
                     (dispatch or {}).get("runner"),
                     run_id=(dispatch or {}).get("run"),
                     dry_run=getattr(args, "dry_run", False))
    if cmd == "check":
        data = check(ctx, dispatch, reserve_g=args.reserve_g,
                     expected_g=args.expected_g, own_tree=args.own_tree,
                     live_runs=args.live_run, local=args.local,
                     own_target=args.own_target,
                     abort_below_g=args.abort_below_g)
        return 2 if data["verdict"] == "aborted" else 0
    if cmd == "collect":
        notes = []
        collect(ctx, dispatch, bench=args.bench, notes=notes)
        for n in notes:
            ctx.diag("NOTE " + n)
        return 0
    if cmd == "kits":
        notes = []
        stream = ctx.out or sys.stdout
        for row in kits(ctx, notes):
            stream.write(json.dumps(row, sort_keys=True) + "\n")
        stream.flush()
        for n in notes:
            ctx.diag("NOTE " + n)
        return 0
    if cmd == "evict":
        notes = []
        evict(ctx, dispatch, need_g=args.need_g, max_trees=args.max_trees,
              notes=notes)
        for n in notes:
            ctx.diag("NOTE " + n)
        return 0
    if cmd == "generations":
        for r in generations(ctx, target=args.target,
                             keep_from=args.keep_from,
                             generation_g=args.generation_g,
                             quota_factor=args.quota_factor):
            ctx.diag("generations %s: evicted %.2f G in %d groups"
                     % (r["target"], r["evicted_g"], r["groups"]))
        return 0
    if cmd == "watch":
        ctx.run_id = args.run
        return watch(ctx, args.run, args.scope, dispatch, args.interval)
    if cmd == "status":
        line = status(ctx)
        out = ctx.out or sys.stdout
        out.write((influx_line(line, ctx.clock()) if args.influx
                   else json.dumps(line, sort_keys=True)) + "\n")
        return 0
    if cmd == "bench":
        out = ctx.out or sys.stdout
        if args.bench_cmd == "new":
            try:
                path = bench_new(ctx, args.name, args.ttl)
            except BenchTimerError as exc:
                out.write(exc.path + "\n")
                return 2
            out.write(path + "\n")
        elif args.bench_cmd == "collect":
            if legacy_lock_held(ctx):
                ctx.diag("NOTE legacy-run: bench collect skipped")
            else:
                collect_bench(ctx, args.name)
        else:
            for row in bench_list(ctx):
                out.write(json.dumps(row, sort_keys=True) + "\n")
        return 0
    if cmd == "idle-off":
        if args.idle_cmd == "arm":
            return idle_off_arm(ctx, args.lane, args.after, args.lanes_file,
                                holder_cmd=args.holder_cmd,
                                power_cmd=args.power_cmd)
        return idle_off_fire(ctx, args.lane, args.lanes_file,
                             holder_cmd=args.holder_cmd,
                             power_cmd=args.power_cmd, after_h=args.after)
    raise SystemExit("unknown command %r" % cmd)


def main(argv=None, ctx=None):
    parser = argparse.ArgumentParser(prog="hygiene.py",
                                     description=__doc__.splitlines()[0])
    sp = parser.add_subparsers(dest="cmd", required=True)
    _build(sp)
    args = parser.parse_args(argv)
    if ctx is not None and getattr(args, "root", None):
        ctx.root = os.path.abspath(args.root)
    return dispatch_args(args, ctx)


if __name__ == "__main__":
    sys.exit(main())
