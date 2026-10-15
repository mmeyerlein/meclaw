"""meclaw -- the scheduler of the pipeline (W3): runners, admission, rounds, ingest, leases.

`runners` holds every lane of the host file (`sync_runners`): its capacity
vector, class and state. `admit` picks the lane of one pass -- the lane the
strand holds a lease on, else a lane with a warm target, else the first free
one in host order -- checks the capacity vector and the tree rule (never two
runs in one tree), reserves disk, opens or reopens the workpackage of the pass
(one round per gate on the same (repo, mode, strand|wave, base)), writes one
task per station and one `task_runs` attempt each, and hands back the
dispatch. `ingest` folds the executor's event stream into the store and
`close` ends the pass.

The leases are the cargo token of `strand.sh token`, carried over to the
store with the same queue, TTL and offer semantics (GH #1053): the holders
are `leases` rows (`lane:<name>` or `token:<n>`), the queue is the additive
table `lease_entries`, the log is the journal (`lease_taken`,
`lease_released`, `lease_offered`, and `lease_noted` for every other line;
`data.verb` keeps the old log words). `pipeline.py lanes <verb>` is the CLI.

Additive store parts owned here (W0a's `store.MIGRATIONS` stays untouched;
OR candidate: move them into a migration row n=2): tables `lease_pools` and
`lease_entries`, columns `runners.root` and `runners.spec` (the host-file
words of a lane: threads, jobs, slots, opts, position).

Items move along W0a's automata only (`store.TRANSITIONS`, one Orga step at a
time): a new workpackage is `planned` and goes `active`, a new task is `open`
and goes `in_progress`; a reopened task goes `done -> open -> in_progress`.
"""
import argparse
import calendar
import contextlib
import dataclasses
import json
import math
import os
import re
import secrets
import select
import socket
import sqlite3
import stat
import subprocess
import sys
import time
import uuid
from typing import Optional

from . import cpus, events, gitenv, store

CAPACITY_KEYS = ("cpu", "colony", "browser_chromium", "browser_webkit", "export", "cargo")
# Orga's seven exec keys (`shared/exec.py` EXEC_KEYS); everything else lives in `doc.station`.
EXEC_KEYS = ("runner", "root", "test_argv", "workspace", "timeout_min", "retries", "evidence")
FIXED_CAPACITY = {"browser_chromium": 1, "browser_webkit": 1, "export": 1, "cargo": 1}
BOOT_S = 300
LEASE_GRACE_S = 1800          # lease TTL = critical path of the timeouts + 30 min
DEFAULT_TIMEOUT_MIN = 30      # a station without `timeout_min`, as in the executor
# Outside the stations but inside the deadline: the push to the lane, the wait
# for the cargo lock, and the tree sync under it (up to 900 s, cargo-target.sh).
DEADLINE_OVERHEAD_S = 1500
# An `active` workpackage past its deadline plus this grace has no kern any
# more (the executor aborts at the deadline, kills after 10 s + `timeout -k 30`):
# it is no live run, and the next admission closes it as lost (review A C1).
ORPHAN_GRACE_S = 600
DEFAULT_EXPECTED_G = 35       # a cold target (W2 point 3)
WS_EXPECTED_G = 20            # a run without cargo: its workspace only
DEFAULT_BUDGET = {"free_min_g": 30, "reserve_g": 60, "max_trees": 4, "export_g": 40,
                  "ws_g": 20, "bench_g": 60, "pool_g": 10}
# `cpu` may be overbooked by this factor (W3 plan: 1.25 until measured). 1.5 is
# the start value since the pre-acceptance on build03 (GR-ii): with the plan's
# start needs, `tests` (nproc) and `clippy` (nproc/2) sum to 1.5 nproc, and at
# 1.25 clippy always queued behind tests. OR candidate until the A6 measurement.
CPU_OVERBOOK = 1.5
EGON_MODES = ("test", "measure")
DEFAULT_ROOT = "/srv/target"
DONE_VERDICTS = ("GREEN", "CACHED", "NOTE", "SKIP")
NO_CALIBRATION = ("CACHED", "SKIP", "ABORTED")
REASON_CODE = re.compile(r"^[a-z_]{1,40}$")

POOL = "cargo"
OFFER_MIN = 10   # one poll period, GH #1053 (see strand.sh)
# Fund 03:1x (welle-testing, R-TV-W21 b): a lease without a live run and without
# a live registered process (the builder ended without `token release`) held
# build03 101 min while a waiter for it stood behind. Once a waiter wants the
# lane, such an idle lease goes back after this short TTL instead of the hold
# TTL -- fail-closed: never under a live run or a live process.
LEASE_IDLE_MIN = 15
TOKEN_VERBS = ("take", "release", "who", "check", "init", "off", "lane", "offer", "status")
# Z11 (2026-10-09): after the W3 merge `take` on an unarmed store said "not
# armed" with exit 0 and the lanes ran unserialised for twelve minutes. A take
# asks to serialise cargo; nothing to serialise it with is an error.
ARM_HINT = ("arm it with 'scripts/strand.sh token init --lanes <a,b,...> --ttl <min>' "
            "(or '--max <n>' without lanes)")
# The old token file of `strand.sh` (its `token_file`): `MECLAW_STRAND_TOKENS`,
# else `${MECLAW_GATE_LOCK%.lock}.tokens` with this lock as the default.
OLD_LOCK = "/tmp/meclaw-w26-cargo.lock"
REFUSED = ("no cargo token for %s -- run 'scripts/strand.sh token take' first. "
           "Without a token: write code, tests and docs without cargo, then end "
           "your turn and wait to be woken (plans/PREAMBLE.md section 5).")

# Tests replace the lane module of `lanes status` (load_lanes, status).
_lane_override = None

# Kit find (k), welle-testing 2026-10-11 15:3x (R-TV-W43): a builder ran
# cargo-mutants by hand over ssh in the lane tree of build04 under its lease;
# the lease watcher saw no runner run (pid -), reclaimed the lease as stale and
# gave build04 to another strand, whose `pre` built the mutated files. A stale
# lane lease therefore moves only after a probe of the lane's root (`lane.root`:
# `tree/`, `slot/<n>/`, `wt/<tree>` all live below it) found no process there --
# a cwd below it or the path in a command line (`lane.ws_procs`). A process probe,
# not a marker: a hand run is exactly the run that carries no marker.
# Tests replace the probe: a callable `(lane.Lane) -> [pid, ...] | None`.
_lane_tree_probe = None
_UNPROBED = object()

ADDITIVE_DDL = (
    "CREATE TABLE IF NOT EXISTS lease_pools (pool TEXT PRIMARY KEY, max INTEGER, "
    "ttl_min INTEGER, armed TEXT, lanes TEXT)",
    "CREATE TABLE IF NOT EXISTS lease_entries (seq INTEGER PRIMARY KEY AUTOINCREMENT, "
    "pool TEXT, holder TEXT, resource TEXT, want TEXT, since TEXT, seen TEXT, pids TEXT, "
    "tree TEXT, offered TEXT, offered_at TEXT, ttl_min INTEGER, run TEXT)",
    # Re-Check tv-kit H2: an older core rewrites `lease_entries` with its fixed
    # column list and drops `hold_min`; the hold lives here as well, keyed by
    # the lease (holder, since), in a table no older core touches.
    "CREATE TABLE IF NOT EXISTS lease_holds (pool TEXT, holder TEXT, since TEXT, "
    "hold_min INTEGER, PRIMARY KEY (pool, holder, since))",
    # GH #1199 (kit find V2-A8): the last progress line of the holder's run (or
    # its refusal), keyed by the lease like `lease_holds` -- `who` reads it per
    # holder by its key; the journal would be a scan over every row (113 547 on
    # 14.10.), and a column of `lease_entries` an older core's `_save` drops.
    "CREATE TABLE IF NOT EXISTS lease_progress (pool TEXT, holder TEXT, since TEXT, "
    "at TEXT, state TEXT, line TEXT, PRIMARY KEY (pool, holder, since))",
)
# `lease_entries.hold_min`: the explicit `take --ttl` of a holder -- a lease
# held on purpose, which the idle reclaim (R-TV-W21 b) leaves to its hold TTL.
ADDITIVE_COLUMNS = (("runners", "root", "TEXT"), ("runners", "spec", "TEXT"),
                    ("lease_entries", "hold_min", "INTEGER"))


# --- small helpers -------------------------------------------------------------------------

def _iso(t):
    """RFC 3339 UTC of an epoch second."""
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(int(t)))


def _epoch(s):
    """Epoch second of an RFC 3339 UTC stamp (`None` -> 0)."""
    if not s:
        return 0
    return calendar.timegm(time.strptime(s, "%Y-%m-%dT%H:%M:%SZ"))


def _now(now):
    return int(time.time() if now is None else now)


def _join(lines):
    return "\n".join(lines) + "\n" if lines else ""


def _ensure(db):
    """Create the additive tables and columns of this module (idempotent, race-safe)."""
    for ddl in ADDITIVE_DDL:
        db.execute(ddl)
    for table, column, kind in ADDITIVE_COLUMNS:
        cols = {r[1] for r in db.execute("PRAGMA table_info(%s)" % table)}
        if column not in cols:
            try:
                db.execute("ALTER TABLE %s ADD COLUMN %s %s" % (table, column, kind))
            except sqlite3.OperationalError as exc:   # a parallel caller added it first
                if "duplicate column" not in str(exc):
                    raise


class _Exit(Exception):
    def __init__(self, code):
        super().__init__(code)
        self.code = code


@contextlib.contextmanager
def _tx(db):
    """One `BEGIN IMMEDIATE` transaction; nested calls join the outer one. A verb
    that ends with an exit code commits what it wrote before (the old log did too)."""
    if db.in_transaction:
        yield
        return
    db.execute("BEGIN IMMEDIATE")
    try:
        yield
    except _Exit:
        db.execute("COMMIT")
        raise
    except BaseException:
        db.execute("ROLLBACK")
        raise
    db.execute("COMMIT")


# --- W0a writes inside one open transaction ------------------------------------------------
# A verb here is ONE transaction (`_tx`). W0a's helpers (`events.emit`,
# `store.item_create`, `store.item_transition`) each open their own BEGIN
# IMMEDIATE, which sqlite refuses inside an open one; inside `_tx` the same
# rows go in with the helpers' SQL form below (same checks, same automata).

def _journal(db, event, kind, data, *, run=None, item=None, at=None, text=None):
    """`events.emit`, also inside an open `_tx`: validated against V0 section 3."""
    if not db.in_transaction:
        return events.emit(db, event, kind, data, run=run, item=item, at=at, text=text)
    events.validate(event, kind, data)
    if kind not in store.JOURNAL_KINDS:
        raise ValueError("unknown journal kind %r" % kind)
    at = at or store.now()
    text = text or events.summary_text(event, data)
    seq = db.execute("INSERT INTO journal (at, item, run, kind, event, text, data) "
                     "VALUES (?, ?, ?, ?, ?, ?, ?)",
                     (at, item, run, kind, event, text, json.dumps(data, sort_keys=True))).lastrowid
    row = {"seq": seq, "at": at, "item": item, "run": run, "kind": kind, "event": event,
           "text": text, "data": data}
    for hook in events.HOOKS:
        try:
            hook(db, row)
        except Exception:  # noqa: BLE001 -- a hook never fails the run (as in emit)
            pass
    return seq


def _item_new(db, type_, title, parent, doc, reason, at):
    """`store.item_create` inside the open transaction: id `it-<hex>`, the next
    `P-<n>`/`T-<n>` key, the type's initial state. Returns (id, key)."""
    item_id = "it-%s" % uuid.uuid4().hex[:12]
    key = store._next_key(db, store.KEY_PREFIX[type_])
    db.execute("INSERT INTO items (id, key, type, title, state, reason, parent, doc, version, "
               "created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, 1, ?, ?)",
               (item_id, key, type_, title, store.INITIAL_STATE[type_], reason, parent,
                json.dumps(doc, sort_keys=True), at, at))
    return item_id, key


def _path(automaton, src, dst):
    """The shortest walk of Orga steps from `src` to `dst` (`[]` when equal)."""
    seen, todo = {src: []}, [src]
    while todo:
        cur = todo.pop(0)
        if cur == dst:
            return seen[cur]
        for nxt in sorted(automaton.get(cur, ())):
            if nxt not in seen:
                seen[nxt] = seen[cur] + [nxt]
                todo.append(nxt)
    raise ValueError("no transition %s -> %s" % (src, dst))


def _item_walk(db, item_id, to_state, reason, at, doc=None):
    """`store.item_transition` inside the open transaction, step by Orga step
    (task done -> open -> in_progress on a reopen, workpackage planned ->
    active), compare-and-set on state and version; a move to the state the item
    is in only changes reason (and doc). Returns the new version."""
    row = db.execute("SELECT type, state, version FROM items WHERE id = ?", (item_id,)).fetchone()
    if row is None:
        raise KeyError(item_id)
    state, version = row["state"], row["version"]
    steps = _path(store.TRANSITIONS[row["type"]], state, to_state) or [state]
    for i, nxt in enumerate(steps):
        last = i == len(steps) - 1
        sets, args = ["state = ?", "reason = ?", "version = version + 1", "updated_at = ?"], \
            [nxt, reason, at]
        if last and doc is not None:
            sets.append("doc = ?")
            args.append(json.dumps(doc, sort_keys=True))
        cur = db.execute("UPDATE items SET %s WHERE id = ? AND state = ? AND version = ?"
                         % ", ".join(sets), args + [item_id, state, version])
        if cur.rowcount != 1:
            raise store.Conflict("%s: not %s@%s" % (item_id, state, version))
        state, version = nxt, version + 1
    return version


class ScheduleError(Exception):
    """A request the scheduler refuses outright (exit 2): an unknown lane and the like."""

    def __init__(self, message, code=2):
        super().__init__(message)
        self.code = code


@dataclasses.dataclass
class Queued:
    """No lane takes the pass now (exit 3); `power_requested` names a lane asked to boot."""
    reason: str
    power_requested: Optional[str] = None
    code: int = 3
    final: bool = False     # no event and no watchdog round resolves it (Review Z-A2 I1)


@dataclasses.dataclass
class Admission:
    """An admitted pass: the lane, the run, the workpackage round and the dispatch."""
    lane: str
    run: str
    deadline_at: str
    ttl_s: int
    capacity: dict
    workpackage: str
    round: int
    tasks: dict
    task_runs: dict
    tree: str
    cargo: bool
    expected_g: int
    dispatch: dict
    lease_taken: bool = False
    item: int = 0
    root: str = DEFAULT_ROOT
    ws: str = ""
    repo: str = ""
    mode: str = ""
    strand: str = ""
    wave: str = ""
    base: str = ""
    # NOTE lines of the admission, one per entry (`gate: NOTE <entry>`)
    notes: list = dataclasses.field(default_factory=list)


@dataclasses.dataclass
class LeaseResult:
    """The outcome of one lease verb: exit code, stdout, stderr, the lane held or taken."""
    code: int
    out: str = ""
    err: str = ""
    lane: Optional[str] = None


# --- runners -------------------------------------------------------------------------------

# The real cores of a topology live in `cpus` (GH #1199, re-check A-I1): the kit's
# one CPU source, read from MECLAW_FP_SYSROOT under `--kit`. Kept here by name for
# the callers and tests that ask the scheduler.
physical_cores = cpus.physical_cores


def runner_capacity(lane, fingerprint):
    """A runner's capacity: cpu in threads (`_runner_cpu`: `threads=`, else the
    fingerprint's vcpu), colonies from ONE source, `colony_capacity` (welle-tb S5):
    the lane's `cores=` in the host file, else the fingerprint's vcpu / 2, else this
    host's -- the fingerprint's own `cores` count is never read here (re-check
    tb-s3 h1 m-3: two counters disagreed on order); an engine only where it starts
    (`browsers` of the fingerprint, OR-TB.S3.17)."""
    cap = dict(FIXED_CAPACITY, cpu=_runner_cpu(lane, fingerprint),
               colony=colony_capacity(getattr(lane, "cores", 0), (fingerprint or {}).get("vcpu")))
    for engine in browser_refusals(fingerprint):
        cap["browser_" + engine] = 0
    return {k: cap[k] for k in CAPACITY_KEYS}


def browser_refusals(fingerprint):
    """The engines a lane's fingerprint says do not start, with the reason (GH #1161:
    build02's WebKit could not load libGL.so.1 and every WebKit proof SKIPped green).
    A fingerprint from before the probe says nothing here: the executor probes before
    the start and a booked engine that does not start is RED there."""
    said = (fingerprint or {}).get("browsers")
    if not isinstance(said, dict):
        return {}
    return {e: str(said.get(e) or "not probed") for e in ("chromium", "webkit")
            if said.get(e) != "ok"}


