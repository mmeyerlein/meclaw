"""Event kinds of the pipeline journal (V0 section 3) and the executor stream.

Every event is one ``journal`` row ``(at, item, run, kind, event, text,
data)``: ``kind`` is Orga's journal kind, ``event`` the name below, ``text``
an English one-liner (Orga's ``note``), ``data`` the body. The executor
streams the same objects as JSONL lines (the row without ``seq``); the core
takes them over line by line with ``ingest``.

    event                  kind            required data fields
    pass_planned           status          run mode repo rev base stations reason cache_known
                                           dirty (bool; optional: lock_wait_secs=0 archive=null narrowing=null)
    run_dispatched         note            run runner tree attempt round deadline_at ws
    hygiene_checked        note|finding    run runner free_g reserved_g leftovers zombies
                                           ports_busy verdict
    lane_fingerprint       note            rustc cargo nextest node playwright python kernel
                                           vcpu ram_g target_fs target_free_g ulimit_n
                                           incremental_policy linker sccache linger
                                           pdftotext portal_venv (values may be null)
    station_started        status          run station needs cmds scope
    station_log            note            run station path (the path, never the content)
    test_result            note            run station part verdict secs
    cache_hit              note            run station part key from_run
    station_ended          status          run station verdict secs exit scope
    flake_isolated         finding         run station test count green verdict
    watchdog_fired         finding         run runner kind value threshold action
    workspace_changed      note            runner id kind path from to bytes
    pass_ended             status          run verdict secs lane_seconds critical_path_s
                                           rounds cache_hits stations
    metrics_failed         finding         run url_host error
    lease_taken|_released|_offered  note   resource holder run ttl_s
    lease_noted            note            resource holder run ttl_s (additive, OR-DT.W3:
                                           every other line of the old token log)
    merge_queued|_gated|_done|_refused|_abandoned, revert_done
                           status          merge strand base
    process_finding        finding         rule subject value threshold since
    finding_resolved       note            seq rule subject note
    night_report           status          date path runs findings
    graph_built            note            rev nodes edges secs
    coverage_built         status          rev runner secs tests files edges granularity
    satellite_pin_needed   finding         merge repo template version
    minimum_measured       status          stage runner seconds critical_path overhead_s x
    lane_power_requested   finding         lane reason eta_s
    owner_decision         status          run station decision reason
    full_pass_marked       status          wave why run scope (full_pass, #1165)
    full_pass_cleared      status          wave run scope

``dirty`` and ``scope`` are additive to V0 section 3 (OR-DT.W0.6): the receipt
needs them (``dirty`` gates the export, ``scope`` is the ``[<scope>]`` of every
GATE line), so they are required, never defaulted -- a missing ``dirty`` must
not read as a clean tree. ``OPTIONAL`` names the documented options and the
value the receipt takes without them. Other optional fields of the contract
(``build_s``, ``tests``, ``failed_names`` ...) and additive ones pass through
untouched. A stream that ends without
``pass_ended`` is a lost run: ``ingest_stream`` marks every ``task_runs`` row
of the core's dispatch list that is still ``dispatched`` as ``lost`` (lanes
know the run and its task_runs, never the items).
"""

import json
import os
from collections import namedtuple

from . import store as _store

Spec = namedtuple("Spec", "kinds required")

_FINGERPRINT = ("rustc", "cargo", "nextest", "node", "playwright", "python", "kernel",
                "vcpu", "ram_g", "target_fs", "target_free_g", "ulimit_n",
                "incremental_policy", "linker", "sccache", "linger", "pdftotext",
                "portal_venv")
_LEASE = ("resource", "holder", "run", "ttl_s")
_MERGE = ("merge", "strand", "base")

