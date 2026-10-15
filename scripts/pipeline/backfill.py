"""Backfill: the archived gates as finished runs (W0, V0 sections 2, 3, 8).

Reads ONLY `<root>/plans/*/receipts/**` -- the archive; never `target/gate/`
or worktrees, which hold copies of the same runs. One run is

- a v1 receipt `<mode>-<sha>.json` (`last-<mode>.json` is a copy and is
  skipped), its pass seconds from the `GATE-SUMMARY` line of the `summary.txt`
  next to it (else finished - started), station seconds ONLY from
  `stations[].secs`, never from logs; or
- a `summary.txt` without a receipt next to it (the satellite form of
  soul2me/coding/orga): line 1 the summary line, whose second word is the repo
  (or the mode of a meclaw run), line 2 `branch <b> set <v>` kept as `data`,
  then `<station> GREEN|RED <s>s`. Its start is the directory name
  (`YYYYMMDD-HHMMSS-<sha>`, local time).

A run is deduplicated over `(repo, rev, mode, started)`: its id `run-<12hex>`
is a hash of that key, so a run already in the store is known, not written
twice. It is written as the events a live run emits (`pass_planned`,
`run_dispatched`, one `station_ended` per station, `pass_ended`) with the
original times, plus a workpackage, a task per station and a closed
`task_runs` row in round 1. The runner is the local host for a run whose
paths lie on this machine, the lane named by the token log for a run on
`/srv/target`, else `unknown`.

Result lines: `BACKFILL repo=<r> runs=<n> lane_hours=<h> red=<n>`.
"""

import hashlib
import json
import os
import pathlib
import re
import subprocess
import sys
from datetime import datetime, timedelta, timezone
from zoneinfo import ZoneInfo

from . import events as _events
from . import metrics as _metrics
from . import store as _store

MECLAW_MODES = ("strand", "integration", "release", "ci")
RECEIPT = re.compile(r"(?P<mode>[a-z]+)-(?P<rev>[0-9a-f]{7,40})\.json")
SUMMARY = re.compile(r"GATE-SUMMARY (?P<a>\S+) (?P<rev>\S+) \d+/\d+ (?P<secs>\d+)s (?P<v>[A-Z]+)")
STATION = re.compile(r"(?P<name>\S+)\s+(?P<v>[A-Z]+)\s+(?P<secs>\d+)s\s*$")
DIR_UTC = re.compile(r"(\d{8})T(\d{6})Z")
DIR_LOCAL = re.compile(r"(\d{8})-(\d{6})")
ARCHIVE_TZ = ZoneInfo("Europe/Berlin")  # the zone the satellite gates stamp their runs in
LANE_ROOT = "/srv/target"


