"""The pipeline store: one SQLite file per coordinator machine, Orga's names.

Path: ``$MECLAW_PIPELINE_DB`` or ``~/.local/state/meclaw/pipeline/pipeline.db``
(WAL, ``busy_timeout`` 5 s; a write refused as locked is retried a bounded
number of times by ``retry_write``, then fails loudly). Lanes never write
into it. Several core versions (one per worktree) share it, so migrations
are additive only: a core reads and writes only the columns it knows, and
never refuses a newer schema.

OR-DT.store-migrations: ``migration_hash`` records number and sha256 of every
applied migration. ``open`` refuses loudly (``MigrationDrift``) when a known
number carries another hash or the applied numbers have a gap -- it never
skips a migration silently. A store whose migrations predate the hashes gets
them recorded once, without a refusal. The strand that adds a migration names
its number in ``plans/<wave>/straenge/<S>.json`` (``"migrations": [n]``); the
merge queue refuses two open strands with the same number.

Tables and columns follow Orga 0.11.14 word for word (``items``, ``edges``,
``counters``, ``task_runs``, ``task_run_results``, ``calibration``,
``journal``) plus the pipeline's own (``runners``, ``cache``, ``flakes``,
``workspaces``, ``leases``, ``merges``, ``minimum``, ``schema_version``).

State machines (Orga 0.11.14, README section Types; locked by a test)::

    type         from                       to
    workpackage  planned                    active
                 active                     finished | stopped
                 finished | stopped         active          (reopen = next round)
    task         open                       in_progress | done | dropped
                 in_progress                waiting | done | dropped
                 waiting                    in_progress | done | dropped
                 done                       open | dropped  (reopen)
                 dropped                    open            (reopen)
    task_runs    dispatched                 done | failed | timed_out

Verdict -> task_runs state: GREEN/CACHED/NOTE/SKIP (and a legacy FLAKY) -> done;
FLAKY -> failed `flaky` (OR-TV-2); RED ->
failed (red); ASK -> failed (ask); ABORTED -> failed with one of
``aborted_disk|aborted_process|aborted_deadline|hygiene``; the station's
deadline -> timed_out. The task follows with ``TASK_AFTER_RUN`` (a failed run
leaves its task ``waiting``, as in Orga).

Every write is one transaction. ``items.version`` counts every change and is
compare-and-set: a writer names the version it read and is refused with
``Conflict`` when somebody else wrote first. The ``journal`` is append-only:
triggers refuse UPDATE and DELETE.
"""

import hashlib
import json
import os
import pathlib
import sqlite3
import sys
import time
import uuid
from contextlib import contextmanager
from datetime import datetime, timedelta, timezone

SCHEMA_VERSION = "meclaw.pipeline.store/4"
SCHEMA_PREFIX = "meclaw.pipeline.store/"
BUSY_TIMEOUT_MS = 5000
# welle-tb S3 (#1161): a write the store refuses as locked/busy is tried this
# often (each try waits up to BUSY_TIMEOUT_MS on a held lock), with a growing
# pause between the tries; then it fails loudly as StoreWriteError.
WRITE_ATTEMPTS = 3
WRITE_BACKOFF_S = 0.25

TASK_RUN_STATES = ("dispatched", "done", "failed", "timed_out")
TRANSITIONS = {
    "workpackage": {
        "planned": {"active"},
        "active": {"finished", "stopped"},
        "finished": {"active"},
        "stopped": {"active"},
    },
    "task": {
        "open": {"in_progress", "done", "dropped"},
        "in_progress": {"waiting", "done", "dropped"},
        "waiting": {"in_progress", "done", "dropped"},
        "done": {"open", "dropped"},
        "dropped": {"open"},
    },
}
INITIAL_STATE = {"workpackage": "planned", "task": "open"}
KEY_PREFIX = {"workpackage": "P", "task": "T"}
TASK_AFTER_RUN = {"done": "done", "failed": "waiting", "timed_out": "waiting"}