EVENTS = {
    "pass_planned": Spec(("status",), ("run", "mode", "repo", "rev", "base", "stations",
                                       "reason", "cache_known", "dirty")),
    "run_dispatched": Spec(("note",), ("run", "runner", "tree", "attempt", "round",
                                       "deadline_at", "ws")),
    "hygiene_checked": Spec(("note", "finding"), ("run", "runner", "free_g", "reserved_g",
                                                  "leftovers", "zombies", "ports_busy",
                                                  "verdict")),
    "lane_fingerprint": Spec(("note",), _FINGERPRINT),
    "station_started": Spec(("status",), ("run", "station", "needs", "cmds", "scope")),
    "station_log": Spec(("note",), ("run", "station", "path")),
    "test_result": Spec(("note",), ("run", "station", "part", "verdict", "secs")),
    "cache_hit": Spec(("note",), ("run", "station", "part", "key", "from_run")),
    "station_ended": Spec(("status",), ("run", "station", "verdict", "secs", "exit",
                                        "scope")),
    "flake_isolated": Spec(("finding",), ("run", "station", "test", "count", "green",
                                          "verdict")),
    "watchdog_fired": Spec(("finding",), ("run", "runner", "kind", "value", "threshold",
                                          "action")),
    "workspace_changed": Spec(("note",), ("runner", "id", "kind", "path", "from", "to",
                                          "bytes")),
    "pass_ended": Spec(("status",), ("run", "verdict", "secs", "lane_seconds",
                                     "critical_path_s", "rounds", "cache_hits", "stations")),
    "metrics_failed": Spec(("finding",), ("run", "url_host", "error")),
    # `pipeline wave start`: the master rev a wave starts from; an integration
    # plans its tests from `<start>..HEAD` (welle-tb S1 part C, KR-TB-1).
    "wave_started": Spec(("status",), ("wave", "start")),
    "lease_taken": Spec(("note",), _LEASE),
    "lease_released": Spec(("note",), _LEASE),
    "lease_offered": Spec(("note",), _LEASE),
    "lease_noted": Spec(("note",), _LEASE),
    # `lanes release-run`: an orphaned run closed by hand (Z-I, #1126).
    "run_closed": Spec(("note",), ("run", "runner", "reason", "by")),
    "merge_queued": Spec(("status",), _MERGE),
    "merge_gated": Spec(("status",), _MERGE),
    "merge_done": Spec(("status",), _MERGE),
    "merge_refused": Spec(("status",), _MERGE),
    "merge_abandoned": Spec(("status",), _MERGE),
    "revert_done": Spec(("status",), _MERGE),
    "process_finding": Spec(("finding",), ("rule", "subject", "value", "threshold",
                                           "since")),
    "finding_resolved": Spec(("note",), ("seq", "rule", "subject", "note")),
    "night_report": Spec(("status",), ("date", "path", "runs", "findings")),
    "graph_built": Spec(("note",), ("rev", "nodes", "edges", "secs")),
    "coverage_built": Spec(("status",), ("rev", "runner", "secs", "tests", "files", "edges",
                                         "granularity")),
    "satellite_pin_needed": Spec(("finding",), ("merge", "repo", "template", "version")),
    "minimum_measured": Spec(("status",), ("stage", "runner", "seconds", "critical_path",
                                           "overhead_s", "x")),
    "lane_power_requested": Spec(("finding",), ("lane", "reason", "eta_s")),
    "owner_decision": Spec(("status",), ("run", "station", "decision", "reason")),
    # A narrowed pass with odd effects marks its wave, a green full pass
    # clears it -- the newest of the two is the state (welle-tb #1165).
    "full_pass_marked": Spec(("status",), ("wave", "why", "run", "scope")),
    "full_pass_cleared": Spec(("status",), ("wave", "run", "scope")),
}

# Documented optional fields and the value a reader takes without them.
OPTIONAL = {
    # `testing_ruling`: the value of MECLAW_TESTING_RULES_RULING the pass ran
    # under (P1c § 4 "Wert im Receipt", R-TV-W21 c), null without one.
    "pass_planned": {"lock_wait_secs": 0, "archive": None, "narrowing": None,
                     "testing_ruling": None},
}

# Fields that must be a JSON boolean (not 0/1, not null).
BOOLEANS = {("pass_planned", "dirty")}