def _runner_cpu(lane, fingerprint):
    vcpu = (fingerprint or {}).get("vcpu")
    if getattr(lane, "cls", "lane") == "egon":
        # Half the cores, never over the Egon cap the executor runs with
        # (`executor._book_threads`): a station books exactly what it runs. With
        # half the cores alone, an egon host of nproc 12 booked 6 and ran 4.
        from .executor import EGON_MAX_THREADS  # lazy: the executor imports schedule lazily
        return min(max(1, int(vcpu or cpus.threads()) // 2), EGON_MAX_THREADS)
    # Before the lane's first `lane_fingerprint` and without `threads=` the
    # kern's own count stands in (as for egon); cpu 1 made the first run serial.
    return int(getattr(lane, "threads", None) or vcpu or cpus.threads())


def colony_capacity(cores=None, vcpu=None):
    """The colonies a lane hosts side by side (GH #1163, welle-tb S5): one per
    real core (`cores=` in the host file; unknown: vCPU / 2), at least the
    widest fixed colony booking of the core's own stations (`plan.NEEDS`, the
    scenario runners' width) so those stay admissible. The old fixed 8 (a W3
    start value, OR-DT-11) was never measured."""
    from . import plan
    widest = max([int(n.get("colony", 0)) for n in plan.NEEDS.values()] + [1])
    real = int(cores or 0) or max(1, int(vcpu or cpus.threads()) // 2)
    return max(real, widest)


def sync_runners(db, lanes, *, states=None, now=None):
    """Register the lanes of the host file in `runners` (party, capacity, class, state).

    A new ssh lane starts `off`, a local one `idle`; a known state stays unless
    `states` names one; a runner no longer in the host file is `drained`."""
    _ensure(db)
    states = states or {}
    at = _iso(_now(now))
    with _tx(db):
        seen = set()
        for pos, lane in enumerate(lanes):
            row = db.execute("SELECT * FROM runners WHERE name = ?", (lane.name,)).fetchone()
            fp = json.loads(row["fingerprint"]) if row and row["fingerprint"] else {}
            cap = runner_capacity(lane, fp)
            if lane.name in states:
                state = states[lane.name]
            elif row and row["state"] and row["state"] != "drained":
                state = row["state"]
            else:
                state = "idle" if lane.transport == "local" else "off"
            spec = {"threads": getattr(lane, "threads", None) or 0,
                    "jobs": getattr(lane, "jobs", None) or 0,
                    "slots": getattr(lane, "slots", None) or 1,
                    "bmc": getattr(lane, "bmc", None) or "",
                    "opts": dict(getattr(lane, "opts", None) or {}), "pos": pos}
            changed = row is None or row["state"] != state
            db.execute(
                "INSERT INTO runners (name, party, capacity, host, transport, class, state, boot_s, "
                "fingerprint, updated_at, root, spec) VALUES (?,?,?,?,?,?,?,?,?,?,?,?) "
                "ON CONFLICT(name) DO UPDATE SET party=excluded.party, capacity=excluded.capacity, "
                "host=excluded.host, transport=excluded.transport, class=excluded.class, "
                "state=excluded.state, boot_s=excluded.boot_s, root=excluded.root, spec=excluded.spec"
                + (", updated_at=excluded.updated_at" if changed else ""),
                (lane.name, "lane:%s" % lane.name, json.dumps(cap), lane.ssh, lane.transport,
                 lane.cls, state, BOOT_S, row["fingerprint"] if row else None, at,
                 lane.root or DEFAULT_ROOT, json.dumps(spec, sort_keys=True)))
            seen.add(lane.name)
        for row in db.execute("SELECT name, state FROM runners").fetchall():
            if row["name"] not in seen and row["state"] != "drained":
                db.execute("UPDATE runners SET state = 'drained', updated_at = ? WHERE name = ?",
                           (at, row["name"]))


def _runners(db):
    rows = db.execute("SELECT rowid AS rid, * FROM runners").fetchall()

    def pos(r):
        try:
            return json.loads(r["spec"] or "{}").get("pos", 1 << 30), r["rid"]
        except ValueError:
            return 1 << 30, r["rid"]
    return sorted(rows, key=pos)


def _lane_of(row):
    """A `lane.Lane` rebuilt from a runner row (for `lane.env_for`)."""
    from . import lane as lane_mod
    spec = json.loads(row["spec"] or "{}")
    return lane_mod.Lane(name=row["name"], ssh=row["host"] or "", bmc=spec.get("bmc", ""),
                         root=row["root"] or DEFAULT_ROOT, transport=row["transport"] or "ssh",
                         cls=row["class"] or "lane", threads=spec.get("threads") or 0,
                         jobs=spec.get("jobs") or 0, slots=spec.get("slots") or 1,
                         opts=spec.get("opts") or {})


# --- plan facts ----------------------------------------------------------------------------

def _needs(st):
    needs = dict(st.get("needs") or {})
    if "needs" not in st:  # legacy plan without `needs`: the old `cargo` flag means the lock
        needs["cargo"] = int(st.get("cargo") or 0)
    if st.get("name") == "build":  # Z17: the one cargo rule (`gate._needs_cargo` asks here)
        needs["cargo"] = max(1, int(needs.get("cargo") or 0))
    return {k: int(needs.get(k) or 0) for k in CAPACITY_KEYS}


def _demand(stations):
    """The peak per resource over the stations (they run one after another or side by side
    inside the executor, which keeps its own sum under the dispatched capacity)."""
    peak = {k: 0 for k in CAPACITY_KEYS}
    for st in stations:
        for k, v in _needs(st).items():
            peak[k] = max(peak[k], v)
    return peak


def check_cpu(stations):
    """Refuses (exit 2) a plan with a station whose `needs.cpu` is no thread count: a
    negative value other than the symbols, None, a missing key or a legacy plan without
    `needs` booked 0 cpu and ran beside anything."""
    from . import plan as plan_mod
    for st in stations:
        needs = st.get("needs")
        if not isinstance(needs, dict):
            raise ScheduleError("station %s has no needs -- a legacy plan books no cpu; "
                                "build the plan again" % st.get("name"))
        cpu = needs.get("cpu", "<missing>")
        ok = (isinstance(cpu, int) and not isinstance(cpu, bool)
              and (cpu >= 1 or cpu in plan_mod.CPU_SYMBOLS))
        if not ok:
            raise ScheduleError("station %s: needs.cpu %r is no thread count -- refused, "
                                "the pass would book no cpu" % (st.get("name"), cpu))


def _core_repo():
    from . import plan as plan_mod
    return plan_mod.REPO


def _demand_on(stations, lane_cpu, lane_colony=None):
    """`_demand` on a lane with `lane_cpu` threads: every `needs.cpu` (the symbols
    `CPU_LANE`, `CPU_SHARDS` and plain counts) resolves to the threads it takes there,
    at most the lane (`check_cpu` has refused anything else); `COLONY_REST` to the
    slots it takes beside browser on a lane with `lane_colony` colonies."""
    from . import plan as plan_mod
    out = []
    for st in stations:
        needs = st.get("needs") or {}
        if "cpu" in needs:
            cpu = plan_mod.resolve_cpu(needs["cpu"], int(lane_cpu or 0))
            st = dict(st, needs=dict(needs, cpu=cpu))
        if lane_colony is not None and "colony" in needs:
            st = dict(st, needs=dict(st["needs"], colony=plan_mod.resolve_colony(
                needs["colony"], int(lane_colony))))
        out.append(st)
    return _demand(out)


def _critical_min(stations):
    """The longest chain of `timeout_min` over the `after` edges."""
    by = {s["name"]: s for s in stations}
    memo = {}

    def walk(name, trail):
        if name in memo:
            return memo[name]
        if name in trail:
            return 0
        st = by[name]
        prev = [walk(a, trail | {name}) for a in st.get("after") or [] if a in by]
        memo[name] = float(st.get("timeout_min") or DEFAULT_TIMEOUT_MIN) + max(prev or [0])
        return memo[name]
    return max([walk(n, frozenset()) for n in by] or [0])


# --- workpackages and live runs ------------------------------------------------------------

def _find_wp(db, repo, mode, strand, wave, base):
    if strand:
        who, val = "$.strand", strand
    else:
        who, val = "$.wave", wave
    return db.execute(
        "SELECT * FROM items WHERE type = 'workpackage' AND json_extract(doc, '$.repo') = ? "
        "AND json_extract(doc, '$.mode') = ? AND json_extract(doc, '$.base') = ? "
        "AND json_extract(doc, ?) = ? %s ORDER BY id DESC LIMIT 1"
        % ("" if strand else "AND coalesce(json_extract(doc, '$.strand'), '') = ''"),
        (repo, mode, base, who, val)).fetchone()


def _orphaned(doc, now):
    """An `active` workpackage whose kern is gone: past deadline + grace."""
    return now >= _epoch(doc.get("deadline_at")) + ORPHAN_GRACE_S


def _close_orphans(db, now):
    """Close the `active` workpackages no kern will close (the kern died between
    `admit` and `close`) as `stopped/lost`, like `close(lost=True)`: their open
    task runs fail `lost`, their tasks wait; the runner is idle when nothing
    else runs on it."""
    at = _iso(now)
    lanes = set()
    for row in db.execute("SELECT id, doc FROM items WHERE type = 'workpackage' AND "
                          "state = 'active'").fetchall():
        doc = json.loads(row["doc"] or "{}")
        if not doc.get("run") or not doc.get("deadline_at") or not _orphaned(doc, now):
            continue
        _item_walk(db, row["id"], "stopped", "lost", at)
        for tr in db.execute(
                "SELECT task_runs.id AS id, task_runs.item AS item FROM task_runs "
                "JOIN items ON items.id = task_runs.item WHERE items.parent = ? "
                "AND task_runs.state = 'dispatched'", (row["id"],)).fetchall():
            db.execute("UPDATE task_runs SET state = 'failed', reason = 'lost', lost = 1, "
                       "ended_at = ? WHERE id = ?", (at, tr["id"]))
            _item_walk(db, tr["item"], "waiting", "lost", at)
        lanes.add(doc.get("lane"))
    busy = {x["lane"] for x in _live_runs(db, now)}
    for lane in lanes - busy - {None}:
        db.execute("UPDATE runners SET state = 'idle', updated_at = ? WHERE name = ? "
                   "AND state = 'busy'", (at, lane))


def _live_runs(db, now):
    """Passes that may still run: workpackages `active` (until `close`, at most
    until deadline + grace), and `stopped/lost` ones until their deadline (the
    lane may still build; the executor aborts a run at its deadline)."""
    live = []
    for row in db.execute(
            "SELECT id, key, state, doc FROM items WHERE type = 'workpackage' AND "
            "(state = 'active' OR (state = 'stopped' AND reason = 'lost'))"):
        doc = json.loads(row["doc"] or "{}")
        if not doc.get("run") or not doc.get("lane"):
            continue
        if row["state"] == "stopped" and now >= _epoch(doc.get("deadline_at")):
            continue
        if row["state"] == "active" and doc.get("deadline_at") and _orphaned(doc, now):
            continue
        live.append({"item": row["id"], "key": row["key"], "run": doc["run"], "lane": doc["lane"],
                     "tree": doc.get("tree"), "demand": doc.get("demand") or {},
                     "expected_g": doc.get("expected_g") or 0})
    return live


def _fit(cap, live_here, demand, tree):
    """Why a pass does not fit beside the live runs of a lane, or ""."""
    same = [r for r in live_here if r["tree"] == tree]
    if same:
        why = ("one cargo run in the build path tree/ at a time" if tree == "tree"
               else "a push would move the tree under a live run")
        return "%s is in use by %s (%s)" % (tree, same[0]["run"], why)
    held = {k: sum(int(r["demand"].get(k) or 0) for r in live_here) for k in CAPACITY_KEYS}
    over = ["%s %d+%d > %d" % (k, held[k], demand[k], cap.get(k, 0)) for k in CAPACITY_KEYS
            if demand[k] and held[k] + demand[k] > cap.get(k, 0)]
    return ("needs " + ", ".join(over)) if over else ""


def _target_g(db, lane):
    row = db.execute(
        "SELECT bytes FROM workspaces WHERE runner = ? AND kind = 'target' AND "
        "state IN ('warm', 'in_use') AND bytes IS NOT NULL ORDER BY used_at DESC LIMIT 1",
        (lane,)).fetchone()
    return int(math.ceil(row["bytes"] / float(1 << 30))) if row else None


# --- admission -----------------------------------------------------------------------------

def admit(db, plan, *, repo, mode, strand, wave, base, lane_pref, now, wait, tree_name=None,
          wait_tree="", any_lane=False, queue=True):
    """Admit one pass: `Admission`, or `Queued` (exit 3), or `ScheduleError` (exit 2).

    Admission never blocks: a queued strand keeps its place in the lease
    queue; `wait` only words the queue line for a caller that blocks itself
    (`admit_waiting`, GH #1145), `wait_tree` is that caller's tree: the place
    stays while the tree does (R2 M3). integration and release take only the
    widest lane unless `any_lane` (`WIDEST_MODES`). `queue=False` (the merge
    gate, which never comes back for an offer) and a pass without a holder take
    no queue place: a waiter that never takes its offer blocks the pool (kit
    find (h), ledger 03:35)."""
    _ensure(db)
    now = _now(now)
    lane_procs = _probe_stale_lanes(db, strand or wave or "", lane_pref or "", now, batch=True)
    with _tx(db):
        return _admit(db, plan, repo=repo, mode=mode, strand=strand or "", wave=wave or "",
                      base=base, lane_pref=lane_pref, now=now, tree_name=tree_name, wait=wait,
                      wait_tree=wait_tree, any_lane=any_lane, queue=queue,
                      lane_procs=lane_procs)


def wait_secs(env, name, default):
    """A wait in seconds from `env[name]`: above 0 and below 2^31, else exit 2 (Review Z-A2)."""
    raw = env.get(name) or default
    try:
        secs = float(raw)
    except ValueError:
        secs = 0.0
    if not 0 < secs < 2 ** 31:      # nan fails both
        raise ScheduleError("%s=%s is no number of seconds above 0 and below 2^31" % (name, raw))
    return secs


def admit_waiting(db, plan, *, secs=None, on_queued=None, **kw):
    """`admit` until it is no longer queued (`gate --wait-admission`, GH #1145).

    No retry clock (R-AG-1): the wake file of the pool is opened before each
    round and blocked on after a queued one -- a lease release, a granted take
    and the close of a run wake it (`_wake`). One watchdog per round
    (`secs`, else `MECLAW_TOKEN_WAIT_SECS`, 30 s) covers the causes without an
    event (a TTL running out, a lane booting). A cause no round resolves (a
    lane off, an egon lane, no lane at all: `Queued.final`) ends the wait at
    once, and the whole wait ends after `MECLAW_ADMISSION_WAIT_SECS` (4 h) --
    each as a final `Queued` that has left the queue (Review Z-A2 I1).
    `on_queued(Queued)` hears the first queued round only; a `ScheduleError`
    ends the wait as it is."""
    secs = float(secs) if secs is not None else wait_secs(os.environ, "MECLAW_TOKEN_WAIT_SECS", "30")
    limit = wait_secs(os.environ, "MECLAW_ADMISSION_WAIT_SECS", "14400")
    end = time.monotonic() + limit
    db_path, said, adm = _db_path(db), False, None
    try:
        while True:
            fd = _wake_open(db_path)
            try:
                adm = admit(db, plan, wait=True, **kw)
                if not isinstance(adm, Queued) or adm.final:
                    return adm
                if not said and on_queued:
                    on_queued(adm)
                said = True
                left = end - time.monotonic()
                if left <= 0:
                    adm = Queued("%s -- no admission within %g s (MECLAW_ADMISSION_WAIT_SECS)"
                                 % (adm.reason, limit), final=True)
                    return adm
                select.select([fd] if fd is not None else [], [], [], min(secs, left))
            finally:
                if fd is not None:
                    os.close(fd)
    finally:
        if not isinstance(adm, Admission):
            _leave_queue(db, kw.get("strand") or kw.get("wave") or "")


def _leave_queue(db, key):
    """A waiter that stops waiting gives its queue place up (R2 M3); a busy store keeps it to its TTL."""
    ctx = _Ctx(db, _now(None), branch=key)
    try:
        with _tx(db):
            st = _load(db)
            if not st or key not in [w["strand"] for w in st["waiting"]]:
                return
            st["waiting"] = [w for w in st["waiting"] if w["strand"] != key]
            _save(db, st, ctx)
            ctx.log("leave", key, "the admission stopped waiting")
    except sqlite3.Error:
        return
    _wake(_db_path(db))


# GH #1162: integration and release run the full suite (release) or the wave's
# diff over every crate (integration); their reference minima are 24-thread
# values (M0 A4 j24 469.7 s against j12 628.2 s on the same lane). On a 12-thread
# lane the integration pass 7b1e0e5 took 2092 s. They wait for the widest lane
# rather than fall back to a narrower one; `--any-lane` is the switch.
WIDEST_MODES = ("integration", "release")


def _lane_cpu(row):
    try:
        return int(json.loads(row["capacity"] or "{}").get("cpu") or 0)
    except (TypeError, ValueError):
        return 0


def widest(runners):
    """The thread count of the widest runner a pass can ever take (not drained,
    not unreachable -- `_admit` skips both --, no egon), and the names that have
    it -- in the order given (GH #1162 hA-m-5)."""
    cands = [r for r in runners
             if r["state"] not in ("drained", "unreachable") and r["class"] != "egon"]
    top = max([_lane_cpu(r) for r in cands] or [0])
    return top, [r["name"] for r in cands if _lane_cpu(r) == top]


def fallback_note(runners, mode, chosen):
    """The NOTE of an integration/release pass on a lane narrower than a
    registered one `widest` skips (unreachable, drained), else None (tb-s10-b
    P7, m-RC-1 rest). An egon lane is no fallback cause: it never takes I/R."""
    cpu = _lane_cpu(chosen)
    skipped = [r for r in runners if r["state"] in ("drained", "unreachable")
               and r["class"] != "egon" and _lane_cpu(r) > cpu]
    if not skipped:
        return None
    skipped.sort(key=lambda r: -_lane_cpu(r))
    return "lane-fallback %s takes %s (%d threads), widest %s" % (
        mode, chosen["name"], cpu, ", ".join("%s (%d threads) is %s" % (
            r["name"], _lane_cpu(r), r["state"]) for r in skipped))


def lane_vcpu(db, name):
    """The thread count of lane `name` from its runner row (0 when unknown)."""
    row = db.execute("SELECT capacity FROM runners WHERE name = ?", (name,)).fetchone()
    return _lane_cpu(row) if row is not None else 0


# GH #1162 A3: the executor ranks and reserves by `calib_s`, else `timeout_min`
# (`executor._priorities`, `_estimate`). No plan set `calib_s`, so release A2
# ranked by timeouts: 788.2 s instead of 678.7 s (V6 sim). A measurement is the
# median of the last MEASURED_RUNS green runs of the station in the same repo
# and mode on a lane of the same class (its thread count, `lane_vcpu`), none
# older than MEASURED_MAX_AGE_S; without one the station's declared `prior_s`
# stands (plan.PRIOR_S, OR-TB.S4.38), else its timeout.
MEASURED_RUNS = 5
MEASURED_MAX_AGE_S = 14 * 86400


def _finite_secs(x):
    """`x` as seconds when it is a finite number > 0, else None (GH #1162 hA-m-1;
    the same rule is `executor._finite_secs` on the lane side)."""
    try:
        x = float(x)
    except (TypeError, ValueError):
        return None
    return x if math.isfinite(x) and x > 0 else None


def _object(text):
    """The JSON object in `text` ({} for none); ValueError for anything else."""
    value = json.loads(text or "{}")
    if not isinstance(value, dict):
        raise ValueError("no JSON object")
    return value


def measured_secs(db, repo, mode, lane, now=None):
    """{station: median seconds} of the green runs that measure `lane`'s class.

    The window is a SQL filter (store times are RFC 3339 UTC, `Z`, and sort as
    text); a row that is no JSON object or no finite duration > 0 measures
    nothing (GH #1162 hA-m-4, hA-m-1)."""
    seen = _green_secs(db, repo, mode, lane, MEASURED_RUNS, _now(now) - MEASURED_MAX_AGE_S)
    out = {}
    for name, xs in seen.items():
        xs = sorted(xs)
        mid = len(xs) // 2
        out[name] = xs[mid] if len(xs) % 2 else (xs[mid - 1] + xs[mid]) / 2.0
    return out


# TV-K5 (b), ruling K6 2026-10-11: a station cap is a hang marker of at least
# 3x the P95 of the station's last HANG_RUNS green runs in the same repo and mode
# on a lane of the same class. No age window: the last 30 are the last 30.
HANG_RUNS = 30


def measured_p95(db, repo, mode, lane):
    """{station: P95 seconds (nearest rank)} of its last HANG_RUNS green runs on
    `lane`'s class."""
    out = {}
    for name, xs in _green_secs(db, repo, mode, lane, HANG_RUNS, None).items():
        xs = sorted(xs)
        out[name] = xs[max(0, int(math.ceil(0.95 * len(xs))) - 1)]
    return out


def with_hang_caps(stations, p95):
    """(stations, raised): every `timeout_min` at least `plan.hang_cap_min` of the
    station's P95 -- raised, never lowered; `raised` = [(name, old, new)], each a
    finding (a cap below 3x P95)."""
    from . import plan as _plan  # lazy: plan imports nothing of the scheduler
    out, raised = [], []
    for st in stations:
        st = dict(st)
        cap = _plan.hang_cap_min(_finite_secs(p95.get(st.get("name"))))
        old = st.get("timeout_min")
        if cap is not None and (old is None or float(old) < cap):
            raised.append((st.get("name"), old, cap))
            st["timeout_min"] = cap
        out.append(st)
    return out, raised


def clamped_caps(stations, p95):
    """[(name, cap, wanted)]: a cap the [5, 120] min clamp of `plan.hang_cap_min`
    holds below 3x the station's P95 -- a finding with its own NOTE line, never
    silent (re-check tv-p0, B-Offen; `wanted` = 3x P95 in whole minutes)."""
    from . import plan as _plan  # lazy: plan imports nothing of the scheduler
    out = []
    for st in stations:
        secs = _finite_secs(p95.get(st.get("name")))
        cap = st.get("timeout_min")
        if secs is None or cap is None:
            continue
        wanted = int(math.ceil(secs * _plan.HANG_FACTOR / 60.0))
        if float(cap) < wanted:
            out.append((st.get("name"), cap, wanted))
    return out


def hang_cap_notes(raised, clamped):
    """The `gate: NOTE cap ...` lines of `with_hang_caps` and `clamped_caps`."""
    return (["gate: NOTE cap %s %s -> %s min (3x the P95 of its last %d green runs, TV-K5)"
             % (name, old, new, HANG_RUNS) for name, old, new in raised]
            + ["gate: NOTE cap %s clamped at %s min, below 3x the P95 of its last %d green "
               "runs (%s min) -- a finding (TV-K5)" % (name, cap, HANG_RUNS, wanted)
               for name, cap, wanted in clamped])


def _green_secs(db, repo, mode, lane, runs, since):
    """{station: [seconds]} of its last `runs` green runs in `repo` and `mode` on a
    lane of `lane`'s class, newest first; `since` (epoch seconds) or None."""
    cls = lane_vcpu(db, lane)
    if cls <= 0:
        return {}
    rows = db.execute(
        "SELECT t.runner, t.results, t.ended_at, i.doc AS tdoc, w.doc AS wdoc FROM task_runs t "
        "JOIN items i ON i.id = t.item JOIN items w ON w.id = i.parent "
        "WHERE t.verdict = 'GREEN' AND t.ended_at IS NOT NULL AND t.ended_at >= ? "
        "ORDER BY t.ended_at DESC", (_iso(since) if since is not None else "",))
    classes, seen = {}, {}
    for r in rows:
        try:
            if since is not None and store.parse_at(r["ended_at"]).timestamp() < since:
                continue
            wdoc, tdoc, results = _object(r["wdoc"]), _object(r["tdoc"]), _object(r["results"])
            station = tdoc.get("station")
            name = station.get("name") if isinstance(station, dict) else None
        except (TypeError, ValueError):
            continue
        secs = _finite_secs(results.get("secs"))
        if not name or secs is None or wdoc.get("repo") != repo or wdoc.get("mode") != mode:
            continue
        if r["runner"] not in classes:
            classes[r["runner"]] = lane_vcpu(db, r["runner"])
        if classes[r["runner"]] != cls:
            continue
        mine = seen.setdefault(name, [])
        if len(mine) < runs:
            mine.append(secs)
    return seen


def with_measured(stations, measured):
    """The stations with `calib_s` from `measured`; the others keep their timeout."""
    out = []
    for st in stations:
        st = dict(st)
        secs = _finite_secs(measured.get(st.get("name")))
        if secs is not None:
            st["calib_s"] = secs
        out.append(st)
    return out


def admission_tree(cargo, tree_name, holder):
    """The tree a pass runs in, relative to its lane's root: `tree` for a cargo pass,
    else `wt/<the tree's name>` (the holder's, `/` as `-`, when the gate names none).
    The one rule of the admission (`_admit`) and of `--plan-only`'s measure
    (`gate._lane_tree`) -- re-check bundle N delta kit-fp M1: a mirror there drifted
    unpinned (OR-T11-22)."""
    return "tree" if cargo else "wt/" + (tree_name or (holder or "").replace("/", "-") or "main")


def lane_root(root):
    """A lane's root as the admission writes it (the default for none, no trailing `/`)."""
    return (root or DEFAULT_ROOT).rstrip("/") or "/"


def tree_under(root, tree):
    """The absolute tree of `admission_tree` under the lane root `root`."""
    return "%s/%s" % (lane_root(root), tree)


def _admit(db, plan, *, repo, mode, strand, wave, base, lane_pref, now, tree_name, wait=False,
           wait_tree="", any_lane=False, queue=True, lane_procs=None):
    stations = list(plan.get("stations") or [])
    check_cpu(stations)
    names = [s["name"] for s in stations]
    demand = _demand(stations)
    cargo = demand["cargo"] >= 1
    holder_key = strand or wave
    tree = admission_tree(cargo, tree_name, holder_key)
    crit_s = int(round(_critical_min(stations) * 60)) + DEADLINE_OVERHEAD_S
    deadline = now + crit_s
    ttl_s = crit_s + LEASE_GRACE_S

    runners = _runners(db)
    by_name = {r["name"]: r for r in runners}
    if lane_pref and (lane_pref not in by_name or by_name[lane_pref]["state"] == "drained"):
        raise ScheduleError("no lane %s among the registered runners (%s)" % (
            lane_pref, " ".join(r["name"] for r in runners if r["state"] != "drained") or "none"))

    _close_orphans(db, now)
    wp = _find_wp(db, repo, mode, strand, wave, base)
    live = _live_runs(db, now)
    if wp is not None:
        mine = [r for r in live if r["item"] == wp["id"]]
        if mine:
            return Queued("live run %s of %s on lane %s -- one run per pass at a time"
                          % (mine[0]["run"], wp["key"], mine[0]["lane"]))

    # the lease queue (armed with lanes): a held lane wins, other strands' lanes are out
    st = _load(db)
    pool_lanes = (st or {}).get("lanes") or []
    if pool_lanes:
        holder_key = _batch_speaker(st, holder_key, lane_pref or "")
    ctx = _Ctx(db, now, branch=holder_key)
    ctx.lane_procs = lane_procs or {}
    held_lane, left = None, None
    if pool_lanes:
        _prune(st, ctx, holder_key, want=lane_pref or "")
        mine = _holder(st, holder_key)
        busy = _live_run_of(mine, ctx) if mine else None
        if busy:
            # Re-check fix5 M1: the pass would overwrite the lease's live run
            # (`mine["run"] = run` below) -- a batch `<wave>/S1+S2` on a member's
            # lease, or a second pass of the holder; the run's close frees it.
            _save(db, st, ctx)
            return Queued("%slease %s in use by run %s of %s on lane %s -- one run per lease at a time"
                          % ("member " if holder_key != (strand or wave) else "", holder_key,
                             busy["run"], busy["key"], busy["lane"]))
        if mine and mine.get("lane"):
            held_lane = mine["lane"]
            if lane_pref and lane_pref != held_lane:
                raise ScheduleError("%s holds lane %s, not %s -- release it first, then take %s"
                                    % (holder_key, held_lane, lane_pref, lane_pref))
        else:
            names_q = [w["strand"] for w in st["waiting"]]
            pos = names_q.index(holder_key) if holder_key in names_q else len(names_q)
            left = _lanes_left(st, st["waiting"][:pos])

    if lane_pref:
        order = [lane_pref]
    elif held_lane:
        order = [held_lane]
    else:
        order = _warm_first(db, runners)
        if pool_lanes:
            # GH #1190: an armed pool is the set of lanes a pass may book; a
            # host-file lane outside it was admitted without a lease (twice on
            # 2026-10-10), warm-first even preferred it. It takes a pass only
            # by an explicit `--host` (`lane_pref` above).
            order = [n for n in order if n in pool_lanes]
    # the widest lane is the widest a pass may book: in the pool, if armed (GH #1190)
    cands = [r for r in runners if r["name"] in pool_lanes] if pool_lanes else runners
    narrow_out = []
    if mode in WIDEST_MODES and not lane_pref and not held_lane and not any_lane:
        top, wide = widest(cands)
        # m-RC-1: a lane `widest` skips (unreachable, drained, egon) is no
        # narrower lane `--any-lane` would take; it stays in `order`, where the
        # loop names it for what it is ("lane build04 is unreachable").
        narrow_out = [n for n in order if n not in wide and n in by_name
                      and by_name[n]["state"] not in ("drained", "unreachable")
                      and by_name[n]["class"] != "egon"]
        order = [n for n in order if n not in narrow_out]

    # `open_`: a cause an event or a round resolves (a fit, a lease) -- without
    # one, the refusal is `final` for a caller that waits (Review Z-A2 I1).
    reasons, off, booting, chosen, lease_blocked, open_ = [], [], [], None, False, False
    for name in order:
        r = by_name.get(name)
        if r is None:
            reasons.append("lane %s is not registered" % name)
            continue
        if r["class"] == "egon" and mode not in EGON_MODES:
            reasons.append("egon lane %s runs only test|measure, not %s" % (name, mode))
            continue
        if r["class"] == "egon" and repo != _core_repo():
            # Koordinator (welle-tb S3 F1, re-check h1 M-1): a satellite never runs on
            # the owner's machine -- its lab books more colonies than an egon lane
            # offers beside the live colony; the admission books it a lane.
            reasons.append("egon lane %s never takes a satellite pass (%s)" % (name, repo))
            continue
        if left is not None and name in pool_lanes and name not in left:
            reasons.append("lane %s is leased to another strand or a waiter ahead" % name)
            lease_blocked = True
            continue
        if r["state"] in ("drained", "unreachable"):
            reasons.append("lane %s is %s" % (name, r["state"]))
            continue
        if r["state"] == "off":
            off.append(r)
            continue
        if r["state"] == "booting":
            booting.append(r)
            continue
        cap = json.loads(r["capacity"] or "{}")
        lane_demand = _demand_on(stations, cap.get("cpu"), cap.get("colony"))
        need = dict(lane_demand, cpu=min(lane_demand["cpu"], int(cap.get("cpu") or 0)))
        # a demand over the lane's whole capacity fits no matter what ends
        # there: no event resolves it, so it is no `open_` cause (GH #1155 (3))
        refused = {"browser_" + e: "%s: %s" % (e, why) for e, why in
                   browser_refusals(json.loads(r["fingerprint"] or "{}")).items()}
        never = ["%s %d > %d%s" % (k, need[k], int(cap.get(k) or 0),
                                   " (%s)" % refused[k] if k in refused else "")
                 for k in CAPACITY_KEYS if need[k] > int(cap.get(k) or 0)]
        if never:
            reasons.append("lane %s can never take the pass: needs %s" % (name, ", ".join(never)))
            continue
        why = _fit(cap, [x for x in live if x["lane"] == name], need, tree)
        if why:
            reasons.append("lane %s: %s" % (name, why))
            open_ = True
            continue
        chosen, demand = r, lane_demand
        break

    if chosen is None and narrow_out:
        # A narrower lane is no way out (GH #1162): the pass waits for the widest
        # one -- open, so `--wait-admission` keeps waiting while it is busy.
        reasons.append("%s waits for the widest lane (%d threads: %s), not %s -- "
                       "--any-lane takes a narrower one"
                       % (mode, widest(cands)[0], " ".join(order) or "none",
                          " ".join(narrow_out)))

    if chosen is None:
        # Fund 21:11 (R-TV-W21 a): a gate with `--host` queues WITH its lane
        # (`want`) -- without it `_lanes_left` handed the waiter the first free
        # lane, which it never takes, and every waiter behind it got none. It
        # queues for its lane even while another lane is free (FIFO per lane).
        if pool_lanes and held_lane is None and lease_blocked and (not left or lane_pref):
            if not holder_key or not queue:
                _save(db, st, ctx)
                return Queued("%s -- no queue place: %s" % (
                    "; ".join(reasons) or "every lane is leased",
                    "the pass names no holder (--strand)" if not holder_key
                    else "a merge gate does not wait for an offer"))
            msg = _enqueue(st, ctx, holder_key, lane_pref or "", wait_tree or "",
                           wait="gate --wait-admission" if wait else False)
            _save(db, st, ctx)
            return Queued(msg)
        if pool_lanes:
            _save(db, st, ctx)
        # nothing ends `booting` in a wait round (`_sync` turns only `off`
        # idle): past its boot time a lane is a final reason, and the wait
        # keeps only for one still in its window (GH #1155 (3))
        in_window = []
        for b in booting:
            boot_s = int(b["boot_s"] or BOOT_S)
            since = _epoch(b["updated_at"]) if b["updated_at"] else None
            if since is not None and now >= since + boot_s:
                reasons.append("lane %s has been booting for more than %ds" % (b["name"], boot_s))
            else:
                in_window.append((b, boot_s))
        if in_window:
            b, boot_s = in_window[0]
            return Queued("lane %s is booting (up to %ds) -- %s" % (
                b["name"], boot_s, "; ".join(reasons) or "no idle lane"))
        if off:
            lane = off[0]
            why = "no idle lane takes the pass: " + ("; ".join(reasons) or "every lane is off")
            asked = db.execute(
                "SELECT 1 FROM journal WHERE event = 'lane_power_requested' AND "
                "json_extract(data, '$.lane') = ? AND at >= ? LIMIT 1",
                (lane["name"], lane["updated_at"] or "")).fetchone()
            # `_sync` alone turns `off` to `idle`, once per gate: no round resolves it.
            final = not (open_ or lease_blocked)
            if asked:
                return Queued("power for lane %s is requested -- %s" % (lane["name"], why), final=final)
            _journal(db, "lane_power_requested", "finding",
                     {"lane": lane["name"], "reason": why,
                      "eta_s": int(lane["boot_s"] or BOOT_S)}, at=_iso(now))
            return Queued("lane %s is off, power requested -- %s" % (lane["name"], why),
                          power_requested=lane["name"], final=final)
        return Queued("; ".join(reasons) or "no lane is registered", final=not (open_ or lease_blocked))

    run = "run-" + secrets.token_hex(6)
    lane_name = chosen["name"]
    lease_taken = False
    if pool_lanes:
        if lane_name in pool_lanes:
            ttl_min = int(math.ceil(ttl_s / 60.0))
            mine = _holder(st, holder_key)
            if mine:
                mine["seen"] = now
                mine["ttl_min"] = max(mine.get("ttl_min") or st["ttl_min"], ttl_min)
                mine["run"] = run
            else:
                _grant(st, ctx, holder_key, lane_name, "", ttl_min=ttl_min, run=run)
                lease_taken = True
        _save(db, st, ctx)

    adm = _dispatch(db, plan, chosen, run, stations, names, demand, cargo, tree, now, deadline,
                    ttl_s, live, wp, lease_taken, repo=repo, mode=mode, strand=strand,
                    wave=wave, base=base)
    if mode in WIDEST_MODES and not lane_pref and not held_lane and not any_lane:
        note = fallback_note(runners, mode, chosen)
        if note:
            adm.notes.append(note)
    return adm


def _dispatch(db, plan, runner, run, stations, names, demand, cargo, tree, now, deadline, ttl_s,
              live, wp, lease_taken, *, repo, mode, strand, wave, base):
    lane = runner["name"]
    root = lane_root(runner["root"])
    cap = json.loads(runner["capacity"] or "{}")
    here = [x for x in live if x["lane"] == lane]
    held = {k: sum(int(x["demand"].get(k) or 0) for x in here) for k in CAPACITY_KEYS}
    capacity = {k: int(cap.get(k) or 0) - held[k] for k in CAPACITY_KEYS}
    need = dict(demand, cpu=min(demand["cpu"], int(cap.get("cpu") or 0)))
    expected_g = WS_EXPECTED_G
    if cargo:
        expected_g = _target_g(db, lane) or DEFAULT_EXPECTED_G
    reserved_live_g = sum(int(x["expected_g"] or 0) for x in here)
    budget = dict(DEFAULT_BUDGET, reserved_live_g=reserved_live_g)
    budget["need_free_g"] = budget["reserve_g"] + reserved_live_g + expected_g
    tree_path = tree_under(root, tree)
    ws = "%s/ws/%s" % (root, run)
    at = _iso(now)
    deadline_at = _iso(deadline)

    wp_doc = {"repo": repo, "mode": mode, "stage": mode, "strand": strand, "wave": wave,
              "rev": plan.get("rev"), "base": base, "tree": tree, "dirty": plan.get("dirty", False),
              "lane": lane, "archive": plan.get("archive"), "plan": names,
              "owner_decisions": list(plan.get("owner_decisions") or []),
              "plan_reason": plan.get("plan_reason") or {},
              "run": run, "demand": need, "expected_g": expected_g, "deadline_at": deadline_at,
              "ws": ws,
              # the local driver of the pass (`lanes release-run` asks whether it lives)
              "driver_pid": os.getpid(), "driver_host": socket.gethostname()}
    title = "%s %s %s" % (repo, mode, strand or wave)
    if wp is None:
        wp_doc["round"] = 1
        wp_id, key = _item_new(db, "workpackage", title, None, wp_doc, "admitted", at)
        _item_walk(db, wp_id, "active", "admitted", at)        # planned -> active
        old_tasks = {}
    else:
        key, wp_id = wp["key"], wp["id"]
        prev = json.loads(wp["doc"] or "{}")
        wp_doc["round"] = int(prev.get("round") or 1) + 1
        _item_walk(db, wp_id, "active", "round", at, doc=wp_doc)   # finished|stopped -> active
        old_tasks = {}
        for row in db.execute("SELECT id, doc FROM items WHERE parent = ? AND type = 'task'",
                              (wp_id,)):
            name = json.loads(row["doc"] or "{}").get("station", {}).get("name")
            if name:
                old_tasks[name] = row["id"]
    rnd = wp_doc["round"]

    tasks, task_runs = {}, {}
    for st in stations:
        name = st["name"]
        argv = list((st.get("cmds") or [{}])[0].get("argv") or [])
        doc = {"exec": {"runner": lane, "root": tree_path, "test_argv": argv, "workspace": ws,
                        "timeout_min": st.get("timeout_min"), "retries": 0, "evidence": ["log"]},
               "station": st}
        if name in old_tasks:
            tid = old_tasks[name]
            # done -> open -> in_progress (reopen), waiting -> in_progress
            _item_walk(db, tid, "in_progress", "round", at, doc=doc)
        else:
            tid, _ = _item_new(db, "task", "%s (%s)" % (name, strand or wave), wp_id, doc,
                               "dispatched", at)
            _item_walk(db, tid, "in_progress", "dispatched", at)  # open -> in_progress
        tasks[name] = tid
    for st in stations:
        for dep in st.get("after") or []:
            if dep not in tasks:
                continue
            data = {"dep": "fs", "lag_min": 0}
            if st.get("when"):
                data["when"] = dict(st["when"])
            db.execute("INSERT OR REPLACE INTO edges (src, dst, kind, at, actor, data) "
                       "VALUES (?, ?, 'depends_on', ?, 'pipeline', ?)",
                       (tasks[st["name"]], tasks[dep], at, json.dumps(data, sort_keys=True)))
    for st in stations:
        name = st["name"]
        tid = tasks[name]
        tkey = db.execute("SELECT key FROM items WHERE id = ?", (tid,)).fetchone()["key"]
        attempt = db.execute("SELECT count(*) AS n FROM task_runs WHERE item = ?",
                             (tid,)).fetchone()["n"] + 1
        tr = "tr-" + secrets.token_hex(6)
        db.execute("INSERT INTO task_runs (id, item, key, runner, attempt, ws, state, deadline_at, "
                   "lost, round) VALUES (?, ?, ?, ?, ?, ?, 'dispatched', ?, 0, ?)",
                   (tr, tid, tkey, lane, attempt, ws, deadline_at, rnd))
        task_runs[name] = tr
        _journal(db, "run_dispatched", "note",
                 {"run": run, "runner": lane, "tree": tree_path, "attempt": attempt,
                  "round": rnd, "deadline_at": deadline_at, "ws": ws, "task_run": tr,
                  "station": name}, run=run, item=tid, at=at)
    db.execute("UPDATE runners SET state = 'busy', updated_at = ? WHERE name = ?", (at, lane))

    fp = json.loads(runner["fingerprint"] or "{}")
    egon = runner["class"] == "egon"
    try:
        from . import lane as lane_mod
        env = lane_mod.env_for(_lane_of(runner), mode, nproc=fp.get("vcpu"))
    except (ImportError, AttributeError):
        env = {"MECLAW_LANE_NAME": lane}
        if egon:
            env.update({"CARGO_BUILD_JOBS": str(int(cap.get("cpu") or 1)),
                        "NEXTEST_TEST_THREADS": "4"})
    dispatch = {"run": run, "repo": repo, "mode": mode, "rev": plan.get("rev"), "base": base,
                "strand": strand or wave, "tree": tree_path, "target": "%s/target" % root, "ws": ws,
                "stations": stations, "capacity": capacity, "cache_known": [],
                "live_runs": [x["run"] for x in here], "expected_g": expected_g,
                "fingerprint_expected": fp, "budget": budget, "quarantine": [], "decide": [],
                "deadline_at": deadline_at, "+env": env, "+nice": egon,
                "+cpu_overbook": CPU_OVERBOOK, "+export_root": "%s/export" % root,
                "+fail_fast": False, "+no_precheck_stop": False, "+only": None,
                "+dispatched_at": at}  # the round's start in the journal (#1152)
    return Admission(lane=lane, run=run, deadline_at=deadline_at, ttl_s=ttl_s, capacity=capacity,
                     workpackage=key, round=rnd, tasks=tasks, task_runs=task_runs, tree=tree,
                     cargo=cargo, expected_g=expected_g, dispatch=dispatch,
                     lease_taken=lease_taken, item=wp_id, root=root, ws=ws, repo=repo, mode=mode,
                     strand=strand, wave=wave, base=base)


# --- ingest and close ----------------------------------------------------------------------

def _tool_name(station):
    name = (station or {}).get("name", "")
    if (station or {}).get("nextest"):
        return "nextest-binary"
    if name.startswith("scenarios"):
        return "scenario"
    if name.startswith("browser"):
        return "browser"
    if name == "unittests":
        return "unittest"
    return "cmd"


def _station_state(verdict, reason, legacy=False):
    # `timeout` or, since #1150, `timeout -- log <path>` (review Z-I1 I3); since
    # TV-K5 a cap is a hang marker: `HANG: station <name> exceeded <n> min ...`.
    if reason == "timeout" or (reason or "").startswith(("timeout ", "HANG: ")):
        return "timed_out", "timeout"
    if verdict in DONE_VERDICTS or (verdict == "FLAKY" and legacy):
        return "done", None
    if verdict == "FLAKY":     # a finding, not green (OR-TV-2, VG TV-F1)
        return "failed", "flaky"
    if verdict == "RED":
        return "failed", "red"
    if verdict == "ASK":
        return "failed", "ask"
    if verdict == "ABORTED":
        return "failed", reason if reason and REASON_CODE.match(reason) else "aborted_process"
    return "failed", "unknown"


def ingest(db, admission, line):
    """Fold one executor event (a JSONL line or a dict) into the store; returns the event.

    Journal always; `station_started`/`station_ended` move the task run and its task,
    `test_result`/`cache_hit` become `task_run_results` rows (`<station>/<part>`),
    `lane_fingerprint` lands on the runner."""
    ev = json.loads(line) if isinstance(line, (str, bytes)) else dict(line)
    data = dict(ev.get("data") or {})
    event = ev.get("event")
    at = ev.get("at") or store.now()
    station = data.get("station")
    item = admission.tasks.get(station) if station else None
    tr = admission.task_runs.get(station) if station else None
    stations = {s["name"]: s for s in admission.dispatch.get("stations") or []}
    with _tx(db):
        _journal(db, event, ev.get("kind") or "note", data, run=admission.run, item=item,
                 at=at, text=ev.get("text"))
        if event == "station_started" and tr:
            db.execute("UPDATE task_runs SET started_at = ? WHERE id = ?", (at, tr))
        elif event == "station_ended" and tr:
            verdict = data.get("verdict")
            state, reason = _station_state(verdict, data.get("reason"), data.get("legacy") is True)
            results = {k: data[k] for k in ("secs", "exit", "build_s", "run_s", "isolated",
                                            "reason") if k in data}
            db.execute("UPDATE task_runs SET state = ?, reason = ?, verdict = ?, ended_at = ?, "
                       "results = ?, build_s = coalesce(?, build_s), run_s = coalesce(?, run_s), "
                       "cache_from = coalesce(?, cache_from) WHERE id = ?",
                       (state, reason, verdict, at, json.dumps(results, sort_keys=True),
                        data.get("build_s"), data.get("run_s"), data.get("cache_from"), tr))
            if item:
                _item_walk(db, item, store.TASK_AFTER_RUN[state],
                           reason or (verdict or "").lower(), at)
        elif event in ("test_result", "cache_hit") and tr:
            part = data.get("part") or ""
            entry = {k: v for k, v in data.items() if k not in ("run", "station")}
            entry["part"] = part
            if event == "cache_hit":
                entry = {"part": part, "verdict": "CACHED", "cache_key": data.get("key"),
                         "cache_from": data.get("from_run")}
                db.execute("UPDATE task_runs SET cache_from = ? WHERE id = ?",
                           (data.get("from_run"), tr))
            db.execute("INSERT OR IGNORE INTO task_run_results (run, at, call, tool_name, entry) "
                       "VALUES (?, ?, ?, ?, ?)",
                       (tr, at, "%s/%s" % (station, part), _tool_name(stations.get(station)),
                        json.dumps(entry, sort_keys=True)))
        elif event == "lane_fingerprint" and not data.get("unanswered"):
            # re-check tv-bn M7r A-M1: a lane that did not answer keeps its register
            # (nulls there keyed the next run's cache over no tool versions)
            fp = {k: v for k, v in data.items() if k != "run"}
            db.execute("UPDATE runners SET fingerprint = ? WHERE name = ?",
                       (json.dumps(fp, sort_keys=True), admission.lane))
    return ev


def close(db, admission, pass_ended, lost=False, now=None, reason=None):
    """End a pass: the workpackage `finished/<verdict>` (or `stopped/aborted`, or
    `stopped/lost` -- then lease, run and runner stay as they are, the lane may
    still build), calibration rows for the stations that took time, the lease the
    admission took goes with the run, the runner is idle when nothing else runs.
    `reason` names why an ABORTED pass's open task runs failed (`orphan` from
    `lanes release-run`); without it they fail `aborted_process`."""
    _ensure(db)
    now = _now(now)
    at = _iso(now)
    open_reason = reason or "aborted_process"
    with _tx(db):
        rows = {name: db.execute("SELECT * FROM task_runs WHERE id = ?", (tr,)).fetchone()
                for name, tr in admission.task_runs.items()}
        wp = db.execute("SELECT doc FROM items WHERE id = ?", (admission.item,)).fetchone()
        # Delta A M-neu-2: a kern that outlived its round (closed lost, the
        # workpackage admitted again) ends only its own task runs -- the
        # workpackage, its tree and the runner are the next round's.
        current = not wp or json.loads(wp["doc"] or "{}").get("run") in (None, admission.run)
        if lost:
            if current:
                _item_walk(db, admission.item, "stopped", "lost", at)
            for name, row in rows.items():
                if row and row["state"] == "dispatched":
                    db.execute("UPDATE task_runs SET state = 'failed', reason = 'lost', lost = 1, "
                               "ended_at = ? WHERE id = ?", (at, row["id"]))
                    _item_walk(db, row["item"], "waiting", "lost", at)
            return
        pe = pass_ended or {}
        data = pe.get("data", pe) if isinstance(pe, dict) else {}
        verdict = data.get("verdict") or "ABORTED"
        if verdict == "ABORTED":
            state, reason = "stopped", "aborted"
        else:
            state, reason = "finished", verdict.lower()
        if current:
            _item_walk(db, admission.item, state, reason, at)
        for name, row in rows.items():
            if row is None:
                continue
            if row["state"] == "dispatched":
                why = open_reason if verdict == "ABORTED" else "missing"
                db.execute("UPDATE task_runs SET state = 'failed', reason = ?, ended_at = ? "
                           "WHERE id = ?", (why, at, row["id"]))
                _item_walk(db, row["item"], "waiting", why, at)
                continue
            secs = json.loads(row["results"] or "{}").get("secs") or 0
            if row["verdict"] in NO_CALIBRATION or not secs or secs <= 0:
                continue
            db.execute("INSERT OR REPLACE INTO calibration (item, type, executing, estimate_min, "
                       "actual_min, actual_s, closed_at, key, estimate_s) "
                       "VALUES (?, 'task', ?, NULL, ?, ?, ?, ?, NULL)",
                       (row["item"], admission.lane, round(secs / 60.0, 4), float(secs), at,
                        "%s@%s" % (name, admission.repo)))
        st = _load(db)
        if st is not None:
            ctx = _Ctx(db, now, branch=None)
            holder = admission.strand or admission.wave
            mine = _holder(st, holder)
            if not admission.lease_taken and not (mine and mine.get("run") == admission.run):
                # Re-check fix4 H1: a batch `<wave>/S1+S2` ran on a member's lease
                # (`_batch_speaker`); its `run` points here, the lease stays the member's.
                mine = next((h for h in st["holders"] if h.get("run") == admission.run), mine)
            if mine and mine.get("run") == admission.run:
                if admission.lease_taken:
                    ctx.branch = holder
                    _release_entry(st, ctx, holder, mine)
                else:
                    # K9 (welle-validierung-2): the end of a run is the lease's
                    # last sign of life -- `seen` dated from the admission, and
                    # 3 s after a 16-min gate the idle reclaim took the lane
                    # back as "idle 16 min". The idle TTL counts from here.
                    _keep_last_station(db, mine, admission.run)
                    mine["run"] = None
                    mine["seen"] = now
                _save(db, st, ctx)
        busy = [x for x in _live_runs(db, now) if x["lane"] == admission.lane]
        if not busy:
            db.execute("UPDATE runners SET state = 'idle', updated_at = ? WHERE name = ? "
                       "AND state = 'busy'", (at, admission.lane))
    _wake(_db_path(db))     # the run's lease may be free now (after the commit)


# --- an orphaned run: `lanes release-run <lane> --run <run>` (Z-I, #1126) -------------------
# 09.10.: a local measuring chain ended without `close()`; its run kept two task
# runs `dispatched` and its workpackage `active` until deadline + grace, and the
# admission kept build04 busy for three hours with nothing running there. The
# verb closes such a run as `stopped/aborted`, its open task runs `failed/orphan`
# -- fail-closed: a live local driver, the run's scope on the lane, a process in
# its workspace, or a probe without an answer refuses and changes nothing.

ORPHAN_REASON = "orphan"
UNIT_GONE_STATES = ("inactive", "failed", "unknown")


def _run_pids(db, run, doc):
    """Every pid the store holds for a run's local driver: the workpackage doc
    (`driver_pid`, `pids`), the lease entries of the run or its holder, and
    `pid`/`pids` fields of the run's journal rows."""
    pids = set()

    def add(value):
        for v in value if isinstance(value, list) else [value]:
            if isinstance(v, int) and v > 1:
                pids.add(v)
            elif isinstance(v, str) and v.isdigit() and int(v) > 1:
                pids.add(int(v))
    add(doc.get("driver_pid"))
    add(doc.get("pid"))
    add(doc.get("pids") or [])
    holder = doc.get("strand") or doc.get("wave") or ""
    for e in db.execute("SELECT pids FROM lease_entries WHERE run = ? OR (holder = ? AND ? != '')",
                        (run, holder, holder)):
        try:
            add(json.loads(e["pids"] or "[]"))
        except ValueError:
            continue
    for row in db.execute("SELECT data FROM journal WHERE run = ?", (run,)):
        try:
            data = json.loads(row["data"] or "{}")
        except ValueError:
            continue
        if isinstance(data, dict):
            add(data.get("pid"))
            add(data.get("pids") or [])
    return sorted(pids)


def release_run(db, lane, run, *, by, probe, now=None):
    """Close the orphaned run `run` on `lane` (a `lane.Lane`); `probe` is the
    lane module (`unit_state`, `ws_procs`). Returns the closed task-run ids;
    raises `ScheduleError` (exit 2) with the reason when it refuses."""
    _ensure(db)
    now = _now(now)
    rows = db.execute("SELECT * FROM items WHERE type = 'workpackage' AND "
                      "json_extract(doc, '$.run') = ? ORDER BY id DESC", (run,)).fetchall()
    if not rows:
        raise ScheduleError("lanes release: no workpackage carries the run %s" % run)
    wp = rows[0]
    doc = json.loads(wp["doc"] or "{}")
    if doc.get("lane") != lane.name:
        raise ScheduleError("lanes release: %s ran on %s, not on %s"
                            % (run, doc.get("lane") or "no lane", lane.name))
    ws = doc.get("ws") or ""
    open_trs = db.execute(
        "SELECT task_runs.* FROM task_runs JOIN items ON items.id = task_runs.item "
        "WHERE items.parent = ? AND task_runs.ws = ? AND task_runs.state = 'dispatched'",
        (wp["id"], ws)).fetchall() if ws else []
    if wp["state"] != "active" and not open_trs:
        raise ScheduleError("lanes release: %s has nothing open (workpackage %s %s/%s)"
                            % (run, wp["key"], wp["state"], wp["reason"] or "-"))
    # (a) no local driver of the run lives
    host = doc.get("driver_host")
    if host and host != socket.gethostname():
        raise ScheduleError("lanes release: the driver of %s ran on %s -- its pid cannot be "
                            "probed from %s" % (run, host, socket.gethostname()))
    alive = [p for p in _run_pids(db, run, doc) if _alive(p)]
    if alive:
        raise ScheduleError("lanes release: driver pid %s of %s is alive -- not an orphan"
                            % (", ".join(str(p) for p in alive), run))
    # (b) the lane shows neither the run's scope nor a process in its workspace
    unit = "pipe-%s.scope" % run
    state = probe.unit_state(lane, run)
    if state not in UNIT_GONE_STATES:
        raise ScheduleError("lanes release: %s on %s is %s -- not an orphan"
                            % (unit, lane.name, state or "unknown (the probe got no answer)"))
    if not ws:
        raise ScheduleError("lanes release: %s names no workspace to probe" % run)
    procs = probe.ws_procs(lane, ws)
    if procs is None:
        raise ScheduleError("lanes release: the workspace probe of %s on %s got no answer"
                            % (ws, lane.name))
    if procs:
        raise ScheduleError("lanes release: pid %s on %s live in %s -- not an orphan"
                            % (", ".join(str(p) for p in procs), lane.name, ws))
    # the close: the store's own `close()` on the run's round
    tasks, task_runs = {}, {}
    for row in db.execute(
            "SELECT task_runs.id AS tr, items.id AS item, items.doc AS doc FROM task_runs "
            "JOIN items ON items.id = task_runs.item WHERE items.parent = ? AND task_runs.ws = ?",
            (wp["id"], ws)):
        name = json.loads(row["doc"] or "{}").get("station", {}).get("name") or row["tr"]
        tasks[name], task_runs[name] = row["item"], row["tr"]
    holder = doc.get("strand") or doc.get("wave") or ""
    lease = db.execute("SELECT run FROM leases WHERE holder = ?", (holder,)).fetchone() \
        if holder else None
    adm = Admission(lane=lane.name, run=run, deadline_at=doc.get("deadline_at") or "", ttl_s=0,
                    capacity={}, workpackage=wp["key"], round=int(doc.get("round") or 1),
                    tasks=tasks, task_runs=task_runs, tree=doc.get("tree") or "",
                    cargo=False, expected_g=int(doc.get("expected_g") or 0), dispatch={},
                    lease_taken=bool(lease and lease["run"] == run), item=wp["id"], ws=ws,
                    repo=doc.get("repo") or "", mode=doc.get("mode") or "",
                    strand=doc.get("strand") or "", wave=doc.get("wave") or "",
                    base=doc.get("base") or "")
    closed = sorted(r["id"] for r in open_trs)
    close(db, adm, {"verdict": "ABORTED"}, now=now, reason=ORPHAN_REASON)
    events.emit(db, "run_closed", "note",
                {"run": run, "runner": lane.name, "reason": ORPHAN_REASON, "by": by,
                 "workpackage": wp["key"], "task_runs": closed,
                 "unit_state": state}, run=run, item=wp["id"], at=_iso(now))
    return closed


def _release_run_cli(argv, env, cwd):
    """`lanes release-run <lane> --run <run> [--by <who>]`: (code, stdout, stderr)."""
    lane_name, run, by, i = "", "", "", 0
    while i < len(argv):
        a = argv[i]
        if a in ("--run", "--by"):
            if i + 1 >= len(argv):
                return 2, "", "strand: lanes release: %s needs a value\n" % a
            if a == "--run":
                run = argv[i + 1]
            else:
                by = argv[i + 1]
            i += 2
        elif not a.startswith("-") and not lane_name:
            lane_name = a
            i += 1
        else:
            return 2, "", "strand: lanes release: unknown argument: %s\n" % a
    if not lane_name or not run:
        return 2, "", "strand: lanes release: usage: lanes release <lane> --run <run>\n"
    if not re.match(r"^run-[0-9a-f]{6,32}$", run):
        return 2, "", "strand: lanes release: not a run id: %s\n" % run
    db_path, lanes_file = _paths(env)
    if not os.path.exists(db_path):
        return 2, "", "strand: lanes release: no pipeline store at %s\n" % db_path
    if _lane_override is not None:
        mod = _lane_override
    else:
        from . import lane as mod
    lane = next((ln for ln in mod.load_lanes(lanes_file) if ln.name == lane_name), None)
    if lane is None:
        return 2, "", "strand: lanes release: no lane %s in %s\n" % (lane_name, lanes_file)
    if not by:
        branch = _git(cwd, "rev-parse", "--abbrev-ref", "HEAD")
        by = "%s@%s%s" % (os.environ.get("USER") or str(os.getuid()), socket.gethostname(),
                          (":" + branch) if branch else "")
    now = int(env.get("MECLAW_STRAND_NOW") or time.time())
    db = store.open(db_path)
    try:
        closed = release_run(db, lane, run, by=by, probe=mod, now=now)
    except ScheduleError as exc:
        return exc.code, "", "strand: %s\n" % exc
    finally:
        db.close()
    return 0, "lanes release: %s on %s closed (stopped/aborted, reason orphan, by %s); " \
        "task runs %s\n" % (run, lane_name, by, " ".join(closed) or "-"), ""


# --- leases: the token of strand.sh on the store -------------------------------------------

class _Ctx:
    """One lease verb: the clock, the caller's branch, its output, the journal log."""

    def __init__(self, db, now, branch=None, path=""):
        self.db, self.now, self.branch, self.path = db, int(now), branch, path
        self.out, self.err = [], []
        self.lane_procs = {}    # lane -> the lane tree probe's answer (`_probe_stale_lanes`)

    def say(self, msg):
        self.err.append("strand: " + msg)

    def fail(self, msg):
        self.say(msg)
        raise _Exit(2)

    def print(self, msg):
        self.out.append(msg)

    def mins(self, t):
        return max(0, (self.now - int(t)) // 60)

    def log(self, verb, holder, detail="", resource=None, run=None, ttl_s=None):
        if verb == "take":
            event = "lease_taken"
        elif verb.split(" ")[0] in ("release", "release-by", "stale"):
            event = "lease_released"
        elif verb == "offer":
            event = "lease_offered"
        else:
            event = "lease_noted"
        _journal(self.db, event, "note",
                 {"resource": resource, "holder": holder, "run": run, "ttl_s": ttl_s,
                  "verb": verb, "branch": self.branch or "-", "detail": detail},
                 run=run, at=_iso(self.now))

    def result(self, code, lane=None):
        return LeaseResult(code, _join(self.out), _join(self.err), lane)


def _armed(db):
    _ensure(db)
    return db.execute("SELECT 1 FROM lease_pools WHERE pool = ? AND armed IS NOT NULL",
                      (POOL,)).fetchone() is not None


def _load(db):
    """The pool state in the form of the old token file, None when unarmed."""
    _ensure(db)
    row = db.execute("SELECT * FROM lease_pools WHERE pool = ? AND armed IS NOT NULL",
                     (POOL,)).fetchone()
    if row is None:
        return None
    st = {"max": int(row["max"]), "ttl_min": int(row["ttl_min"]), "armed": row["armed"],
          "holders": [], "waiting": []}
    lanes = json.loads(row["lanes"]) if row["lanes"] else []
    if lanes:
        st["lanes"] = lanes
    holds = {(r["holder"], r["since"]): r["hold_min"] for r in db.execute(
        "SELECT holder, since, hold_min FROM lease_holds WHERE pool = ?", (POOL,))}
    for e in db.execute("SELECT * FROM lease_entries WHERE pool = ? ORDER BY seq", (POOL,)):
        if e["resource"]:
            h = {"strand": e["holder"], "since": _epoch(e["since"]), "seen": _epoch(e["seen"]),
                 "pids": [int(p) for p in json.loads(e["pids"] or "[]")], "tree": e["tree"] or "",
                 "resource": e["resource"], "ttl_min": e["ttl_min"], "run": e["run"],
                 "hold_min": e["hold_min"] or holds.get((e["holder"], e["since"]))}
            if e["resource"].startswith("lane:"):
                h["lane"] = e["resource"][len("lane:"):]
            st["holders"].append(h)
        else:
            w = {"strand": e["holder"], "since": _epoch(e["since"]), "tree": e["tree"] or ""}
            if e["want"]:
                w["lane"] = e["want"]
            if e["offered"] is not None:
                w["offered"] = {"at": _epoch(e["offered_at"])}
                if e["offered"]:
                    w["offered"]["lane"] = e["offered"]
            st["waiting"].append(w)
    return st


def _hold_ttl(h, st):
    return int(h.get("ttl_min") or st["ttl_min"])


def _save(db, st, ctx):
    """Write the pool back -- every write states which waiter a free token is offered to."""
    _mark_offers(st, ctx)
    db.execute("DELETE FROM lease_entries WHERE pool = ?", (POOL,))
    for h in st["holders"]:
        db.execute("INSERT INTO lease_entries (pool, holder, resource, want, since, seen, pids, "
                   "tree, offered, offered_at, ttl_min, run, hold_min) "
                   "VALUES (?,?,?,NULL,?,?,?,?,NULL,NULL,?,?,?)",
                   (POOL, h["strand"], h["resource"], _iso(h["since"]), _iso(h["seen"]),
                    json.dumps(h["pids"]), h.get("tree") or "", h.get("ttl_min"), h.get("run"),
                    h.get("hold_min")))
        if h.get("hold_min"):
            db.execute("INSERT OR REPLACE INTO lease_holds (pool, holder, since, hold_min) "
                       "VALUES (?,?,?,?)", (POOL, h["strand"], _iso(h["since"]), h["hold_min"]))
    for w in st["waiting"]:
        o = w.get("offered")
        db.execute("INSERT INTO lease_entries (pool, holder, resource, want, since, seen, pids, "
                   "tree, offered, offered_at, ttl_min, run) VALUES (?,?,NULL,?,?,NULL,'[]',?,?,?,NULL,NULL)",
                   (POOL, w["strand"], w.get("lane"), _iso(w["since"]), w.get("tree") or "",
                    (o.get("lane") or "") if o else None, _iso(o["at"]) if o else None))
    held = {h["resource"]: h for h in st["holders"]}
    for row in db.execute("SELECT resource FROM leases WHERE resource LIKE 'lane:%' "
                          "OR resource LIKE 'token:%'").fetchall():
        if row["resource"] not in held:
            db.execute("DELETE FROM leases WHERE resource = ?", (row["resource"],))
    for res, h in held.items():
        ttl_s = (h["seen"] - h["since"]) + _hold_ttl(h, st) * 60
        db.execute("INSERT INTO leases (resource, holder, run, taken_at, ttl_s, offered_to) "
                   "VALUES (?, ?, ?, ?, ?, NULL) ON CONFLICT(resource) DO UPDATE SET "
                   "holder = excluded.holder, run = excluded.run, taken_at = excluded.taken_at, "
                   "ttl_s = excluded.ttl_s",
                   (res, h["strand"], h.get("run"), _iso(h["since"]), ttl_s))


def _alive(pid):
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    except OverflowError:
        return False
    return True


def _stale(h, st, ctx, want=None, idle=True, key=None):
    """Why `h` is stale, "" while it is not. `want`: the caller's own wish (a
    lane, "" for any) besides the queue's, `key` the caller; `idle=False` for
    the caller's own lease, which is alive by its call. A lease with a live run
    is never stale (Re-check fix5 M1): neither a gone worktree nor a short
    `take --ttl` frees a lane a run still builds on."""
    if _run_live(h, ctx):
        return ""
    if h.get("tree") and not os.path.isdir(h["tree"]):
        return "its worktree is gone"
    if any(_alive(p) for p in h["pids"]):
        return ""
    if ctx.now - h["seen"] > _hold_ttl(h, st) * 60:
        return "last seen %d min ago, no live process" % ctx.mins(h["seen"])
    who = _wanted(h, st, want, key) if idle and not h.get("hold_min") \
        and ctx.now - h["seen"] > LEASE_IDLE_MIN * 60 else ""
    if not who:
        return ""
    # V2-P1196-2: the caller's own wish counts (R-TV-W21 b) -- name it, never
    # "a waiter" while the queue holds none.
    if who in {w["strand"] for w in st["waiting"]}:
        by = "with a waiter for it (%s)" % who
    else:
        by = "wanted by the caller %s, no waiter in the queue" % who
    return ("idle %d min (over %d) %s, no run and no live process"
            % (ctx.mins(h["seen"]), LEASE_IDLE_MIN, by))


def _wanted(h, st, want=None, key=None):
    """Does a blocked wish want what `h` holds? A waiter's wish (or the caller's
    `want`, "" for any lane) counts only while nothing free serves it -- Re-Check
    tv-kit M1: counting every wish took the idle warm lane of a builder between
    two test-tier runs back on any `take` without `--lane` while another lane
    stood free. A named lane counts when it is the one `h` holds; no wish counts
    when no lane is left after the waiters ahead (in a pool without lanes: no
    token left). A caller that already holds a lease wants nothing more.
    Returns who wants it -- the first such waiter, else the caller `key` -- or ""."""
    queue = st["waiting"]
    asks = [(w["strand"], w.get("lane") or "", queue[:i]) for i, w in enumerate(queue)]
    if want is not None and not (key and _holder(st, key)):
        names = [w["strand"] for w in queue]
        asks.append((key or "", want, queue[:names.index(key)] if key in names else queue))
    return next((who or "?" for who, lane, ahead in asks if _blocked(h, st, lane, ahead)), "")


def _blocked(h, st, lane, ahead):
    """Is a wish for `lane` ("" for any) behind the waiters `ahead` served by nothing free?"""
    if not st.get("lanes"):
        return len(ahead) >= st["max"] - len(st["holders"])
    if lane:
        return lane == h.get("lane")
    return not _lanes_left(st, ahead)


def _live_run_of(h, ctx):
    """The live run a lease carries -- its `run`, or any run on its lane -- else None.
    The one liveness test of every site that takes over, frees or reclaims a
    lease (Re-check fix5 M1): a lease with a live run moves only with `--force`."""
    for r in _live_runs(ctx.db, ctx.now):
        if (h.get("run") and r["run"] == h["run"]) or (h.get("lane") and r["lane"] == h["lane"]):
            return r
    return None


def _run_live(h, ctx):
    """fail-closed: a holder whose run (or any run on its lane) may still build keeps it."""
    return _live_run_of(h, ctx) is not None


def _stale_wait(w, st, ctx):
    # A waiter in its own worktree is alive as long as the tree is; the TTL is
    # for waiters queued from outside (`--strand`), which have no tree.
    if w.get("tree"):
        return "" if os.path.isdir(w["tree"]) else "its worktree is gone"
    if ctx.now - w["since"] > st["ttl_min"] * 60:
        return "waiting %d min, nobody took it up" % ctx.mins(w["since"])
    return ""


def _holder(st, strand):
    return next((h for h in st["holders"] if h["strand"] == strand), None)


def _batch_speaker(st, key, want):
    """The member lease a batch holder `<wave>/S1+S2` speaks for (merge `_members`):
    the member holding the lane `want`, else (no wish) the first member holding
    a lease; `key` itself when it holds one or no member does (kit find (h)).
    The `+` in a holder's last segment is reserved syntax for every pass, not
    only the merge gate: `gate --strand w/a+b` by hand runs on a's or b's lease."""
    prefix, _, names = (key or "").rpartition("/")
    if "+" not in names or _holder(st, key):
        return key
    members = ["%s/%s" % (prefix, n) if prefix else n for n in names.split("+")]
    held = [h for m in members for h in st["holders"] if h["strand"] == m]
    pick = next((h for h in held if h.get("lane") == want), None) if want else \
        (held[0] if held else None)
    return pick["strand"] if pick else key


def _host_lanes(ctx, lanes_file):
    try:
        with open(lanes_file, encoding="utf-8") as fh:
            text = fh.read()
    except OSError:
        ctx.fail("token init: --lanes needs the host file %s (lines "
                 "'<name> <ssh-target> [<bmc>]')" % lanes_file)
    names = []
    for line in text.splitlines():
        words = line.split("#", 1)[0].split()
        if len(words) >= 2:
            names.append(words[0])
    return names


def _lanes_left(st, ahead):
    """The free lanes in the order of `init --lanes`, minus what the waiters
    `ahead` get first: a named lane is reserved, a waiter without a wish takes
    the first free lane (FIFO)."""
    held = {h.get("lane") for h in st["holders"]}
    left = [n for n in st.get("lanes") or [] if n not in held]
    for w in ahead:
        if w.get("lane"):
            if w["lane"] in left:
                left.remove(w["lane"])
        elif left:
            left.pop(0)
    return left


def _admission_order(db, left):
    """`left` in the order the admission tries lanes (kit find (a), 09.10.): a
    warm target first, then the runners' order; a drained or unreachable runner
    last; a lane without a runner keeps its place of `init --lanes`."""
    runners = _runners(db)
    pos = {n: i for i, n in enumerate(_warm_first(db, runners))}
    bad = {r["name"] for r in runners if r["state"] in ("drained", "unreachable")}
    return sorted(left, key=lambda n: (n in bad, pos.get(n, len(pos)), left.index(n)))


def _warm_first(db, runners):
    """The admission's lane order, shared with `take` (Review Z-A2 R3): a warm target first, then the runners'."""
    warm = {r["runner"] for r in db.execute(
        "SELECT DISTINCT runner FROM workspaces WHERE kind = 'target' AND state = 'warm'")}
    return ([r["name"] for r in runners if r["name"] in warm]
            + [r["name"] for r in runners if r["name"] not in warm])


def _refuse_over_a_queue(st, ctx, verb, way_out):
    if st and (st["holders"] or st["waiting"]):
        who = ["%s holds a cargo token" % h["strand"] for h in st["holders"]]
        who += ["%s waits in the queue" % w["strand"] for w in st["waiting"]]
        ctx.fail("token %s: %s -- %s" % (verb, ", ".join(who), way_out))


def _refuse_over_a_live_run(ctx, verb, lanes, way_out):
    """Re-check fix6 M1: a run admitted while the pool was off has no holder, so
    arming over it handed its lane to the next `take` or admission (probe P1:
    `init` 0, then `take welle-x/c --lane south` 0 under the live run). Arming
    refuses while a run is live on a lane it arms (with `--max`: any live run);
    `init --force` stays the way through, so the grant itself stays as it is
    (`release --force`). Re-check fix7 M2: no exception for a carried-over
    holder of that lane -- the old token file cannot say whose run it is, and a
    foreign holder (welle-x/c over run-A of welle-x/b) got the lane under the run."""
    for r in _live_runs(ctx.db, ctx.now):
        if not lanes or r["lane"] in lanes:
            ctx.fail("token %s: run %s of %s is live on lane %s -- %s"
                     % (verb, r["run"], r["key"], r["lane"], way_out))


def _offer_of(st, i):
    w = st["waiting"][i]
    if st.get("lanes"):
        left = _lanes_left(st, st["waiting"][:i])
        if w.get("lane"):
            return w["lane"] if w["lane"] in left else None
        return left[0] if left else None
    return "" if i < st["max"] - len(st["holders"]) else None


def _offer_name(w):
    return w["offered"]["lane"] if w["offered"].get("lane") else "a token"


def _mark_offers(st, ctx):
    """Stamp each waiter with the token offered to it; an offer keeps its first
    time while the waiter would get SOME token and goes when it would get none."""
    for i, w in enumerate(st["waiting"]):
        got = _offer_of(st, i)
        if got is None:
            w.pop("offered", None)
            continue
        old = w.get("offered")
        if old and old.get("lane", "") == got:
            continue
        w["offered"] = {"at": old["at"] if old else ctx.now}
        if got:
            w["offered"]["lane"] = got
        ctx.log("offer", w["strand"], ("lane " + got) if got else "token",
                resource=("lane:" + got) if got else "token")


def _offer_not_taken(w, st, ctx):
    o = w.get("offered")
    return bool(o) and ctx.now - o["at"] >= OFFER_MIN * 60 \
        and ctx.now - w["since"] > st["ttl_min"] * 60


def _lane_tree_procs(db, name):
    """What lives in the lane tree of `name`: a list of pids ([] = nothing), None
    when the probe got no answer or the lane is known nowhere. The lane comes from
    its runner row, else from the host file (`lane.load_lanes`, MECLAW_LANES_FILE):
    a lane armed by `token init --lanes` before any gate or `lanes status` ran
    there has no runner row, and returning "nothing lives there" for it reclaimed
    the lease unprobed (re-check tv-kit fix10 M2). The test hook is asked first,
    so a test never reads the host file of the machine."""
    from . import lane as lane_mod
    row = db.execute("SELECT * FROM runners WHERE name = ?", (name,)).fetchone()
    if _lane_tree_probe is not None:
        return _lane_tree_probe(_lane_of(row) if row is not None
                                else lane_mod.Lane(name=name, ssh=""))
    if row is not None:
        ln = _lane_of(row)
    else:
        try:
            ln = next((x for x in lane_mod.load_lanes() if x.name == name), None)
        except ValueError:      # fix11 H3: a host file that is no UTF-8 is no answer
            return None
        if ln is None:
            return None
    try:
        return lane_mod.ws_procs(ln, ln.root)
    except lane_mod.LaneError:
        return None


_PROBE_FAULTS = {}   # lane -> "<class>: <text>" of the probe's last exception


def _tree_answer(db, name):
    """`_lane_tree_procs` for the verbs: a probe that raises anything else (an
    OSError at the spawn, a sqlite error) is no answer, never a traceback -- the
    release ended with exit 1 and no line, the lease kept but unexplained
    (re-check tv-kit fix13 H1, probe Q2). The exception is kept for the line
    (`_no_answer`): swallowed without a trace, a lasting programming error of the
    probe read like a network blip and pushed every release to --force
    (re-check tv-kit fix14 H2)."""
    _PROBE_FAULTS.pop(name, None)
    try:
        return _lane_tree_procs(db, name)
    except Exception as e:  # noqa: BLE001 -- fail-closed: no answer keeps the lease
        _PROBE_FAULTS[name] = "%s: %s" % (type(e).__name__, e)
        return None


def _no_answer(lane):
    """The words for a lane tree probe without an answer, with the exception that
    ended it when there was one."""
    fault = _PROBE_FAULTS.get(lane)
    return "the lane tree probe got no answer" + (" (%s)" % fault if fault else "")


def _probe_stale_lanes(db, key, want, now, batch=False):
    """Probe the lane tree of every lane lease `_prune` would reclaim -- BEFORE the
    `BEGIN IMMEDIATE` of the verb: an ssh probe (connect timeout 10 s, probe 30 s)
    inside it would hold the store's write lock past its 5 s busy timeout. A lease
    that turns stale between this read and the transaction finds no answer in
    `ctx.lane_procs` and stays (fail-closed); the next call probes it. The other
    direction stays open: a hand run that starts after the probe (up to 10 + 30 s
    before `BEGIN IMMEDIATE`) is not seen and the lease moves -- closing it needs a
    lock on the lane itself; hand runs in a lane tree are forbidden (re-check fix10 H4)."""
    st = _load(db)
    if not st or not st.get("lanes"):
        return {}
    if batch:
        key = _batch_speaker(st, key, want)
    ctx = _Ctx(db, now, branch=key)
    out = {}
    for h in st["holders"]:
        if h.get("lane") and h["lane"] not in out \
                and _stale(h, st, ctx, want=want, idle=h["strand"] != key, key=key):
            out[h["lane"]] = _tree_answer(db, h["lane"])
    return out


def _lane_tree_busy(h, ctx, why):
    """True when the stale lease `h` must stay: a process lives in its lane tree,
    the probe got no answer, or nothing probed it in this verb (R-TV-W43: no
    takeover of a live run). Notes `stale-but-busy` once per busy spell."""
    procs = ctx.lane_procs.get(h["lane"], _UNPROBED)
    if procs is _UNPROBED:
        return True
    if procs == []:
        return False
    what = (_no_answer(h["lane"]) if procs is None else
            "pid %s live in the lane tree" % ", ".join(str(p) for p in procs))
    detail = "%s; %s" % (why, what)
    _, prev = _busy_spell(ctx.db, h)
    # one note per busy spell: `why` ages with every call ("idle 16 min", "17 min"),
    # what lives in the tree does not while the same hand run goes on; the
    # holder's return ends a spell (`_busy_spell`), so the next one is noted anew
    if not prev or not str(prev.get("detail")).endswith("; " + what):
        ctx.log("stale-but-busy", h["strand"], detail, resource=h["resource"], run=h.get("run"))
    # Re-check fix10 M1: the old hint "once the lane is idle, 'token release
    # --strand S' frees it" was run at once by the next agent while the pid
    # still mutated the tree. A pid needs no hand: the next take moves the lease
    # once the tree is idle. Only a probe without an answer needs a look.
    way = ("it moves by itself at the next take once the tree is idle" if procs else
           "check the lane by hand, then 'token release --force --strand %s'" % h["strand"])
    ctx.say("kept the lease of %s on lane %s -- %s, but %s (stale-but-busy: no takeover of a "
            "live run; lane runs only through the runner); %s."
            % (h["strand"], h["lane"], why, what, way))
    return True


def _last_lease_note(db, holder, resource):
    """The last lease journal line of `holder` on `resource` as (at, data) -- ("", {})
    without one. The resource filter is load-bearing: `strand.sh merge` journals
    lease_taken/lease_released of `tree:master@<repo>` under the strand's own name
    (merge.take_tree/release_tree), and that line hid a `stale-but-busy` note of
    the strand's lane lease -- the release freed the lane unprobed under a live pid
    (re-check tv-kit fix13 M1, probe Q1). Every note carries its lease's resource."""
    last = db.execute(
        "SELECT at, data FROM journal WHERE event IN ('lease_taken', 'lease_released', "
        "'lease_offered', 'lease_noted') AND json_extract(data, '$.holder') = ? "
        "AND json_extract(data, '$.resource') = ? "
        "ORDER BY seq DESC LIMIT 1", (holder, resource)).fetchone()
    return (last["at"], json.loads(last["data"])) if last else ("", {})


def _busy_note(db, h):
    """The `stale-but-busy` note that speaks for the lease `h` as (at, data), ("", {})
    without one: the last lease line of its holder, written since the lease was
    taken. Nothing else lifts it -- `seen` says the holder is back, not that the
    hand run left the tree, and a stranger's `check` sets it too (re-check tv-kit
    fix12 M1, probes P1/P2/P5); a lease line of the holder on another resource (the
    merge lease) is not its line (fix13 M1); `lease_release` probes the tree under it."""
    at, note = _last_lease_note(db, h["strand"], h["resource"])
    if note.get("verb") != "stale-but-busy" or _epoch(at) < int(h["since"]):
        return "", {}
    return at, note


def _busy_spell(db, h):
    """`_busy_note` while its busy spell lasts: the holder was not seen after it
    (check, take, the admission and the close of a run on the lease refresh `seen`
    without a journal line). Only
    the one-note-per-spell rule of `_lane_tree_busy` reads it; a heartbeat in the
    note's own second keeps the spell (re-check fix12 H2)."""
    at, note = _busy_note(db, h)
    if note and int(h["seen"]) > _epoch(at):
        return "", {}
    return at, note


def _release_probe(db, holder, force):
    """The lane tree probe of `lease_release` -- BEFORE its `BEGIN IMMEDIATE`, as
    `_probe_stale_lanes`: only for a lane lease a `stale-but-busy` note speaks for
    (`_busy_note`), never under `--force` (k3 narrowed, re-check fix12 M1).
    Returns {lane: answer}; a note that appears between this read and the
    transaction finds no answer and keeps the lease (fail-closed)."""
    if force:
        return {}
    st = _load(db)
    mine = _holder(st, holder) if st else None
    if not mine or not mine.get("lane") or not _busy_note(db, mine)[1]:
        return {}
    return {mine["lane"]: _tree_answer(db, mine["lane"])}


def _prune(st, ctx, key, want=None):
    """The first half of `take`: reclaim stale holders, drop dead waiters and
    waiters past the TTL that left their offer untaken (the caller is exempt).
    `want`: the caller's lane wish ("" for any), so an idle lease goes back on
    the caller's first call, not only once it is queued (R-TV-W21 b)."""
    for h in list(st["holders"]):
        why = _stale(h, st, ctx, want=want, idle=h["strand"] != key, key=key)
        if why and h.get("lane") and _lane_tree_busy(h, ctx, why):
            continue
        if why:
            st["holders"].remove(h)
            ctx.log("stale", h["strand"], why, resource=h["resource"], run=h.get("run"))
            ctx.say("reclaimed the cargo token of %s -- %s (ttl %d min)"
                    % (h["strand"], why, _hold_ttl(h, st)))
    for w in list(st["waiting"]):
        why = "" if w["strand"] == key else _stale_wait(w, st, ctx)
        if why:
            st["waiting"].remove(w)
            ctx.log("stale-wait", w["strand"], why)
            ctx.say("dropped %s from the queue -- %s (ttl %d min)"
                    % (w["strand"], why, st["ttl_min"]))
    _mark_offers(st, ctx)
    while True:
        late = next((w for w in st["waiting"]
                     if w["strand"] != key and _offer_not_taken(w, st, ctx)), None)
        if late is None:
            break
        st["waiting"].remove(late)
        what = _offer_name(late)
        if late["offered"].get("lane"):
            what = "lane " + what
        ctx.log("dropped", late["strand"],
                "waiter past ttl, did not take %s offered at %s, waiting %d min"
                % (what, _iso(late["offered"]["at"]), ctx.mins(late["since"])))
        ctx.say("dropped %s from the queue -- waiter past ttl, did not take %s offered %d min ago "
                "(ttl %d min)" % (late["strand"], what, ctx.mins(late["offered"]["at"]),
                                  st["ttl_min"]))
        _mark_offers(st, ctx)


def _grant(st, ctx, key, lane, tree, ttl_min=None, run=None):
    st["waiting"] = [w for w in st["waiting"] if w["strand"] != key]
    if lane:
        resource = "lane:" + lane
    else:
        used = {h["resource"] for h in st["holders"]}
        resource = next("token:%d" % n for n in range(1, st["max"] + len(used) + 2)
                        if "token:%d" % n not in used)
    entry = {"strand": key, "since": ctx.now, "seen": ctx.now, "pids": [], "tree": tree,
             "resource": resource, "ttl_min": ttl_min, "run": run}
    if lane:
        entry["lane"] = lane
    st["holders"].append(entry)
    ctx.log("take", key, ("lane " + lane) if lane else "", resource=resource, run=run,
            ttl_s=(ttl_min or st["ttl_min"]) * 60)
    return entry


def _enqueue(st, ctx, key, want, tree, wait=False):
    """Queue `key` (or keep its place) and return the refusal line of `take`;
    `wait`: the caller blocks itself (`take --wait`, `gate --wait-admission`)."""
    names = [w["strand"] for w in st["waiting"]]
    pos = names.index(key) if key in names else len(names)
    free = st["max"] - len(st["holders"])
    if key not in names:
        entry = {"strand": key, "since": ctx.now, "tree": tree}
        if want:
            entry["lane"] = want
        st["waiting"].append(entry)
        ctx.log("wait", key, ("lane " + want) if want else "")
    elif want:
        st["waiting"][pos]["lane"] = want
    held = ", ".join("%s %d min" % (h["strand"], ctx.mins(h["since"])) for h in st["holders"])
    if free <= 0:
        head = "all %d cargo tokens are held (%s)" % (st["max"], held)
    else:
        head = ("%d of %d cargo tokens are held (%s) and the queue goes first"
                % (len(st["holders"]), st["max"], held or "none"))
    if want:
        by = next((h["strand"] for h in st["holders"] if h.get("lane") == want), "")
        head = ("lane %s is held by %s" % (want, by)) if by else \
            ("lane %s goes to a strand ahead in the queue" % want)
    if wait:
        return ("%s -- %s is #%d in the queue; %s blocks here until it is taken."
                % (head, key, pos + 1, wait if isinstance(wait, str) else "take --wait"))
    return ("%s -- %s is #%d in the queue. End your turn; you are woken when a "
            "token is free." % (head, key, pos + 1))


def _by(ctx, key):
    """"" for the holder's own branch -- `<strand>:<n>` is the strand's own too (R2 M5)."""
    # The admission's close sets the branch to the holder key itself (re-check Z-A M-a).
    if ctx.branch and ctx.branch in (key, key.split(":", 1)[0]):
        return ""
    return " (released by %s)" % (ctx.branch or "a detached tree")


def _release_entry(st, ctx, key, mine):
    st["holders"] = [h for h in st["holders"] if h["strand"] != key]
    st["waiting"] = [w for w in st["waiting"] if w["strand"] != key]
    by = _by(ctx, key)
    ctx.log("release" if not by else "release-by", key, "held %d min" % ctx.mins(mine["since"]),
            resource=mine["resource"], run=mine.get("run"))
    return by


def lease_init(db, *, max=None, lanes=None, ttl=90, force=False, lanes_file=None, now=None,
               branch=None, path=""):  # noqa: A002 -- the old flag name
    """`init [--max n | --lanes a,b] [--ttl min] [--force]`: arm the pool."""
    _ensure(db)
    ctx = _Ctx(db, _now(now), branch, path)
    try:
        with _tx(db):
            cap = _whole(ctx, "init", "--max", 3 if max is None else max)
            ttl_min = _whole(ctx, "init", "--ttl", ttl)
            raw = ",".join(lanes) if isinstance(lanes, (list, tuple)) else (lanes or "")
            names = [n for n in raw.split(",") if n]
            if raw and names:
                lf = lanes_file or os.path.expanduser("~/.config/meclaw/lanes")
                known = _host_lanes(ctx, lf)
                unknown = [n for n in names if n not in known]
                if unknown:
                    ctx.fail("token init: no lane %s in the host file %s" % (", ".join(unknown), lf))
                if len(set(names)) != len(names):
                    ctx.fail("token init: a lane is named twice in --lanes")
                cap = len(names)
            elif raw:
                ctx.fail("token init: --lanes needs at least one name")
            if not force:
                _refuse_over_a_queue(_load(db), ctx, "init", "pass --force to start over")
                _refuse_over_a_live_run(ctx, "init", names,
                                        "wait for its close or pass --force")
            db.execute("DELETE FROM lease_entries WHERE pool = ?", (POOL,))
            db.execute("INSERT OR REPLACE INTO lease_pools (pool, max, ttl_min, armed, lanes) "
                       "VALUES (?, ?, ?, ?, ?)", (POOL, cap, ttl_min, _iso(ctx.now),
                                                  json.dumps(names) if names else None))
            st = {"max": cap, "ttl_min": ttl_min, "holders": [], "waiting": []}
            if names:
                st["lanes"] = names
            _save(db, st, ctx)
            ctx.log("init --force" if force else "init", "-", "max %d ttl %d%s"
                    % (cap, ttl_min, (" lanes " + ",".join(names)) if names else ""))
            if names:
                ctx.say("build lanes armed: %s, ttl %d min (%s)" % (" ".join(names), ttl_min, path))
            else:
                ctx.say("cargo tokens armed: %d, ttl %d min (%s)" % (cap, ttl_min, path))
        return ctx.result(0)
    except _Exit as ex:
        return ctx.result(ex.code)


def _whole(ctx, verb, name, value):
    try:
        n = int(value)
    except (TypeError, ValueError):
        n = 0
    if n < 1:
        ctx.fail("token %s: %s must be a whole number of at least 1, not %r" % (verb, name, value))
    return n


def _unarmed(ctx, verb):
    if verb == "who":
        ctx.print("cargo tokens are not armed on this host (%s)" % ctx.path)
    elif verb == "off":
        ctx.say("cargo tokens are not armed on this host -- nothing to switch off")
    elif verb == "take":
        ctx.say("cargo tokens are not armed on this host -- nothing serialises this "
                "strand's cargo; %s, then take again (%s)" % (ARM_HINT, ctx.path))
        return ctx.result(2)
    elif verb == "release":
        ctx.say("cargo tokens are not armed on this host -- nothing to %s" % verb)
    return ctx.result(0)


def lease_off(db, *, force=False, now=None, branch=None, path=""):
    """`off [--force]`: disarm the pool; the journal (the log) stays."""
    _ensure(db)
    ctx = _Ctx(db, _now(now), branch, path)
    try:
        with _tx(db):
            st = _load(db)
            if not force:
                _refuse_over_a_queue(st, ctx, "off", "release them or pass --force")
            if st is None:
                return _unarmed(ctx, "off")
            db.execute("DELETE FROM lease_entries WHERE pool = ?", (POOL,))
            db.execute("DELETE FROM lease_pools WHERE pool = ?", (POOL,))
            db.execute("DELETE FROM leases WHERE resource LIKE 'lane:%' OR resource LIKE 'token:%'")
            ctx.log("off --force" if force else "off", "-")
            ctx.say("cargo tokens switched off; the log stays (%s)" % path)
        return ctx.result(0)
    except _Exit as ex:
        return ctx.result(ex.code)


def lease_who(db, *, now=None, path=""):
    """`who`: holders, the queue with offers, the last five log lines."""
    _ensure(db)
    ctx = _Ctx(db, _now(now), None, path)
    with _tx(db):   # one snapshot: `who` must not see half a queue
        return _who(db, ctx)


def _who(db, ctx):
    st = _load(db)
    if st is None:
        return _unarmed(ctx, "who")
    ctx.print("cargo tokens: %d/%d held, ttl %d min, armed %s"
              % (len(st["holders"]), st["max"], st["ttl_min"], st.get("armed") or "?"))
    if st.get("lanes"):
        ctx.print("lanes: %s" % " ".join(st["lanes"]))
    for h in st["holders"]:
        why = _stale(h, st, ctx)
        pids = ", ".join("%d (%s)" % (p, "alive" if _alive(p) else "gone") for p in h["pids"])
        ctx.print("  %s%s  held %d min, seen %d min ago, pid %s, %s%s"
                  % (h["strand"], ("  lane " + h["lane"]) if h.get("lane") else "",
                     ctx.mins(h["since"]), ctx.mins(h["seen"]), pids or "-",
                     _liveness(db, h, ctx), ("  STALE: " + why) if why else ""))
    if st["waiting"]:
        ctx.print("queue:")
        for n, w in enumerate(st["waiting"], 1):
            why = _stale_wait(w, st, ctx)
            if why:
                note = "  STALE: " + why
            elif ctx.now - w["since"] > st["ttl_min"] * 60:
                note = "  waiting beyond the ttl"
            else:
                note = ""
            if w.get("offered"):
                note += "  offered %s at %s" % (_offer_name(w), _iso(w["offered"]["at"]))
            ctx.print("  #%d %s%s  waiting %d min%s"
                      % (n, w["strand"], ("  for lane " + w["lane"]) if w.get("lane") else "",
                         ctx.mins(w["since"]), note))
    else:
        ctx.print("queue: empty")
    rows = db.execute("SELECT at, data FROM journal WHERE event LIKE 'lease_%' "
                      "ORDER BY seq DESC LIMIT 5").fetchall()
    if rows:
        ctx.print("last events:")
        for row in reversed(rows):
            d = json.loads(row["data"] or "{}")
            ctx.print("  " + "  ".join([row["at"], d.get("verb", ""), d.get("holder") or "-",
                                        d.get("branch") or "-", d.get("detail", "")]))
    return ctx.result(0)


def _last_station(db, run):
    """(epoch, `GATE <station>`) of the last station the pass `run` journaled, else None."""
    j = db.execute("SELECT at, data FROM journal WHERE run = ? AND event IN "
                   "('station_started', 'station_ended') ORDER BY seq DESC LIMIT 1",
                   (run,)).fetchone()
    if not j:
        return None
    return (int(store.parse_at(j["at"]).timestamp()),
            "GATE %s" % json.loads(j["data"] or "{}").get("station", "?"))


def _keep_last_station(db, h, run):
    """GH #1199 V2-E-2: the end of a pass on a held lease clears `run`, the only
    way `who` read its stations -- 15.10. 03:06 a lease said "no progress since
    take" right after a 35-min gate. Its last station becomes the lease's
    progress row (the row `progress.beat` writes), unless a newer one is there."""
    last = _last_station(db, run)
    if last is None or last[0] < h["since"]:
        return
    since = _iso(h["since"])
    row = db.execute("SELECT at FROM lease_progress WHERE pool = ? AND holder = ? AND since = ?",
                     (POOL, h["strand"], since)).fetchone()
    if row and _epoch(row["at"]) > last[0]:
        return
    db.execute("INSERT OR REPLACE INTO lease_progress (pool, holder, since, at, state, line) "
               "VALUES (?, ?, ?, ?, 'progress', ?)", (POOL, h["strand"], since, _iso(last[0]), last[1]))


def _last_progress(db, h):
    """(epoch, state, line) of the last progress of the lease `h`, None without
    one: the row its run's `progress.beat` wrote (an ended pass leaves its last
    station there, `_keep_last_station`), or the last station of the gate pass
    the lease carries (`run`, journaled as the stream arrives)."""
    row = db.execute("SELECT at, state, line FROM lease_progress WHERE pool = ? AND holder = ? "
                     "AND since = ?", (POOL, h["strand"], _iso(h["since"]))).fetchone()
    best = (_epoch(row["at"]), row["state"] or "progress", row["line"] or "") if row else None
    last = _last_station(db, h["run"]) if h.get("run") else None
    if last and last[0] >= h["since"] and (best is None or last[0] >= best[0]):
        best = (last[0], "progress", last[1])
    return best


def _liveness(db, h, ctx):
    """`progress N min ago`, `REFUSED N min ago: <line>` or `no progress since
    take` (GH #1199: a run that never started showed a held lane for 4 h)."""
    last = _last_progress(db, h)
    if last is None:
        return "no progress since take"
    at, state, line = last
    if state == "progress":
        return "progress %d min ago" % ctx.mins(at)
    return "%s %d min ago: %s" % (state.upper(), ctx.mins(at), line)


def lease_lane(db, holder):
    """`lane`: the lane `holder` holds, or nothing."""
    _ensure(db)
    with _tx(db):
        st = _load(db)
    mine = _holder(st, holder) if st and holder else None
    if mine and mine.get("lane"):
        return LeaseResult(0, mine["lane"] + "\n", "", mine["lane"])
    return LeaseResult(0)


def lease_speaker(db, holder, want=""):
    """The lease `holder` runs on: for a batch `<wave>/S1+S2` the member lease
    it speaks for (`_batch_speaker`, as the admission resolves it), else
    `holder` itself (re-check RC-GK2 I2: the merge point's golden generator)."""
    _ensure(db)
    with _tx(db):
        st = _load(db)
    return _batch_speaker(st, holder, want or "") if st and holder else holder


def lease_on_lane(db, holder, lane):
    """The lease of `holder` that holds `lane`: `holder` itself, else its one
    second lease `holder:<n>` on that lane (GH #1146), else "" (kit find
    "holder push": a run of `S` on the lane of `S:n` entered its PID nowhere)."""
    _ensure(db)
    with _tx(db):
        st = _load(db)
    if not st or not holder or not lane:
        return ""
    mine = _holder(st, holder)
    if mine:
        return holder if mine.get("lane") == lane else ""
    second = [h["strand"] for h in st["holders"]
              if h["strand"].startswith(holder + ":") and h.get("lane") == lane]
    return second[0] if len(second) == 1 else ""


def lease_offer(db, *, now=None, branch=None, path=""):
    """`offer`: the waiters a free token or lane is offered to, one line each."""
    _ensure(db)
    ctx = _Ctx(db, _now(now), branch, path)
    with _tx(db):
        st = _load(db)
        if st is None:
            return ctx.result(0)
        _save(db, st, ctx)
        for w in st["waiting"]:
            if w.get("offered"):
                what = ("lane " + w["offered"]["lane"]) if w["offered"].get("lane") else "a token"
                ctx.print("%s  %s  offered at %s" % (w["strand"], what, _iso(w["offered"]["at"])))
    return ctx.result(0)


def lease_check(db, holder, *, pid=None, lane=None, caller=None, skip="", now=None, branch=None,
                path=""):
    """`check [--pid n] [--lane x]`: a heartbeat of the holder; exit 3 without a lease."""
    _ensure(db)
    ctx = _Ctx(db, _now(now), branch, path)
    pid_arg = "" if pid is None else str(pid)
    caller = int(caller if caller is not None else os.getppid())
    try:
        with _tx(db):
            st = _load(db)
            if st is None or not holder:
                return ctx.result(0)
            if skip:
                ctx.log("skip", holder, skip)
                _save(db, st, ctx)
                ctx.say("token check skipped for %s: %s" % (holder, skip))
                return ctx.result(0)
            mine = _holder(st, holder)
            if mine and lane and st.get("lanes") and mine.get("lane") != lane:
                ctx.log("refused", holder, "lane %s" % lane)
                ctx.say("%s holds lane %s, not %s -- gate on your own lane, or release "
                        "and take again" % (holder, mine.get("lane") or "-", lane))
                return ctx.result(3)
            if mine:
                mine["seen"] = ctx.now
                if pid_arg:
                    pids = [p for p in mine["pids"] if p != caller and _alive(p)]
                    new_pid = int(pid_arg) if pid_arg.isdigit() else 0
                    if new_pid and new_pid not in pids:
                        pids.append(new_pid)
                    mine["pids"] = pids
                _save(db, st, ctx)
                return ctx.result(0, mine.get("lane"))
            if pid_arg == "0":
                return ctx.result(3)
            ctx.log("refused", holder)
            ctx.say(REFUSED % holder)
            return ctx.result(3)
    except _Exit as ex:
        return ctx.result(ex.code)


def lease_release(db, holder, *, now=None, branch=None, path="", force=False):
    """`release`: give the token back or leave the queue. A lease that carries a
    live run (its workpackage still active on the lane) stays: the run's close
    frees it -- a second run of the same strand released it under the first one
    (kit find (c), 09.10.); `--force` releases it anyway. So does a lane lease
    whose last journal note is `stale-but-busy` (a hand run or no answer in its
    lane tree, re-check tv-kit fix10 M1) since its take: the release probes the
    lane tree (before the transaction) and frees it only when the probe finds it
    empty; a pid or no answer is exit 3 (fix12 M1). `--force` frees it unprobed
    and journals `release --force` with the note's finding."""
    _ensure(db)
    ctx = _Ctx(db, _now(now), branch, path)
    ctx.lane_procs = _release_probe(db, holder, force)
    try:
        with _tx(db):
            st = _load(db)
            if st is None:
                return _unarmed(ctx, "release")
            key = holder
            mine = _holder(st, key)
            queued = any(w["strand"] == key for w in st["waiting"])
            if not mine and not queued:
                ctx.say("%s holds no cargo token and is not in the queue -- nothing to release" % key)
                return ctx.result(0)
            if not mine:
                st["waiting"] = [w for w in st["waiting"] if w["strand"] != key]
                _save(db, st, ctx)
                by = _by(ctx, key)
                ctx.log("leave" if not by else "leave-by", key)
                ctx.say("%s left the queue%s" % (key, by))
                return ctx.result(0)
            busy = _live_run_of(mine, ctx)     # Re-check fix5 M1: run OR lane, as `_prune`
            live = [busy] if busy else []
            if live and not force:
                ctx.say("%s holds lane %s for the live run %s of %s -- the run's close frees it; "
                        "release --force frees it under the run" % (
                            key, mine.get("lane") or live[0]["lane"], live[0]["run"], live[0]["key"]))
                return ctx.result(3)
            # Re-check fix10 M1: a lane lease whose last note is stale-but-busy
            # had a hand run (or no answer) in its tree -- a release without
            # --force freed it under the run. Nothing lifts the note (fix12 M1:
            # `seen` freed it blindly under a live pid); the probe of
            # `_release_probe` decides: [] frees, a pid or no answer keeps it.
            at, note = _busy_note(db, mine) if mine.get("lane") else ("", {})
            hand = bool(note)
            what = str(note.get("detail")).rsplit("; ", 1)[-1] if hand else ""
            procs = ctx.lane_procs.get(mine["lane"], _UNPROBED) if hand else _UNPROBED
            if hand and not force and procs == []:
                hand, what = False, "tree probed empty"
            elif hand and not force:
                # fix13 H2: a note that came between `_release_probe` and this
                # transaction had no probe -- a second release probes it, so
                # pointing at --force (the "no answer" way) came too early.
                if procs is _UNPROBED:
                    seen, way = "the note came after the probe -- release again", "no probe ran yet"
                elif procs is None:
                    seen, way = (_no_answer(mine["lane"]),
                                 "check the lane by hand, then release --force")
                else:
                    seen = "pid %s live in the lane tree" % ", ".join(str(p) for p in procs)
                    way = ("the next take or release moves it once the tree is idle; a pid that "
                           "never leaves (a daemon) needs a check by hand, then release --force")
                # fix14 H1: without a probe the line says when the note was seen,
                # never "probed at" beside "no probe ran yet".
                when = ("noted after the probe at %s" if procs is _UNPROBED
                        else "probed at %s") % _iso(ctx.now)
                ctx.say("%s holds lane %s: stale-but-busy since %s; %s: %s -- %s"
                        % (key, mine["lane"], at, when, seen, way))
                return ctx.result(3)
            st["holders"] = [h for h in st["holders"] if h["strand"] != key]
            st["waiting"] = [w for w in st["waiting"] if w["strand"] != key]
            _save(db, st, ctx)
            by = _by(ctx, key)
            verb = ("release" if not by else "release-by") + (" --force" if live or hand else "")
            # fix11 H2: a force over the note says why, on stderr and in the journal
            over = ("; over stale-but-busy since %s: %s" % (at, what) if hand else
                    "; %s" % what if what else "")
            ctx.log(verb, key, "held %d min%s" % (ctx.mins(mine["since"]), over),
                    resource=mine["resource"], run=mine.get("run"))
            if hand:
                ctx.say("warning: released %s over stale-but-busy since %s (%s) -- a hand run "
                        "may still write in the tree of lane %s" % (key, at, what, mine["lane"]))
            if st["waiting"] and st.get("lanes") and mine.get("lane"):
                freed, nxt = mine["lane"], None
                for i, w in enumerate(st["waiting"]):
                    left = _lanes_left(st, st["waiting"][:i])
                    got = w["lane"] if w.get("lane") else (left[0] if left else None)
                    if got == freed and freed in left:
                        nxt = w
                        break
                if nxt:
                    tail = "next in the queue: %s (lane %s, waiting %d min)" % (
                        nxt["strand"], freed, ctx.mins(nxt["since"]))
                else:
                    tail = "no waiter takes lane %s (%d in the queue, each for a held lane)" % (
                        freed, len(st["waiting"]))
            elif st["waiting"]:
                nxt = st["waiting"][0]
                tail = "next in the queue: %s (waiting %d min)" % (nxt["strand"], ctx.mins(nxt["since"]))
            else:
                tail = "the queue is empty"
            ctx.say("cargo token released by %s after %d min -- %d/%d held; %s%s"
                    % (key, ctx.mins(mine["since"]), len(st["holders"]), st["max"], tail, by))
            return ctx.result(0)
    except _Exit as ex:
        return ctx.result(ex.code)


def _enter_pid(entry, pid):
    """Put the live process `pid` into the lease `entry` (a dead one, 0 or
    not a number enters nothing): `_stale` never frees a lease with a live
    process."""
    pid = str(pid or "")
    if pid.isdigit() and int(pid) > 1 and _alive(int(pid)) and int(pid) not in entry["pids"]:
        entry["pids"] = [p for p in entry["pids"] if _alive(p)] + [int(pid)]


def lease_take(db, holder, *, lane=None, ttl=None, tree="", now=None, branch=None, run=None,
               path="", wait=False, pid=None, queue=True):
    """`take [--lane x] [--ttl min] [--pid n]`: a token or a lane, or a place in
    the queue (exit 3). `--pid` enters the process that uses the lease (kit
    find "holder push", Welle Testing 10/12: a run on the lane of a holder
    lease `S:n` entered no PID -- `take` dropped `--pid`, only `check` took it
    -- and the lease fell away after its TTL under the running load).
    `queue=False` (the merge point's golden generator, #1199 V2-A17): exit 3
    without a queue place -- like the merge gate's admission (kit find (h)),
    a caller that never comes back for an offer must not hold one."""
    _ensure(db)
    ctx = _Ctx(db, _now(now), branch, path)
    key, want = holder, lane or ""
    ctx.lane_procs = _probe_stale_lanes(db, key, want, ctx.now)
    try:
        with _tx(db):
            st = _load(db)
            if st is None:
                return _unarmed(ctx, "take")
            ttl_min = _whole(ctx, "take", "--ttl", ttl) if ttl is not None else None
            _prune(st, ctx, key, want=want)
            if want and not st.get("lanes"):
                ctx.fail("token take: --lane %s, but this host is not armed with lanes" % want)
            if want and want not in st["lanes"]:
                ctx.fail("token take: no lane %s among the armed lanes (%s)"
                         % (want, " ".join(st["lanes"])))
            mine = _holder(st, key)
            if mine and want and mine.get("lane") != want:
                ctx.fail("%s holds lane %s, not %s -- release it first, then take %s"
                         % (key, mine.get("lane") or "-", want, want))
            if mine:
                mine["seen"] = ctx.now
                if ttl_min:
                    mine["ttl_min"] = mine["hold_min"] = ttl_min
                _enter_pid(mine, pid)
                _save(db, st, ctx)
                ctx.say("cargo token already held by %s (%d/%d)" % (key, len(st["holders"]), st["max"]))
                if mine.get("lane"):
                    ctx.print("lane %s" % mine["lane"])
                return ctx.result(0, mine.get("lane"))
            names = [w["strand"] for w in st["waiting"]]
            pos = names.index(key) if key in names else len(names)
            free = st["max"] - len(st["holders"])
            got = None
            if st.get("lanes"):
                left = _lanes_left(st, st["waiting"][:pos])
                if not want:
                    left = _admission_order(db, left)
                got = want if want in left else (None if want else (left[0] if left else None))
                granted = got is not None
            else:
                granted = pos < free
            if granted:
                entry = _grant(st, ctx, key, got or "", tree, ttl_min=ttl_min, run=run)
                entry["hold_min"] = ttl_min  # held on purpose: no idle reclaim
                _enter_pid(entry, pid)
                _save(db, st, ctx)
                ctx.say("cargo token %d/%d taken by %s" % (len(st["holders"]), st["max"], key))
                if got:
                    ctx.print("lane %s" % got)
                return ctx.result(0, got)
            if not queue:
                _save(db, st, ctx)
                ctx.say("%s -- no queue place: the caller does not wait for an offer" % (
                    ("lane %s is leased to another strand or a waiter ahead" % want) if want
                    else "every lane is leased to another strand or a waiter ahead"))
                return ctx.result(3)
            msg = _enqueue(st, ctx, key, want, tree, wait=wait)
            _save(db, st, ctx)
            ctx.say(msg)
            return ctx.result(3)
    except _Exit as ex:
        return ctx.result(ex.code)


def old_tokens_path(env):
    """The old token file `strand.sh` kept the pool in (its `token_file`)."""
    if env.get("MECLAW_STRAND_TOKENS"):
        return env["MECLAW_STRAND_TOKENS"]
    lock = env.get("MECLAW_GATE_LOCK") or OLD_LOCK
    return (lock[:-len(".lock")] if lock.endswith(".lock") else lock) + ".tokens"


def _superseded(db, t):
    """Did the store decide about the pool (init, off, a migration) at or after `t`?"""
    for row in db.execute("SELECT at, data FROM journal WHERE event = 'lease_noted'"):
        verb = (json.loads(row["data"] or "{}").get("verb") or "").split(" ")[0]
        if verb in ("init", "off", "migrate") and _epoch(row["at"]) >= t:
            return True
    return False


def lease_migrate(db, old_path, *, lanes_file=None, now=None, branch=None, path=""):
    """Carry the old token file into an unarmed pool: lanes, max, TTL, holders, queue.

    Z11: the W3 merge moved the token verbs onto the store and left the armed
    file behind, so the store was unarmed while the host was not. Once only: a
    store decision (init, off, an earlier migration) at or after the file's
    `armed` stamp wins over the file, which is only read, never written. A file
    that cannot be carried over is exit 2 with the reason, never a silent pass."""
    _ensure(db)
    ctx = _Ctx(db, _now(now), branch, path)
    if _armed(db):
        return ctx.result(0)
    try:
        with _tx(db):
            if _armed(db):
                return ctx.result(0)
            # Z15 (M2): the stamp comes from `stat` first -- it needs no read
            # right -- so an unreadable path an init/off outlived is no error.
            try:
                armed_t = int(os.stat(old_path).st_mtime)
            except FileNotFoundError:
                return ctx.result(0)
            except OSError as exc:
                armed_t = ctx.now
            try:
                text = open(old_path, encoding="utf-8").read()
            except FileNotFoundError:
                return ctx.result(0)
            except OSError as exc:
                text = None
                why = str(exc)
            try:
                if text is None:
                    raise ValueError(why)
                old = json.loads(text)
                armed_t = _epoch(old["armed"]) if old.get("armed") else armed_t
            except (ValueError, KeyError, TypeError, AttributeError) as exc:
                old, why = None, str(exc) or type(exc).__name__
            if _superseded(db, armed_t):
                return ctx.result(0)
            try:
                if old is None:
                    raise ValueError(why)
                cap, ttl_min = int(old["max"]), int(old["ttl_min"])
                names = list(old.get("lanes") or [])
                if cap < 1 or ttl_min < 1 or not isinstance(old["holders"], list) \
                        or not isinstance(old["waiting"], list):
                    raise ValueError("max, ttl_min, holders or waiting out of form")
                if names:
                    try:
                        known = _host_lanes(_Ctx(None, 0), lanes_file
                                            or os.path.expanduser("~/.config/meclaw/lanes"))
                    except _Exit:
                        known = []
                    unknown = [n for n in names if n not in known]
                    if unknown:
                        raise ValueError("no lane %s in the host file" % ", ".join(unknown))
                    cap = len(names)
                st = {"max": cap, "ttl_min": ttl_min, "holders": [], "waiting": []}
                if names:
                    st["lanes"] = names
                n = 0
                for h in old["holders"]:
                    lane = h.get("lane") or ""
                    if names and lane not in names:
                        raise ValueError("holder %s on no armed lane" % h["strand"])
                    pid = h.get("pid", 0)
                    if not lane:
                        n += 1
                    st["holders"].append({
                        "strand": h["strand"], "since": int(h["since"]), "seen": int(h["seen"]),
                        # Z15 (M5): 0 and below are no process (`kill(0, 0)` is the group).
                        "pids": [int(x) for x in h.get("pids", [pid] if pid else []) if int(x) > 1],
                        "tree": h.get("tree") or "",
                        "resource": ("lane:" + lane) if lane else "token:%d" % n,
                        "ttl_min": h.get("ttl_min"), "run": None})
                    if lane:
                        st["holders"][-1]["lane"] = lane
                for w in old["waiting"]:
                    entry = {"strand": w["strand"], "since": int(w["since"]),
                             "tree": w.get("tree") or ""}
                    if w.get("lane"):
                        entry["lane"] = w["lane"]
                    st["waiting"].append(entry)
            except (ValueError, KeyError, TypeError, AttributeError) as exc:
                ctx.fail("cargo tokens: the old token file %s cannot be carried over into "
                         "the store (%s) -- look at it, then %s"
                         % (old_path, str(exc) or type(exc).__name__, ARM_HINT))
            _refuse_over_a_live_run(ctx, "migrate", names,
                                    "wait for its close, then any token verb carries the old "
                                    "file %s over, or pass 'token init --force'" % old_path)
            db.execute("DELETE FROM lease_entries WHERE pool = ?", (POOL,))
            db.execute("INSERT OR REPLACE INTO lease_pools (pool, max, ttl_min, armed, lanes) "
                       "VALUES (?, ?, ?, ?, ?)", (POOL, cap, ttl_min, _iso(armed_t),
                                                  json.dumps(names) if names else None))
            _save(db, st, ctx)
            detail = "from %s max %d ttl %d%s holders %d waiting %d" % (
                old_path, cap, ttl_min, (" lanes " + ",".join(names)) if names else "",
                len(st["holders"]), len(st["waiting"]))
            ctx.log("migrate", "-", detail)
            ctx.say("cargo tokens migrated from the old token file %s into the store: %s, "
                    "ttl %d min, %d held, %d waiting (%s)"
                    % (old_path, ("lanes " + " ".join(names)) if names else "max %d" % cap,
                       ttl_min, len(st["holders"]), len(st["waiting"]), path))
        return ctx.result(0)
    except _Exit as ex:
        return ctx.result(ex.code)


def _migrate_old(env, cwd):
    """`lease_migrate` for one CLI verb, when an old token file is there at all."""
    old = old_tokens_path(env)
    if not os.path.exists(old):
        return 0, "", ""
    db_path, lanes_file = _paths(env)
    now = int(env.get("MECLAW_STRAND_NOW") or time.time())
    db = store.open(db_path)
    try:
        res = lease_migrate(db, old, lanes_file=lanes_file, now=now,
                            branch=_git(cwd, "rev-parse", "--abbrev-ref", "HEAD"), path=db_path)
    finally:
        db.close()
    return res.code, res.out, res.err


# --- the CLI: pipeline.py lanes <verb> -------------------------------------------------------

def _git(cwd, *args):
    try:
        res = subprocess.run(["git", "-C", cwd] + list(args), capture_output=True, text=True,
                             timeout=30)
    except (OSError, subprocess.SubprocessError):
        return ""
    return res.stdout.strip() if res.returncode == 0 else ""


def _paths(env):
    db_path = env.get("MECLAW_PIPELINE_DB") or os.path.expanduser(
        "~/.local/state/meclaw/pipeline/pipeline.db")
    lanes_file = env.get("MECLAW_LANES_FILE") or os.path.expanduser("~/.config/meclaw/lanes")
    return db_path, lanes_file


def _fingerprint_cli(argv, env):
    """`lanes fingerprint <lane>`: the lane's fingerprint now, into the store, and one
    line `<lane> browsers ok|<engine>: <why>; ...` (exit 1 on a refusal).

    Re-Check S3 Teil 2 m-2: a runner's fingerprint came only from a run's
    `lane_fingerprint` event (`ingest`), so a lane refused for an engine stayed
    refused after `lanes browsers <lane> install` until another run wrote a new one --
    and a refused lane gets no browser run. `strand.sh lanes browsers` calls this
    after every check and install."""
    if len(argv) != 1:
        return 2, "", "lanes fingerprint <lane>\n"
    db_path, lanes_file = _paths(env)
    if not os.path.isfile(lanes_file):
        return 2, "", "lanes fingerprint: no lane host file at %s\n" % lanes_file
    if _lane_override is not None:
        mod = _lane_override
    else:
        from . import lane as mod
    lanes = mod.load_lanes(lanes_file)
    found = [ln for ln in lanes if ln.name == argv[0]]
    if not found:
        return 2, "", "lanes fingerprint: no lane %s in %s\n" % (argv[0], lanes_file)
    try:
        fp = mod.fingerprint(found[0])
    except Exception as exc:  # pylint: disable=broad-except
        # fail-closed: the store keeps what it had, the caller sees why
        return 2, "", "lanes fingerprint: %s: %s\n" % (argv[0], exc)
    db = store.open(db_path)
    try:
        _ensure(db)
        sync_runners(db, lanes)
        with _tx(db):
            db.execute("UPDATE runners SET fingerprint = ? WHERE name = ?",
                       (json.dumps(fp, sort_keys=True), argv[0]))
        sync_runners(db, lanes)  # the capacity of the new fingerprint
    finally:
        db.close()
    refused = browser_refusals(fp)
    said = "; ".join("%s: %s" % kv for kv in refused.items()) or "ok"
    return (1 if refused else 0), "%s browsers %s\n" % (argv[0], said), ""


def _status(env, only=None):
    """`lanes status [<lane>]`: one line per lane of the host file, or of the
    one lane named (GH #1144: the disk probe goes through the host file)."""
    db_path, lanes_file = _paths(env)
    ctx = _Ctx(None, 0)
    if not os.path.isfile(lanes_file):
        ctx.say("no lane host file at %s" % lanes_file)
        return 2, "", _join(ctx.err)
    if _lane_override is not None:
        mod = _lane_override
    else:
        from . import lane as mod
    lanes = mod.load_lanes(lanes_file)
    probe = [ln for ln in lanes if not only or ln.name == only]
    if not probe:
        ctx.say("lanes status: no lane %s in %s" % (only, lanes_file))
        return 2, "", _join(ctx.err)
    now = int(env.get("MECLAW_STRAND_NOW") or time.time())
    raw, states, held, refusals = {}, {}, {}, {}
    for ln in probe:
        raw[ln.name] = mod.status(ln)
        states[ln.name] = "unreachable" if raw[ln.name].split()[:1] == ["unreachable"] else "idle"
    if os.path.exists(db_path):
        db = store.open(db_path)
        try:
            _ensure(db)
            held = _held(db, now)
            current = {r["name"]: r["state"] for r in db.execute("SELECT name, state FROM runners")}
            for name, state in list(states.items()):
                if state == "idle" and (current.get(name) == "busy" or held.get(name)):
                    states[name] = "busy"
            sync_runners(db, lanes, states=states)
            # the engines a lane's fingerprint refuses, with the reason (Re-Check S3
            # Teil 2, point 5): the admission books them 0 there
            for r in db.execute("SELECT name, fingerprint FROM runners"):
                refused = browser_refusals(json.loads(r["fingerprint"] or "{}"))
                if refused:
                    refusals[r["name"]] = "browsers refused " + "; ".join(
                        "%s: %s" % kv for kv in refused.items())
        finally:
            db.close()
    lines = []
    for ln in probe:
        line = raw[ln.name]
        if held.get(ln.name):
            # Z13: the lane's own `state idle` only sees cargo processes; a
            # lost run's admission kept build03's cpu while this line said
            # idle and two runs were refused `needs cpu 8+8 > 8`.
            line = "  ".join([re.sub(r"\bstate idle\b", "state leased", line)] + held[ln.name])
        if refusals.get(ln.name):
            line = "%s  %s" % (line, refusals[ln.name])
        lines.append("%s  %s" % (ln.name, line))
    # Review Zb R2 M6: the one lane named unreachable is no success.
    rc = 2 if only and states.get(only) == "unreachable" else 0
    return rc, _join(lines), ""


def _held(db, now):
    """The admissions that bind a lane's capacity now (`_live_runs`), one
    `lease <run> age <n>m deadline <iso> <state>/<reason>` word group each,
    by lane. The age counts from the run's first journal row (its dispatch)."""
    held = {}
    for live in _live_runs(db, now):
        row = db.execute("SELECT state, reason, doc, created_at FROM items WHERE id = ?",
                         (live["item"],)).fetchone()
        if row is None:
            continue
        doc = json.loads(row["doc"] or "{}")
        first = db.execute("SELECT MIN(at) FROM journal WHERE run = ?", (live["run"],)).fetchone()[0]
        since = first or row["created_at"]
        age = max(0, (now - _epoch(since)) // 60) if since else 0
        held.setdefault(live["lane"], []).append(
            "lease %s age %dm deadline %s %s/%s" % (live["run"], age, doc.get("deadline_at") or "-",
                                                    row["state"], row["reason"] or "-"))
    return held


# --- the wake file of the pool: `take --wait` blocks on it ------------------------------
# A FIFO beside the store (`<db>.wake`). A waiter opens its read end BEFORE
# each round of `take` and then blocks on it; a verb that may free a lane
# (release, offer, a granted take, init, off, check, the close of a run) opens
# and closes the write end after its commit, which wakes every open read end.
# Opening before the round closes the gap between "queued" and "blocked": a
# release committed after the round's read wakes the waiter. The one clock is
# a one-shot watchdog per round (`MECLAW_TOKEN_WAIT_SECS`, 30 s) for what has
# no event: a holder's TTL running out, its process or worktree vanishing.
WAKE_SUFFIX = ".wake"
HOLDER_RE = re.compile(r"^[A-Za-z0-9_-]{1,32}$")
HOLDER_VERBS = ("take", "release", "check", "lane")


def strand_colon_error(strand):
    """The refusal of the old form `--strand <s>:<n>` (kit find tv-br M5,
    Welle Testing M7d2), or None. `<s>:<n>` is the key of a strand's second
    lease (GH #1146) and was taken as `--strand` by accident -- by `token` as
    that key, by `test` whole into the push refspec
    `<sha>:refs/heads/lane/<s>:<n>`, whose colon splits src:dst. The second
    lease is `--holder <n>`; the message names the new form."""
    if strand and ":" in strand:
        base, _, holder = strand.partition(":")
        return ("--strand %s: a strand name carries no ':' -- the second lease of a strand "
                "is --strand %s --holder %s (GH #1146)" % (strand, base, holder))
    return None
WAKE_VERBS = ("take", "release", "offer", "init", "off", "check")


def _wake_open(db_path):
    """The read end of the pool's wake file (created on demand), or None."""
    fifo = db_path + WAKE_SUFFIX
    try:
        os.makedirs(os.path.dirname(fifo) or ".", exist_ok=True)
        try:
            os.mkfifo(fifo, 0o600)
        except FileExistsError:
            pass
        if not stat.S_ISFIFO(os.stat(fifo).st_mode):
            return None
        return os.open(fifo, os.O_RDONLY | os.O_NONBLOCK)
    except OSError:
        return None


def _wake(db_path):
    """Wake every waiter of the pool: open and close the write end (no reader = nobody waits)."""
    if not db_path:
        return
    fifo = db_path + WAKE_SUFFIX
    try:
        if not stat.S_ISFIFO(os.stat(fifo).st_mode):
            return
        fd = os.open(fifo, os.O_WRONLY | os.O_NONBLOCK)
    except OSError:
        return
    os.close(fd)


def _db_path(db):
    try:
        row = db.execute("PRAGMA database_list").fetchone()
    except sqlite3.Error:
        return ""
    return (row[2] if row else "") or ""


def run_lanes(argv, env=None, cwd=None, emit=None):
    """`pipeline.py lanes <verb> ...` in-process: returns (code, stdout, stderr).

    `take --wait` takes again when the pool's wake file says a lane may be
    free (see `_wake_open`), until the take ends otherwise than queued (exit
    3) -- 1:1 the words, exits, queue and TTL of `strand.sh token take --wait`,
    without a retry clock (R-AG-1): each round carries one watchdog of
    `MECLAW_TOKEN_WAIT_SECS` (default 30 s) for the causes without an event.
    The queue line is said once; the words of the round that ends the wait are
    said too. `emit(out, err)` hands the first round's output out at once (the
    CLI), else it is part of the return value."""
    env = dict(os.environ if env is None else env)
    cwd = cwd or os.getcwd()
    argv = list(argv)
    if argv[:1] == ["release-run"]:
        return _release_run_cli(argv[1:], env, cwd)
    if argv[:1] == ["fingerprint"]:
        return _fingerprint_cli(argv[1:], env)
    if argv[:1] == ["take"] and "--wait" in argv[1:]:
        rest = argv     # `--wait` stays: the queue line says that this process blocks
        try:
            secs = wait_secs(env, "MECLAW_TOKEN_WAIT_SECS", "30")
        except ScheduleError as exc:
            return exc.code, "", "lanes: %s\n" % exc
        db_path = _paths(env)[0]
        outs, first_err, tries = [], "", 0
        while True:
            fd = _wake_open(db_path)
            try:
                code, out, err = _run_once(rest, env, cwd)
                tries += 1
                if tries == 1:
                    if emit:
                        emit(out, err)
                    else:
                        outs.append(out)
                        first_err = err
                else:
                    outs.append(out)
                if code != 3:
                    return code, "".join(outs), first_err + (err if tries > 1 else "")
                # block until a verb wakes the pool, or the round's watchdog fires
                select.select([fd] if fd is not None else [], [], [], secs)
            finally:
                if fd is not None:
                    os.close(fd)
    return _run_once(argv, env, cwd)


def _run_once(argv, env, cwd):
    """One verb; an old token file goes into an unarmed store first (Z11).

    Every verb but init/status migrates (OR-DT.Z11.1, Z15 M3): `who` arms the
    store as a side effect of reading it, and a file that cannot be carried
    over makes `check` exit 2 too -- test-tier then refuses local cargo. Both
    are the fail-closed side on purpose: an armed file nobody can read is no
    reason to build unserialised."""
    pre = ""
    if argv and argv[0] in TOKEN_VERBS and argv[0] not in ("init", "status"):
        code, _, pre = _migrate_old(env, cwd)
        if code:
            return code, "", pre
    code, out, err = _run_verb(argv, env, cwd)
    return code, out, pre + err


def _run_verb(argv, env, cwd):
    ctx = _Ctx(None, 0)
    verb = argv[0] if argv else ""
    if verb not in TOKEN_VERBS:
        ctx.say("token: expected %s" % "|".join(TOKEN_VERBS))
        return 2, "", _join(ctx.err)
    if verb == "status":
        if len(argv) > 2:
            ctx.say("lanes status: at most one lane name")
            return 2, "", _join(ctx.err)
        return _status(env, argv[1] if len(argv) > 1 else None)
    opts = {"strand": "", "wave": "", "max": "3", "ttl": None, "pid": None, "force": False,
            "lanes": "", "lane": "", "wait": False, "holder": None}
    max_set = False
    args = argv[1:]
    i = 0
    while i < len(args):
        a = args[i]
        if a in ("--strand", "--wave", "--max", "--lanes", "--lane", "--ttl", "--pid", "--holder"):
            if i + 1 >= len(args):
                ctx.say("%s needs a value" % a)
                return 2, "", _join(ctx.err)
            opts[a[2:]] = args[i + 1]
            max_set = max_set or a == "--max"
            i += 2
        elif a == "--force":
            opts["force"] = True
            i += 1
        elif a == "--wait" and verb == "take":
            opts["wait"] = True
            i += 1
        else:
            ctx.say("token %s: unknown argument: %s" % (verb, a))
            return 2, "", _join(ctx.err)

    db_path, lanes_file = _paths(env)
    branch = _git(cwd, "rev-parse", "--abbrev-ref", "HEAD")
    tree = _git(cwd, "rev-parse", "--show-toplevel")
    strand_in = opts["strand"]
    bad = strand_colon_error(strand_in)
    if bad:
        ctx.say("token %s: %s" % (verb, bad))
        return 2, "", _join(ctx.err)
    # The one chain `--strand` > branch (`gitenv.strand_of`, GH #1199 f).
    who = gitenv.strand_of(branch, strand_in, opts["wave"])
    key = who.key
    if strand_in and "/" not in key:
        ctx.say("token: --strand %s needs --wave, a strand branch, or the form <wave>/<name>"
                % strand_in)
        return 2, "", _join(ctx.err)
    # A second lease of one strand (GH #1146): `<strand>:<n>`, its own holder.
    if opts["holder"] is not None:     # `--holder ""` is refused, never the main lease (R2 M4)
        if not HOLDER_RE.match(opts["holder"]) or verb not in HOLDER_VERBS:
            ctx.say("token %s: --holder takes a short suffix ([A-Za-z0-9_-], up to 32) "
                    "and belongs to %s" % (verb, "|".join(HOLDER_VERBS)))
            return 2, "", _join(ctx.err)
        key = "%s:%s" % (key, opts["holder"]) if key else ""

    if verb in ("check", "lane") and (not os.path.exists(db_path) or not key):
        return 0, "", ""
    if opts["lanes"]:
        if verb != "init":
            ctx.say("token %s: --lanes belongs to init" % verb)
            return 2, "", _join(ctx.err)
        if max_set:
            ctx.say("token init: --lanes and --max exclude each other "
                    "(a lane host file entry is one token; the count is the list)")
            return 2, "", _join(ctx.err)
    if verb in ("take", "release") and not key:
        ctx.say("token %s: %s" % (verb, who.why))
        return 2, "", _join(ctx.err)

    now = int(env.get("MECLAW_STRAND_NOW") or time.time())
    if verb != "init" and not os.path.exists(db_path):
        ctx.path = db_path
        res = _unarmed(ctx, verb)
        return res.code, res.out, res.err

    db = store.open(db_path)
    try:
        if verb == "init":
            res = lease_init(db, max=opts["max"], lanes=opts["lanes"], ttl=opts["ttl"] or "90",
                             force=opts["force"], lanes_file=lanes_file, now=now, branch=branch,
                             path=db_path)
        elif verb == "off":
            res = lease_off(db, force=opts["force"], now=now, branch=branch, path=db_path)
        elif verb == "who":
            res = lease_who(db, now=now, path=db_path)
        elif verb == "offer":
            res = lease_offer(db, now=now, branch=branch, path=db_path)
        elif verb == "lane":
            res = lease_lane(db, key)
        elif verb == "check":
            caller = env.get("MECLAW_LANES_CALLER")
            res = lease_check(db, key, pid=opts["pid"], lane=opts["lane"] or None,
                              caller=int(caller) if caller and caller.isdigit() else None,
                              skip=env.get("MECLAW_STRAND_TOKEN_SKIP", ""), now=now,
                              branch=branch, path=db_path)
        elif verb == "release":
            res = lease_release(db, key, now=now, branch=branch, path=db_path,
                                force=opts["force"])
        else:
            res = lease_take(db, key, lane=opts["lane"] or None, ttl=opts["ttl"],
                             tree=tree if branch == key else "", now=now, branch=branch,
                             path=db_path, wait=opts["wait"], pid=opts["pid"])
    finally:
        db.close()
    if res.code == 0 and verb in WAKE_VERBS:
        _wake(db_path)      # after the commit: a lane may be free for a waiter
    return res.code, res.out, res.err


def _cli_lanes(args):
    def emit(out, err):
        sys.stdout.write(out)
        sys.stdout.flush()
        sys.stderr.write(err)
        sys.stderr.flush()
    code, out, err = run_lanes(args.rest, os.environ, os.getcwd(), emit=emit)
    emit(out, err)
    return code


def register(sub):
    """`pipeline.py lanes <verb> ...` -- the lease verbs of `strand.sh token` and `lanes status`."""
    p = sub.add_parser("lanes", help="cargo tokens and build lanes (leases), lane status",
                       add_help=False)
    p.add_argument("rest", nargs=argparse.REMAINDER)
    p.set_defaults(func=_cli_lanes)