VERDICTS = ("GREEN", "RED", "CACHED", "FLAKY", "SKIP", "NOTE", "ASK", "ABORTED")
DONE_VERDICTS = ("GREEN", "CACHED", "NOTE", "SKIP")
ABORT_REASONS = ("aborted_disk", "aborted_process", "aborted_deadline", "hygiene")
JOURNAL_KINDS = ("note", "finding", "status", "chain")
EDGE_KINDS = ("depends_on", "blocks", "belongs_to")
CACHE_VERDICTS = ("GREEN",)   # a FLAKY is a finding, never cached (OR-TV-2)
WORKSPACE_KINDS = ("tree", "target", "ws", "bench", "pool", "export", "kit")
WORKSPACE_STATES = ("created", "in_use", "warm", "closed", "collected", "evicted")
# The timestamp column a workspace state stamps when it is set.
WORKSPACE_STAMP = {"created": "created_at", "in_use": "used_at",
                   "closed": "closed_at", "collected": "collected_at"}


class Conflict(Exception):
    """A compare-and-set lost: the row moved since the writer read it."""


class MigrationDrift(Exception):
    """The store's migrations disagree with this core's (OR-DT.store-migrations)."""


class StoreWriteError(Exception):
    """A write the store refused WRITE_ATTEMPTS times (locked or busy)."""


def _busy(exc):
    text = str(exc).lower()
    return isinstance(exc, sqlite3.OperationalError) and ("locked" in text or "busy" in text)


def retry_write(fn, *args, sleep=None, attempts=None, **kwargs):
    """`fn(*args, **kwargs)`, retried while the store answers locked/busy.

    Each try already waits `busy_timeout` on a held write lock; the retry
    covers what that wait never does -- a `BEGIN IMMEDIATE` on a stale WAL
    snapshot fails at once (`database is locked`). The pause runs outside
    the `except`, so the failed try's frames (and an open cursor in them)
    are gone before the next one. After WRITE_ATTEMPTS a StoreWriteError
    names the last refusal; any other error passes through untouched.
    `fn` must be safe to repeat: one transaction, or idempotent.
    `attempts` (default WRITE_ATTEMPTS) lowers the bound for a caller that
    already knows the store is refusing (re-check tb-s3-store m-3)."""
    sleep = time.sleep if sleep is None else sleep
    attempts = WRITE_ATTEMPTS if attempts is None else max(1, int(attempts))
    last = None
    for attempt in range(1, attempts + 1):
        if attempt > 1:
            sleep(WRITE_BACKOFF_S * (attempt - 1))
        try:
            return fn(*args, **kwargs)
        except sqlite3.OperationalError as exc:
            if not _busy(exc):
                raise
            last = str(exc)
    raise StoreWriteError("%s (%d attempts)" % (last, attempts))