# Closed vocabularies a field of an event must come from.
VOCAB = {
    ("station_ended", "verdict"): _store.VERDICTS,
    ("test_result", "verdict"): _store.VERDICTS,
    ("flake_isolated", "verdict"): ("FLAKY", "RED"),
    ("pass_ended", "verdict"): ("GREEN", "RED", "ASK", "ABORTED"),
    ("watchdog_fired", "kind"): ("disk", "process", "deadline"),
    # the width of the pass that set or cleared a full-pass mark (re-check S1b M1)
    ("full_pass_marked", "scope"): ("strand", "integration"),
    ("full_pass_cleared", "scope"): ("strand", "integration"),
    ("watchdog_fired", "action"): ("aborted", "killed"),
    ("coverage_built", "granularity"): ("module", "binary"),
}

# Called after every journal write as hook(db, row); a hook never fails a run
# (W0b hangs the metrics push in here).
HOOKS = []


class EventError(ValueError):
    """An event the journal must not take: unknown, wrong kind, fields missing."""


def validate(event, kind, data):
    spec = EVENTS.get(event)
    if spec is None:
        raise EventError(f"unknown event {event!r}")
    if kind not in spec.kinds:
        raise EventError(f"{event}: kind {kind!r} is not one of {spec.kinds}")
    if not isinstance(data, dict):
        raise EventError(f"{event}: data must be an object")
    missing = [f for f in spec.required if f not in data]
    if missing:
        raise EventError(f"{event}: missing {', '.join(missing)}")
    for name, field in BOOLEANS:
        if name == event and not isinstance(data[field], bool):
            raise EventError(f"{event}: {field} {data[field]!r} is not a boolean")
    for (name, field), allowed in VOCAB.items():
        if name == event and data.get(field) not in allowed:
            raise EventError(f"{event}: {field} {data.get(field)!r} is not one of {allowed}")


def summary_text(event, data):
    """The English one-liner of a journal row (Orga's note text)."""
    parts = [event]
    for field in EVENTS[event].required:
        value = data.get(field)
        if isinstance(value, (str, int, float)) and not isinstance(value, bool):
            parts.append(f"{field}={value}")
    return " ".join(parts)[:200]


# The `process_finding` rule of a pass the store lost a record of: the pass
# ended ABORTED although its stream's `pass_ended` says otherwise, and every
# reader of a pass verdict (`receipt.project`, `retro.passes`) reads it so
# (re-check tb-s3-store M-2, #1161; probe P4: journal and receipt GREEN).
STORE_LOST_RULE = "store_record_lost"

# The finding is best effort and goes with the store when the store is still
# locked at the end of the pass (re-check tb-s3-store2 m-n3). The gate then
# appends one line to this ledger -- in the archive root of the pass, else
# beside the store -- and every reader of a pass verdict reads it as the
# finding's fallback. Append-only, like the journal.
STORE_LOST_LEDGER = "store-lost.jsonl"


def store_file(db):
    """The file behind a store connection, or None (in-memory)."""
    for row in db.execute("PRAGMA database_list"):
        if row[1] == "main" and row[2]:
            return row[2]
    return None


def lost_ledgers(archives=(), db=None):
    """The ledger files of `archives` (archive roots) and the one beside `db`'s store."""
    out = [os.path.join(a, STORE_LOST_LEDGER) for a in archives if a]
    path = store_file(db) if db is not None else None
    if path:
        out.append(os.path.join(os.path.dirname(path), STORE_LOST_LEDGER))
    return list(dict.fromkeys(out))


def ledger_runs(paths):
    """The runs the ledger files name (a missing file or a broken line names none)."""
    out = set()
    for path in paths:
        try:
            with open(path, encoding="utf-8") as fh:
                lines = fh.readlines()
        except OSError:
            continue
        for line in lines:
            try:
                d = json.loads(line)
            except ValueError:
                continue
            if isinstance(d, dict) and d.get("rule") == STORE_LOST_RULE and d.get("run"):
                out.add(d["run"])
    return out


def write_ledger(path, data):
    """Append one ledger line (`data` carries `rule` and `run`), synced to disk."""
    os.makedirs(os.path.dirname(path) or ".", exist_ok=True)
    with open(path, "a", encoding="utf-8") as fh:
        fh.write(json.dumps(data, sort_keys=True) + "\n")
        fh.flush()
        os.fsync(fh.fileno())