def _iso(dt):
    return dt.astimezone(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def _dir_start(name, fallback):
    m = DIR_UTC.search(name)
    if m:
        return _iso(datetime.strptime("".join(m.groups()), "%Y%m%d%H%M%S")
                    .replace(tzinfo=timezone.utc))
    m = DIR_LOCAL.search(name)
    if m:
        # local time of the archive, read in a fixed zone: the TZ of the machine
        # that reads it must not change the run id. Known limit (W0b): replace()
        # takes fold=0, so a directory from the SECOND pass through the doubled
        # hour at the end of DST (e.g. 25.10. 02:00-03:00) reads one hour early.
        return _iso(datetime.strptime("".join(m.groups()), "%Y%m%d%H%M%S")
                    .replace(tzinfo=ARCHIVE_TZ))
    return _iso(datetime.fromtimestamp(fallback, timezone.utc))


def _summary(path):
    try:
        lines = path.read_text().splitlines()
    except OSError:
        return None, []
    m = SUMMARY.match(lines[0]) if lines else None
    return m, lines[1:]


def token_lanes(log):
    """Token log -> [(at, strand, lane)] of every `take` that named a lane."""
    out = []
    try:
        text = pathlib.Path(log).read_text()
    except (OSError, TypeError):
        return out
    for raw in text.splitlines():
        cols = raw.split("\t")
        if len(cols) >= 5 and cols[1] == "take" and cols[4].startswith("lane "):
            out.append((cols[0], cols[3], cols[4].split()[1]))
    return out


def _lane_of(takes, strand, started):
    """The lane of the strand's last `take` before the run started."""
    hits = [(at, lane) for at, holder, lane in takes
            if holder.rsplit("/", 1)[-1] == strand and at <= started]
    return max(hits)[1] if hits else None


def _runner(paths, takes, strand, started, local):
    paths = [p for p in paths if p]
    if any(p.startswith(LANE_ROOT) for p in paths):
        return _lane_of(takes, strand, started) or "unknown"
    return local


def _counts(stations):
    out = {k: 0 for k in _metrics.STATION_COUNTS}
    for s in stations:
        key = s["verdict"].lower()
        if key in out:
            out[key] += 1
    return out


def _from_receipt(path, rel, takes, local):
    r = json.loads(path.read_text())
    m, _ = _summary(path.parent / "summary.txt")
    started, finished = r["started"], r["finished"]
    secs = int(m["secs"]) if m and m["v"] == r["verdict"] else int(
        (_store.parse_at(finished) - _store.parse_at(started)).total_seconds())
    stations = [{"name": s["name"], "verdict": s["verdict"], "secs": s.get("secs") or 0,
                 "scope": s.get("scope") or ""} for s in r["stations"]]
    strand = rel.parts[3]
    paths = [r.get("archive") or ""] + [s.get("log") or "" for s in r["stations"]]
    return {"repo": "meclaw", "mode": r["mode"], "rev": r["rev"], "base": r.get("base"),
            "dirty": bool(r.get("dirty")), "started": _iso(_store.parse_at(started)),
            "secs": secs, "verdict": r["verdict"], "stations": stations,
            "strand": strand, "source": str(rel), "data": {},
            "runner": _runner(paths, takes, strand, started, local)}


def _from_summary(path, rel, local):
    m, rest = _summary(path)
    if m is None:
        return None
    repo, mode = ("meclaw", m["a"]) if m["a"] in MECLAW_MODES else (m["a"], "strand")
    data, stations = {}, []
    for raw in rest:
        if raw.startswith("branch "):
            data["branch"] = raw
            continue
        s = STATION.match(raw)
        if s:
            stations.append({"name": s["name"], "verdict": s["v"],
                             "secs": int(s["secs"]), "scope": ""})
    return {"repo": repo, "mode": mode, "rev": m["rev"], "base": None, "dirty": False,
            "started": _dir_start(path.parent.name, path.stat().st_mtime),
            "secs": int(m["secs"]), "verdict": m["v"], "stations": stations,
            "strand": rel.parts[3], "source": str(rel), "data": data, "runner": local}


def scan(root, runner_local=None, token_log=None, since=None, until=None):
    """Every archived run under `<root>/plans/*/receipts/`, deduplicated.

    `since`/`until` are UTC dates (`YYYY-MM-DD`, both inclusive) on `started`.
    """
    root = pathlib.Path(root)
    from . import lane   # the one rule of the own lane name (re-check tv-bn M7r B-M3)
    local = lane.own_name(runner_local)
    takes = token_lanes(token_log)
    dirs = set()
    for f in root.glob("plans/*/receipts/**/*"):
        if f.name == "summary.txt" or RECEIPT.fullmatch(f.name):
            dirs.add(f.parent)
    out, seen = [], set()
    for d in sorted(dirs):
        rel = d.relative_to(root)
        if len(rel.parts) < 4:
            continue
        receipts = sorted(f for f in d.iterdir() if RECEIPT.fullmatch(f.name)
                          and not f.name.startswith("last-"))
        recs = []
        for f in receipts:
            try:
                recs.append(_from_receipt(f, rel, takes, local))
            except (ValueError, KeyError, TypeError):
                continue
        if not receipts and (d / "summary.txt").is_file():
            recs.append(_from_summary(d / "summary.txt", rel, local))
        for rec in recs:
            if rec is None:
                continue
            day = rec["started"][:10]
            if (since and day < since) or (until and day > until):
                continue
            key = (rec["repo"], rec["rev"], rec["mode"], rec["started"])
            if key in seen:
                continue
            seen.add(key)
            rec["run"] = "run-" + hashlib.sha256("|".join(key).encode()).hexdigest()[:12]
            out.append(rec)
    return out


def sums(recs):
    out = {}
    for r in recs:
        s = out.setdefault(r["repo"], {"runs": 0, "secs": 0, "red": 0})
        s["runs"] += 1
        s["secs"] += r["secs"]
        s["red"] += r["verdict"] not in ("GREEN", "ASK")
    return out


def result_lines(s):
    order = sorted(s, key=lambda repo: (repo != "meclaw", repo))
    return [f"BACKFILL repo={repo} runs={s[repo]['runs']} "
            f"lane_hours={s[repo]['secs'] / 3600:.2f} red={s[repo]['red']}" for repo in order]


def annotate_diff(recs, root):
    """Set `diff` (V0 section 8) on meclaw runs from `git diff --name-only base rev`."""
    libs = _metrics.lib_crates(root)
    for r in recs:
        if r["repo"] != "meclaw" or not r.get("base"):
            continue
        res = subprocess.run(["git", "-C", str(root), "diff", "--name-only", r["base"], r["rev"]],
                             capture_output=True, text=True, check=False)
        if res.returncode == 0:
            r["diff"] = _metrics.diff_class(res.stdout.split(), libs)


def _verdict(v):
    return v if v in _store.VERDICTS and v != "ABORTED" else "RED"


def _write_one(db, r):
    at0 = _store.parse_at(r["started"])
    run = r["run"]
    wp = _store.item_create(db, "workpackage", f"{r['repo']} {r['mode']} {r['rev'][:8]}",
                            doc={"repo": r["repo"], "mode": r["mode"], "rev": r["rev"],
                                 "run": run, "stage": r["mode"], "source": r["source"],
                                 "strand": r["strand"], "backfill": True, "data": r["data"]},
                            at=r["started"])
    planned = {"run": run, "mode": r["mode"], "repo": r["repo"], "rev": r["rev"],
               "base": r["base"], "stations": [s["name"] for s in r["stations"]],
               "reason": {}, "cache_known": 0, "dirty": r["dirty"], "stage": r["mode"]}
    if r.get("diff"):
        planned["diff"] = r["diff"]
    _events.emit(db, "pass_planned", "status", planned, run=run, item=wp["id"], at=r["started"])
    _events.emit(db, "run_dispatched", "note",
                 {"run": run, "runner": r["runner"], "tree": None, "attempt": 1, "round": 1,
                  "deadline_at": None, "ws": None}, run=run, item=wp["id"], at=r["started"])
    t = at0
    for s in r["stations"]:
        start = _iso(t)
        t = t + timedelta(seconds=s["secs"])
        end = _iso(t)
        task = _store.item_create(db, "task", s["name"], parent=wp["id"],
                                  doc={"station": s["name"], "scope": s["scope"]}, at=start)
        tr = _store.run_open(db, task["id"], r["runner"], key=task["key"], at=start)
        verdict = _verdict(s["verdict"])
        _events.emit(db, "station_ended", "status",
                     {"run": run, "station": s["name"], "verdict": verdict, "secs": s["secs"],
                      "exit": 0 if verdict in _store.DONE_VERDICTS else 1,
                      "scope": s["scope"], "round": 1},
                     run=run, item=task["id"], at=end)
        _store.run_close(db, tr, verdict, at=end, evidence={"kind": "receipt",
                                                            "path": r["source"]})
    pass_verdict = r["verdict"] if r["verdict"] in ("GREEN", "RED", "ASK") else "RED"
    finished = _iso(at0 + timedelta(seconds=r["secs"]))
    _events.emit(db, "pass_ended", "status",
                 {"run": run, "verdict": pass_verdict, "secs": r["secs"],
                  "lane_seconds": r["secs"], "critical_path_s": r["secs"], "rounds": 1,
                  "cache_hits": 0, "stations": _counts(r["stations"])},
                 run=run, item=wp["id"], at=finished)
    wp = _store.item_get(db, wp["id"])
    v = _store.item_transition(db, wp["id"], "planned", "active", wp["version"], at=finished)
    _store.item_transition(db, wp["id"], "active", "finished", v, at=finished)


def known(db, run):
    return db.execute("SELECT 1 FROM journal WHERE run=? AND event='pass_ended' LIMIT 1",
                      (run,)).fetchone() is not None


def write(db, recs):
    """Write the runs the store does not know yet; metrics hooks stay quiet here
    (the caller pushes the backfilled lines in one go)."""
    saved = list(_events.HOOKS)
    _events.HOOKS.clear()
    written = skipped = 0
    try:
        for r in recs:
            if known(db, r["run"]):
                skipped += 1
                continue
            with _store.tx(db):  # one run, one transaction: no half runs
                _write_one(db, r)
            written += 1
    finally:
        _events.HOOKS[:] = saved
    return {"written": written, "known": skipped}


def _cmd_backfill(args):
    root = pathlib.Path(args.root).resolve()
    token_log = args.tokens or (os.environ.get("MECLAW_STRAND_TOKENS") or
                                os.environ.get("MECLAW_GATE_LOCK", "/tmp/meclaw-w26-cargo.lock")
                                .removesuffix(".lock") + ".tokens") + ".log"
    recs = scan(root, token_log=token_log, since=args.since, until=args.until)
    if not args.dry_run:
        annotate_diff(recs, root)
        db = _store.open(args.db)
        db.execute("PRAGMA synchronous = NORMAL")
        try:
            res = write(db, recs)
            print(f"BACKFILL written={res['written']} known={res['known']}")
            target = _metrics.url()
            if target and not args.no_push:
                try:
                    n = _metrics.push(target, _metrics.lines(db, runs=[r["run"] for r in recs]))
                    print(f"METRICS pushed={n} url_host={_metrics.url_host(target)}")
                except Exception as exc:  # noqa: BLE001 -- a push never fails the backfill
                    print(f"METRICS pushed=0 url_host={_metrics.url_host(target)} "
                          f"error={type(exc).__name__}", file=sys.stderr)
        finally:
            _store.close(db)
    for out in result_lines(sums(recs)):
        print(out)
    return 0


def register(sub):
    """`pipeline backfill [--root <repo>] [--since D] [--until D]`."""
    p = sub.add_parser("backfill", help="import the archived receipts as finished runs")
    p.add_argument("--root", default=".", help="repository whose plans/*/receipts/ is read")
    p.add_argument("--since", default=None, help="first UTC day (YYYY-MM-DD), inclusive")
    p.add_argument("--until", default=None, help="last UTC day (YYYY-MM-DD), inclusive")
    p.add_argument("--db", default=None, help="store path (default $MECLAW_PIPELINE_DB)")
    p.add_argument("--tokens", default=None, help="token log naming the lanes")
    p.add_argument("--dry-run", action="store_true", help="count only, write nothing")
    p.add_argument("--no-push", action="store_true", help="write the store, push no metrics")
    p.set_defaults(func=_cmd_backfill)