# Migration n creates schema `meclaw.pipeline.store/<n>`. Additive only: a
# later entry may add tables, columns and indexes, never drop or rename one.
MIGRATIONS = [
    (1, [
        """CREATE TABLE IF NOT EXISTS items (
            id TEXT PRIMARY KEY, key TEXT UNIQUE, type TEXT NOT NULL, title TEXT,
            state TEXT NOT NULL, reason TEXT, parent TEXT, doc TEXT,
            version INTEGER NOT NULL DEFAULT 1, created_at TEXT, updated_at TEXT)""",
        """CREATE TABLE IF NOT EXISTS edges (
            src TEXT NOT NULL, dst TEXT NOT NULL, kind TEXT NOT NULL, at TEXT,
            actor TEXT, data TEXT, PRIMARY KEY (src, dst, kind))""",
        """CREATE TABLE IF NOT EXISTS counters (
            name TEXT PRIMARY KEY, value INTEGER NOT NULL)""",
        """CREATE TABLE IF NOT EXISTS task_runs (
            id TEXT PRIMARY KEY, item TEXT, key TEXT, runner TEXT, attempt INTEGER,
            ws TEXT, state TEXT NOT NULL, started_at TEXT, deadline_at TEXT,
            ended_at TEXT, reason TEXT, evidence TEXT, results TEXT,
            lost INTEGER NOT NULL DEFAULT 0, round INTEGER, verdict TEXT,
            build_s REAL, run_s REAL, cache_from TEXT, fingerprint TEXT)""",
        """CREATE TABLE IF NOT EXISTS task_run_results (
            run TEXT NOT NULL, at TEXT, call TEXT NOT NULL, tool_name TEXT,
            entry TEXT, PRIMARY KEY (run, call))""",
        """CREATE TABLE IF NOT EXISTS runners (
            name TEXT PRIMARY KEY, party TEXT, capacity TEXT, host TEXT,
            transport TEXT, class TEXT, state TEXT, boot_s REAL, fingerprint TEXT,
            updated_at TEXT)""",
        """CREATE TABLE IF NOT EXISTS calibration (
            item TEXT, type TEXT, executing TEXT, estimate_min REAL,
            actual_min REAL, actual_s REAL, closed_at TEXT NOT NULL,
            key TEXT NOT NULL, estimate_s REAL, PRIMARY KEY (key, closed_at))""",
        """CREATE TABLE IF NOT EXISTS journal (
            seq INTEGER PRIMARY KEY AUTOINCREMENT, at TEXT NOT NULL, item TEXT,
            run TEXT, kind TEXT NOT NULL, event TEXT NOT NULL, text TEXT, data TEXT)""",
        "CREATE INDEX IF NOT EXISTS journal_run ON journal (run, seq)",
        """CREATE TRIGGER IF NOT EXISTS journal_no_update BEFORE UPDATE ON journal
            BEGIN SELECT RAISE(ABORT, 'journal is append-only'); END""",
        """CREATE TRIGGER IF NOT EXISTS journal_no_delete BEFORE DELETE ON journal
            BEGIN SELECT RAISE(ABORT, 'journal is append-only'); END""",
        """CREATE TABLE IF NOT EXISTS cache (
            key TEXT PRIMARY KEY, station TEXT, part TEXT, verdict TEXT, run TEXT,
            runner TEXT, at TEXT, inputs TEXT)""",
        """CREATE TABLE IF NOT EXISTS flakes (
            test TEXT PRIMARY KEY, station TEXT, repo TEXT, first_at TEXT,
            last_at TEXT, runs INTEGER, reds INTEGER, flaky INTEGER,
            quarantined_until TEXT, issue TEXT)""",
        """CREATE TABLE IF NOT EXISTS workspaces (
            id TEXT PRIMARY KEY, runner TEXT, kind TEXT, path TEXT, state TEXT,
            run TEXT, bytes INTEGER, created_at TEXT, used_at TEXT, closed_at TEXT,
            collected_at TEXT, ttl_s INTEGER)""",
        """CREATE TABLE IF NOT EXISTS leases (
            resource TEXT PRIMARY KEY, holder TEXT NOT NULL, run TEXT,
            taken_at TEXT, ttl_s INTEGER, offered_to TEXT)""",
        """CREATE TABLE IF NOT EXISTS merges (
            id TEXT PRIMARY KEY, repo TEXT, strand TEXT, base TEXT, merge_sha TEXT,
            state TEXT, versions TEXT, pins TEXT, run TEXT, at TEXT)""",
        """CREATE TABLE IF NOT EXISTS minimum (
            stage TEXT NOT NULL, runner TEXT NOT NULL, repo TEXT NOT NULL,
            seconds REAL, critical_path TEXT, overhead_s REAL, x REAL,
            measured_at TEXT NOT NULL, source TEXT,
            PRIMARY KEY (stage, runner, repo, measured_at))""",
    ]),
    # 2 (#1135): the flake register names where a hand-registered flake was seen.
    (2, ["ALTER TABLE flakes ADD COLUMN evidence TEXT"]),
    # 3 (#1136): the run-scoped drift rule reads one station's tail newest-first.
    (3, ["CREATE INDEX IF NOT EXISTS journal_station ON journal"
         " (json_extract(data, '$.station'), seq) WHERE event = 'station_ended'"]),
    # 4 (OR-TV-2, tv-p1c): a quarantine is armed with owner and cause class, and
    # its expiry duty ends with a measured fix (VG TV-F3/TV-F6).
    (4, ["ALTER TABLE flakes ADD COLUMN owner TEXT",
         "ALTER TABLE flakes ADD COLUMN cause TEXT",
         "ALTER TABLE flakes ADD COLUMN fixed TEXT"]),
]