def store_lost_runs(db, runs):
    """The runs whose pass the store lost a record of (re-check tb-s3-store M-2,
    m-n3): a `store_record_lost` finding in the journal, else a line in a ledger
    -- the archive root of the run's `pass_planned` or the one beside the store."""
    out, runs = set(), list(dict.fromkeys(r for r in runs if r))
    archives = set()
    for i in range(0, len(runs), 500):
        chunk = runs[i:i + 500]
        marks = ",".join("?" * len(chunk))
        out.update(r[0] for r in db.execute(
            "SELECT run FROM journal WHERE event='process_finding'"
            f" AND json_extract(data, '$.rule')=? AND run IN ({marks})",
            [STORE_LOST_RULE] + chunk))
        archives.update(r[0] for r in db.execute(
            "SELECT json_extract(data, '$.archive') FROM journal WHERE event='pass_planned'"
            f" AND run IN ({marks})", chunk) if r[0])
    rest = set(runs) - out
    if rest:
        out |= rest & ledger_runs(lost_ledgers(sorted(archives), db))
    return out


def emit(db, event, kind, data, run=None, item=None, at=None, text=None):
    """Validate and journal one event; returns its `seq`."""
    validate(event, kind, data)
    at = at or _store.now()
    text = text or summary_text(event, data)
    seq = _store.journal_add(db, at, item, run, kind, event, text, data)
    row = {"seq": seq, "at": at, "item": item, "run": run, "kind": kind,
           "event": event, "text": text, "data": data}
    for hook in HOOKS:
        try:
            hook(db, row)
        except Exception:  # noqa: BLE001 -- a hook never fails the run
            pass
    return seq


def read_run(db, run):
    """The journal rows of one run, in `seq` order, `data` parsed."""
    rows = []
    for r in db.execute("SELECT * FROM journal WHERE run=? ORDER BY seq", (run,)):
        row = dict(r)
        row["data"] = json.loads(row["data"]) if row["data"] else {}
        rows.append(row)
    return rows


def to_line(row):
    """A journal row as a stream line: the same object without `seq`."""
    out = {k: row[k] for k in ("at", "run", "item", "kind", "event", "text", "data")
           if row.get(k) is not None}
    return out


def write_jsonl(fh, rows):
    for row in rows:
        fh.write(json.dumps(to_line(row), sort_keys=True) + "\n")


def read_jsonl(fh):
    for raw in fh:
        raw = raw.strip()
        if raw:
            yield json.loads(raw)


def ingest(db, line):
    """Take one stream line (str or dict) into the journal; returns its `seq`."""
    obj = json.loads(line) if isinstance(line, str) else line
    for field in ("at", "kind", "event", "data"):
        if field not in obj:
            raise EventError(f"stream line without {field}")
    return emit(db, obj["event"], obj["kind"], obj["data"], run=obj.get("run"),
                item=obj.get("item"), at=obj["at"], text=obj.get("text"))


def ingest_stream(db, lines, run, task_runs=()):
    """Take a whole executor stream of `run`; mark it lost without `pass_ended`.

    `task_runs` is the dispatch list of the core (the `tr-...` ids it opened
    for this run). Without `pass_ended` every one of them still `dispatched`
    is marked `lost` -- also stations that never started and so never named
    themselves in the stream.
    """
    count, ended = 0, False
    for raw in lines:
        obj = json.loads(raw) if isinstance(raw, str) else raw
        if obj.get("run") != run:
            raise EventError(f"stream line of run {obj.get('run')!r} in the stream of {run}")
        ingest(db, obj)
        count += 1
        ended = ended or obj.get("event") == "pass_ended"
    marked = []
    ids = list(dict.fromkeys(task_runs))
    if not ended and ids:
        marks = ",".join("?" * len(ids))
        with _store.tx(db):
            marked = [r[0] for r in db.execute(
                f"SELECT id FROM task_runs WHERE state='dispatched' AND id IN ({marks})"
                " ORDER BY id", ids)]
            db.execute(
                f"UPDATE task_runs SET lost=1 WHERE state='dispatched' AND id IN ({marks})",
                ids)
    return {"lines": count, "lost": not ended, "marked": marked}