def now():
    """The current time, RFC 3339 UTC with a `Z` (callers in tests pass `at`)."""
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def parse_at(text):
    """RFC 3339 (`Z` or an offset) -> aware datetime."""
    return datetime.fromisoformat(text.replace("Z", "+00:00"))


def default_path():
    env = os.environ.get("MECLAW_PIPELINE_DB")
    if env:
        return pathlib.Path(env)
    return pathlib.Path.home() / ".local/state/meclaw/pipeline/pipeline.db"


def _dump(value):
    return None if value is None else json.dumps(value, sort_keys=True)


def _version_n(text):
    try:
        return int(str(text).rsplit("/", 1)[1])
    except (IndexError, ValueError):
        return 0


@contextmanager
def tx(db):
    """One write transaction (BEGIN IMMEDIATE, so a CAS reads what it writes).

    Inside an open transaction it nests as a SAVEPOINT, so a caller can make
    several writes one atomic unit (the backfill writes one run that way)."""
    if db.in_transaction:
        db.execute("SAVEPOINT nested_tx")
        try:
            yield db
        except BaseException:
            db.execute("ROLLBACK TO nested_tx")
            db.execute("RELEASE nested_tx")
            raise
        db.execute("RELEASE nested_tx")
        return
    db.execute("BEGIN IMMEDIATE")
    try:
        yield db
    except BaseException:
        db.execute("ROLLBACK")
        raise
    db.execute("COMMIT")


def migration_hash(statements):
    """sha256 of one migration's statements, in order (OR-DT.store-migrations)."""
    return hashlib.sha256("\n\0".join(statements).encode("utf-8")).hexdigest()


def _check_migrations(db, applied):
    """Refuse a gap or a known number with another hash; record missing hashes."""
    applied = {n for n in applied if n > 0}
    if applied and applied != set(range(1, max(applied) + 1)):
        missing = sorted(set(range(1, max(applied) + 1)) - applied)
        raise MigrationDrift("store migrations have a gap: applied %s, missing %s"
                             % (sorted(applied), missing))
    recorded = {r[0]: r[1] for r in db.execute("SELECT n, sha256 FROM migration_hash")}
    for n, statements in MIGRATIONS:
        if n not in applied:
            continue
        mine = migration_hash(statements)
        if n not in recorded:
            db.execute("INSERT INTO migration_hash VALUES (?, ?, ?)", (n, mine, now()))
        elif recorded[n] != mine:
            raise MigrationDrift(
                "migration %d in the store has hash %s, this core's has %s -- two "
                "strands used the same number (OR-DT.store-migrations)"
                % (n, recorded[n][:12], mine[:12]))


def open(path=None):  # noqa: A001 -- the contract names it `open` (W0 section 1)
    """Open (and create or migrate) the store; returns a sqlite3 connection."""
    path = pathlib.Path(path) if path else default_path()
    path.parent.mkdir(parents=True, exist_ok=True)
    db = sqlite3.connect(str(path), isolation_level=None, timeout=BUSY_TIMEOUT_MS / 1000)
    db.row_factory = sqlite3.Row
    db.execute(f"PRAGMA busy_timeout = {BUSY_TIMEOUT_MS}")
    db.execute("PRAGMA journal_mode = WAL")
    try:
        _migrate(db)
    except MigrationDrift:
        db.close()
        raise
    return db


def _migrate(db):
    with tx(db):
        db.execute("CREATE TABLE IF NOT EXISTS schema_version (version TEXT, applied_at TEXT)")
        db.execute("CREATE TABLE IF NOT EXISTS migration_hash ("
                   "n INTEGER PRIMARY KEY, sha256 TEXT NOT NULL, recorded_at TEXT)")
        applied = {_version_n(r[0]) for r in db.execute("SELECT version FROM schema_version")}
        _check_migrations(db, applied)
        for n, statements in MIGRATIONS:
            if n in applied:
                continue
            for sql in statements:
                db.execute(sql)
            db.execute("INSERT INTO schema_version VALUES (?, ?)",
                       (f"{SCHEMA_PREFIX}{n}", now()))
            db.execute("INSERT INTO migration_hash VALUES (?, ?, ?)",
                       (n, migration_hash(statements), now()))


def open_ro(path=None):
    """Open an existing store read-only (``mode=ro``): no create, no migration.

    For the read commands (``store check``, ``receipt``); a missing file
    raises ``FileNotFoundError`` instead of creating the shared store.
    """
    path = pathlib.Path(path) if path else default_path()
    if not path.is_file():
        raise FileNotFoundError(f"no store at {path}")
    db = sqlite3.connect(f"{path.resolve().as_uri()}?mode=ro", uri=True,
                         isolation_level=None, timeout=BUSY_TIMEOUT_MS / 1000)
    db.row_factory = sqlite3.Row
    db.execute(f"PRAGMA busy_timeout = {BUSY_TIMEOUT_MS}")
    return db


def close(db):
    db.close()


def check(db):
    """What `pipeline store check` prints: schema, tables, append-only triggers."""
    versions = [r[0] for r in db.execute("SELECT version FROM schema_version")]
    newest = max(versions, key=_version_n) if versions else None
    tables = sorted(r[0] for r in db.execute(
        "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'"))
    triggers = sorted(r[0] for r in db.execute(
        "SELECT name FROM sqlite_master WHERE type='trigger' AND tbl_name='journal'"))
    return {"schema": newest, "known": SCHEMA_VERSION, "tables": tables,
            "journal_triggers": triggers,
            "journal_rows": db.execute("SELECT count(*) FROM journal").fetchone()[0]}


# --- items ---------------------------------------------------------------

def _next_key(db, prefix):
    name = f"key:{prefix}"
    while True:
        row = db.execute("SELECT value FROM counters WHERE name=?", (name,)).fetchone()
        if row is None:
            db.execute("INSERT INTO counters VALUES (?, 1)", (name,))
            return f"{prefix}-1"
        cur = db.execute("UPDATE counters SET value=? WHERE name=? AND value=?",
                         (row[0] + 1, name, row[0]))
        if cur.rowcount == 1:
            return f"{prefix}-{row[0] + 1}"


def item_get(db, item_id):
    row = db.execute("SELECT * FROM items WHERE id=?", (item_id,)).fetchone()
    if row is None:
        return None
    out = dict(row)
    out["doc"] = json.loads(out["doc"]) if out["doc"] else None
    return out


def item_create(db, type, title, parent=None, doc=None, reason=None, at=None):  # noqa: A002
    if type not in INITIAL_STATE:
        raise ValueError(f"unknown item type {type!r}")
    at = at or now()
    item_id = f"it-{uuid.uuid4().hex[:12]}"
    with tx(db):
        key = _next_key(db, KEY_PREFIX[type])
        db.execute("INSERT INTO items VALUES (?,?,?,?,?,?,?,?,1,?,?)",
                   (item_id, key, type, title, INITIAL_STATE[type], reason, parent,
                    _dump(doc), at, at))
    return item_get(db, item_id)


def item_update(db, item_id, version, title=None, doc=None, reason=None, at=None):
    """Change title/doc/reason; compare-and-set on `version`; returns the new one."""
    sets, args = [], []
    for col, value in (("title", title), ("doc", _dump(doc)), ("reason", reason)):
        if value is not None:
            sets.append(f"{col}=?")
            args.append(value)
    with tx(db):
        cur = db.execute(
            f"UPDATE items SET {', '.join(sets + ['version=version+1', 'updated_at=?'])} "
            "WHERE id=? AND version=?", args + [at or now(), item_id, version])
        if cur.rowcount != 1:
            raise Conflict(f"{item_id}: version {version} is stale")
    return version + 1


def item_transition(db, item_id, from_state, to_state, version, reason=None, at=None):
    """Move an item along its automaton; compare-and-set on state AND version."""
    row = db.execute("SELECT type FROM items WHERE id=?", (item_id,)).fetchone()
    if row is None:
        raise KeyError(item_id)
    allowed = TRANSITIONS[row[0]].get(from_state, set())
    if to_state not in allowed:
        raise ValueError(f"{row[0]}: {from_state} -> {to_state} is not a transition")
    with tx(db):
        cur = db.execute(
            "UPDATE items SET state=?, reason=?, version=version+1, updated_at=? "
            "WHERE id=? AND state=? AND version=?",
            (to_state, reason, at or now(), item_id, from_state, version))
        if cur.rowcount != 1:
            raise Conflict(f"{item_id}: not {from_state}@{version}")
    return version + 1


def edge_link(db, src, dst, kind, actor, data=None, at=None):
    if kind not in EDGE_KINDS:
        raise ValueError(f"unknown edge kind {kind!r}")
    with tx(db):
        db.execute("INSERT OR REPLACE INTO edges VALUES (?,?,?,?,?,?)",
                   (src, dst, kind, at or now(), actor, _dump(data)))


# --- task runs -------------------------------------------------------------

def verdict_state(verdict, reason=None, legacy=False):
    """Verdict -> (task_runs.state, reason) as V0 section 2 maps it; a FLAKY
    is a finding (OR-TV-2): failed `flaky`, done only for a legacy test."""
    if verdict in DONE_VERDICTS or (verdict == "FLAKY" and legacy):
        return "done", None
    if verdict == "FLAKY":
        return "failed", "flaky"
    if verdict == "RED":
        return "failed", reason or "red"
    if verdict == "ASK":
        return "failed", reason or "ask"
    if verdict == "ABORTED":
        if reason not in ABORT_REASONS:
            raise ValueError(f"ABORTED needs one of {ABORT_REASONS}, got {reason!r}")
        return "failed", reason
    raise ValueError(f"unknown verdict {verdict!r}")


def run_open(db, item, runner, attempt=1, round=1, ws=None, deadline_at=None,  # noqa: A002
             key=None, at=None):
    run_id = f"tr-{uuid.uuid4().hex[:12]}"
    with tx(db):
        db.execute(
            "INSERT INTO task_runs (id, item, key, runner, attempt, ws, state, started_at,"
            " deadline_at, round) VALUES (?,?,?,?,?,?,'dispatched',?,?,?)",
            (run_id, item, key, runner, attempt, ws, at or now(), deadline_at, round))
    return run_id


def run_close(db, run_id, verdict, reason=None, timed_out=False, build_s=None,
              run_s=None, cache_from=None, fingerprint=None, evidence=None,
              results=None, legacy=False, at=None):
    """Close a dispatched run by its verdict; returns the task_runs state.

    Only the ``task_runs`` row changes. The task does not follow by itself:
    the caller sets it done/waiting with ``item_transition`` (the state from
    ``TASK_AFTER_RUN[state]``), in its own compare-and-set write (W3).
    """
    if timed_out:
        state, reason = "timed_out", reason or "deadline"
    else:
        state, reason = verdict_state(verdict, reason, legacy=legacy)
    with tx(db):
        cur = db.execute(
            "UPDATE task_runs SET state=?, reason=?, verdict=?, ended_at=?, build_s=?,"
            " run_s=?, cache_from=?, fingerprint=?, evidence=?, results=?"
            " WHERE id=? AND state='dispatched'",
            (state, reason, verdict, at or now(), build_s, run_s, cache_from,
             _dump(fingerprint), _dump(evidence), _dump(results), run_id))
        if cur.rowcount != 1:
            raise Conflict(f"{run_id}: not dispatched")
    return state


def result_append(db, run, call, tool_name, entry, at=None):
    with tx(db):
        db.execute("INSERT INTO task_run_results VALUES (?,?,?,?,?)",
                   (run, at or now(), call, tool_name, _dump(entry)))


def journal_add(db, at, item, run, kind, event, text, data):
    if kind not in JOURNAL_KINDS:
        raise ValueError(f"unknown journal kind {kind!r}")
    with tx(db):
        cur = db.execute(
            "INSERT INTO journal (at, item, run, kind, event, text, data)"
            " VALUES (?,?,?,?,?,?,?)", (at, item, run, kind, event, text, _dump(data)))
    return cur.lastrowid


# --- leases, cache, workspaces ---------------------------------------------

def lease_who(db, resource):
    row = db.execute("SELECT * FROM leases WHERE resource=?", (resource,)).fetchone()
    return dict(row) if row else None


def _expired(row, at):
    if not row["ttl_s"] or not row["taken_at"]:
        return False
    return parse_at(row["taken_at"]) + timedelta(seconds=row["ttl_s"]) <= parse_at(at)


def lease_take(db, resource, holder, run=None, ttl_s=None, at=None):
    """Take a lease; True when held now. An expired lease is taken over."""
    at = at or now()
    with tx(db):
        row = db.execute("SELECT * FROM leases WHERE resource=?", (resource,)).fetchone()
        if row is not None and row["holder"] != holder and not _expired(row, at):
            return False
        db.execute("INSERT OR REPLACE INTO leases VALUES (?,?,?,?,?,NULL)",
                   (resource, holder, run, at, ttl_s))
    return True


def lease_release(db, resource, holder):
    with tx(db):
        cur = db.execute("DELETE FROM leases WHERE resource=? AND holder=?",
                         (resource, holder))
    return cur.rowcount == 1


def cache_put(db, key, station, part, verdict, run, runner, inputs, at=None):
    if verdict not in CACHE_VERDICTS:
        raise ValueError(f"only {CACHE_VERDICTS} are cached, not {verdict!r}")
    with tx(db):
        db.execute("INSERT OR REPLACE INTO cache VALUES (?,?,?,?,?,?,?,?)",
                   (key, station, part, verdict, run, runner, at or now(), _dump(inputs)))


def cache_get(db, key):
    row = db.execute("SELECT * FROM cache WHERE key=?", (key,)).fetchone()
    if row is None:
        return None
    out = dict(row)
    out["inputs"] = json.loads(out["inputs"]) if out["inputs"] else None
    return out


def runner_set_state(db, name, state, at=None):
    """Set `runners.state` of lane `name` (GH #1129); a lane the schedule has
    not registered yet gets a bare row (`sync_runners` fills the rest)."""
    at = at or now()
    with tx(db):
        cur = db.execute("UPDATE runners SET state=?, updated_at=? WHERE name=?", (state, at, name))
        if cur.rowcount == 0:
            db.execute("INSERT INTO runners (name, party, state, updated_at) VALUES (?,?,?,?)",
                       (name, "lane:%s" % name, state, at))


def runner_state(db, name):
    row = db.execute("SELECT state FROM runners WHERE name=?", (name,)).fetchone()
    return row["state"] if row else None


def workspace_set(db, ws_id, runner, kind, path, state, run=None, bytes=None,  # noqa: A002
                  ttl_s=None, at=None):
    """Create or move a workspace row (transitions are W2's `hygiene.py`)."""
    if kind not in WORKSPACE_KINDS:
        raise ValueError(f"unknown workspace kind {kind!r}")
    if state not in WORKSPACE_STATES:
        raise ValueError(f"unknown workspace state {state!r}")
    at = at or now()
    with tx(db):
        db.execute(
            "INSERT INTO workspaces (id, runner, kind, path, state, run, bytes, ttl_s)"
            " VALUES (?,?,?,?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET runner=excluded.runner,"
            " kind=excluded.kind, path=excluded.path, state=excluded.state,"
            " run=COALESCE(excluded.run, run), bytes=COALESCE(excluded.bytes, bytes),"
            " ttl_s=COALESCE(excluded.ttl_s, ttl_s)",
            (ws_id, runner, kind, path, state, run, bytes, ttl_s))
        col = WORKSPACE_STAMP.get(state)
        if col:
            db.execute(f"UPDATE workspaces SET {col}=? WHERE id=?", (at, ws_id))


# --- CLI -------------------------------------------------------------------

def _cmd_store(args):
    if args.action == "check":
        try:
            db = open_ro(args.db)
        except FileNotFoundError as exc:
            print(f"store: {exc}", file=sys.stderr)
            return 1
    else:
        db = open(args.db)
    try:
        if args.action == "check":
            print(json.dumps(check(db), indent=2))
        else:
            print(f"store {default_path() if not args.db else args.db}: {check(db)['schema']}")
    finally:
        close(db)
    return 0


def register(sub):
    """`pipeline store init|check` (V0 section 9)."""
    p = sub.add_parser("store", help="create (init) or read-only check the pipeline store")
    p.add_argument("action", choices=("init", "check"))
    p.add_argument("--db", default=None, help="store path (default $MECLAW_PIPELINE_DB)")
    p.set_defaults(func=_cmd_store)

