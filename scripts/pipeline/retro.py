"""The retro as data and the process findings (W11, V0 sections 2, 3, 8).

Two halves over ONE source, the pipeline journal:

* **Process findings.** ``RULES`` is the rule table (data, not code): one
  entry per rule with what it measures, its threshold, its window and the one
  proposal sentence a reader acts on. ``evaluate(db)`` runs every rule and
  journals one ``process_finding {rule, subject, value, threshold, since}`` per
  (rule, subject) that is not already open; ``resolve(db, seq, note)`` writes
  the follow-up row ``finding_resolved`` (the journal stays append-only). A
  finding is never a blocker: ``evaluate`` and the hook never raise, a broken
  rule is skipped, a rule whose data does not exist yet produces nothing.
* **Retro numbers.** ``table(db, since, until)`` gives one row per number
  (value + definition), ``quarter`` the four columns Q14..Q17 of
  ``scripts/wave_retro.py``. Every number is a function below whose docstring
  is its definition. ``pipeline retro <wave>`` compares the wave's window with
  the window before it (the backfill), so before and after come from the same
  journal. The command only reads (``store.open_ro``).

Importing this module hangs ``hook`` into ``events.HOOKS`` (like
``metrics.py``): after every ``pass_ended`` it evaluates the rules; it pushes
``pipe_finding`` (open=1 on ``process_finding``, 0 on ``finding_resolved``) and
``pipe_turnaround`` lines when ``$MECLAW_PIPELINE_METRICS_URL`` is set; a failed
push becomes ``metrics_failed``.

Percentiles: p50 is the median, p90 the nearest rank (``ceil(0.9 n)``-th
smallest value). Times are RFC 3339 UTC strings compared as text; a window is
half-open ``[since, until)``.
"""

import json
import math
import pathlib
import re
import statistics
import sys
from datetime import timedelta, timezone

from . import events as _events
from . import executor as _executor
from . import flakes as _flakes
from . import metrics as _metrics
from . import store as _store

# --- the rule table (B and C import these names) ------------------------------

RULES = (
    {"rule": "station_drift",
     "metric": "p50 seconds of a station's GREEN runs per repo/mode/station, in percent above"
               " the calibration",
     "threshold": 20,
     "window": "last 5 runs against the 20 runs before them (needs 25 runs and a"
               " difference of at least 1 s)",
     "proposal": "Read the last five logs of the station and split or re-calibrate it"
                 " before its estimate drifts further."},
    {"rule": "cache_quota_low",
     "metric": "cached share of the stations of integration passes, percent",
     "threshold": 50,
     "window": "last 3 integration passes per repo (only while the cache table holds rows)",
     "proposal": "Check the cache keys of the integration stations: a key that changes"
                 " with every merge never hits."},
    {"rule": "flake_rate_high",
     "metric": "flaky runs over all runs of one test (flakes table), percent",
     "threshold": 5,
     "window": "every run the flakes table counts",
     "proposal": "Quarantine the test with a GH issue or fix it; a test above five percent"
                 " teaches builders to rerun."},
    {"rule": "coverage_stale",
     "metric": "age of the newest coverage_built in days",
     "threshold": 7,
     "window": "newest coverage_built",
     "proposal": "Run the coverage build on a lane at night; the test selection reads a"
                 " stale map."},
    {"rule": "minimum_stale",
     "metric": "runner fingerprint changed after the newest minimum_measured",
     "threshold": 0,
     "window": "newest minimum_measured per stage and runner (target_free_g ignored)",
     "proposal": "Measure the minimum of that stage on that runner again; the old one"
                 " describes a different machine."},
    {"rule": "lane_reserve_breached",
     "metric": "disk watchdog firings plus ABORTED hygiene checks per runner",
     "threshold": 2,
     "window": "last 24 h",
     "proposal": "Collect stray workspaces on the lane or raise its reserve before the"
                 " next pass."},
    {"rule": "stray_target_old",
     "metric": "hours since a target or tree workspace was last touched",
     "threshold": 24,
     "window": "workspaces of kind target/tree that are neither evicted nor collected",
     "proposal": "Collect or evict the workspace with the hygiene run; nobody touched it"
                 " for a day."},
    {"rule": "turnaround_spread",
     "metric": "builder turnaround p90 against p50 (pre -> gate -> merge), factor",
     "threshold": 2,
     "window": "the wave of the newest workpackage, else the last 7 days (needs 5"
               " turnarounds)",
     "proposal": "Read the slowest turnarounds: a long tail is queue or fix rounds, not"
                 " build time."},
    {"rule": "coverage_skipped_no_lane",
     "metric": "night coverage run skipped because no lane was on",
     "threshold": 0,
     "window": "each night report (emitted by night.py only)",
     "proposal": "Power a lane for the night coverage run, or accept the stale map until"
                 " the next working day."},
)

DRIFT_RECENT, DRIFT_BASE, DRIFT_MIN_DELTA_S = 5, 20, 1.0
CACHE_RUNS = 3
TURNAROUND_MIN = 5
TURNAROUND_MATCH_H = 24
# a fingerprint field that is a measure, not a property of the lane: the
# executor's own set (re-check tv-bn M7r A-H3, one source -- no mirror here)
VOLATILE_FINGERPRINT = _executor.FINGERPRINT_MEASURES
RED_VERDICTS = ("RED", "ABORTED", "ASK")
THRESHOLDS = pathlib.Path(__file__).resolve().parent.parent / "retro" / "thresholds.json"
WAVE_DATE = re.compile(r"(\d{4}-\d{2}-\d{2})")


# --- small helpers ------------------------------------------------------------

def _at(value=None):
    """`None` -> now; a datetime -> RFC 3339 UTC; a string stays."""
    if value is None:
        return _store.now()
    if isinstance(value, str):
        return value
    if value.tzinfo is None:  # a naive datetime is read as UTC
        value = value.replace(tzinfo=timezone.utc)
    return value.astimezone(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def _shift(at, **delta):
    dt = _store.parse_at(at) + timedelta(**delta)
    return dt.strftime("%Y-%m-%dT%H:%M:%SZ")


def _secs(a, b):
    return (_store.parse_at(b) - _store.parse_at(a)).total_seconds()


def _rows(db, sql, args=()):
    out = []
    for r in db.execute(sql, args):
        row = dict(r)
        if "data" in row:
            row["data"] = json.loads(row["data"]) if row["data"] else {}
        out.append(row)
    return out


def _p50(values):
    return float(statistics.median(values)) if values else None


def _p90(values):
    if not values:
        return None
    ordered = sorted(values)
    return float(ordered[max(0, math.ceil(0.9 * len(ordered)) - 1)])


def _counts(stations):
    if isinstance(stations, dict):
        return {k: int(v or 0) for k, v in stations.items()}
    return {"green": len(stations or ())}


def _fully_cached(counts):
    rest = sum(v for k, v in counts.items() if k not in ("cached", "skip", "note"))
    return counts.get("cached", 0) > 0 and rest == 0


def _planned(db, runs):
    """run -> (at, data) of its `pass_planned`."""
    out, runs = {}, list(dict.fromkeys(r for r in runs if r))
    for i in range(0, len(runs), 500):
        chunk = runs[i:i + 500]
        for r in _rows(db, "SELECT run, at, data FROM journal WHERE event='pass_planned'"
                           f" AND run IN ({','.join('?' * len(chunk))}) ORDER BY seq", chunk):
            out.setdefault(r["run"], (r["at"], r["data"]))
    return out


def _store_lost(db, runs):
    """The runs the gate ended ABORTED whatever their stream's `pass_ended`
    says: a `store_record_lost` finding, else its ledger line (re-check
    tb-s3-store M-2, tb-s3-store2 m-n3; `events.store_lost_runs`)."""
    return _events.store_lost_runs(db, runs)


def _dispatched(db, runs):
    """run -> runner of its first `run_dispatched`."""
    out, runs = {}, list(dict.fromkeys(r for r in runs if r))
    for i in range(0, len(runs), 500):
        chunk = runs[i:i + 500]
        for r in _rows(db, "SELECT run, data FROM journal WHERE event='run_dispatched'"
                           f" AND run IN ({','.join('?' * len(chunk))}) ORDER BY seq", chunk):
            out.setdefault(r["run"], r["data"].get("runner"))
    return out


def passes(db, since, until):
    """Every pass whose `pass_ended` lies in [since, until), with its plan.

    A pass: run, start, end, mode, repo, rev, base, strand, lock_wait, verdict,
    secs, lane (lane_seconds), crit (critical_path_s), rounds, cache_hits,
    counts (station verdict counts), runner (of its `run_dispatched`, else
    "unknown")."""
    ended = _rows(db, "SELECT seq, at, run, data FROM journal WHERE event='pass_ended'"
                      " AND at >= ? AND at < ? ORDER BY seq", (since, until))
    plans = _planned(db, [e["run"] for e in ended])
    runners = _dispatched(db, [e["run"] for e in ended])
    lost = _store_lost(db, [e["run"] for e in ended])
    out = []
    for e in ended:
        start, plan = plans.get(e["run"], (None, {}))
        d = e["data"]
        out.append({
            "run": e["run"], "start": start or e["at"], "end": e["at"],
            "mode": plan.get("mode") or "unknown", "repo": plan.get("repo") or "unknown",
            "rev": plan.get("rev"), "base": plan.get("base"), "strand": plan.get("strand"),
            "lock_wait": float(plan.get("lock_wait_secs") or 0),
            "verdict": "ABORTED" if e["run"] in lost else d.get("verdict"),
            "secs": float(d.get("secs") or 0),
            "lane": float(d.get("lane_seconds") or 0),
            "crit": float(d.get("critical_path_s") or 0),
            "rounds": int(d.get("rounds") or 1), "cache_hits": int(d.get("cache_hits") or 0),
            "counts": _counts(d.get("stations")),
            "runner": runners.get(e["run"]) or "unknown",
        })
    return out


def _count(db, sql, args=()):
    return db.execute(sql, args).fetchone()[0]


# --- retro numbers --------------------------------------------------------------

def lane_minutes_by_day(db, since, until):
    """Q14 detail: lane minutes per UTC day of the pass end, split by verdict.

    A pass puts its `lane_seconds` into `red` when its verdict is not GREEN,
    into `cached` when every station that counted was CACHED (skip/note aside),
    else into `green`. Passes without lane seconds (local pre passes) count no
    day."""
    days = {}
    for p in passes(db, since, until):
        if p["lane"] <= 0:
            continue
        if p["verdict"] != "GREEN":
            cls = "red"
        elif _fully_cached(p["counts"]):
            cls = "cached"
        else:
            cls = "green"
        day = days.setdefault(p["end"][:10], {"green": 0.0, "red": 0.0, "cached": 0.0})
        day[cls] += p["lane"] / 60
    return {d: {k: round(v, 2) for k, v in c.items()} for d, c in sorted(days.items())}


def lane_hours_peak_day(db, since, until):
    """Q14 detail: lane hours of the busiest day, summed over all lanes."""
    days = lane_minutes_by_day(db, since, until)
    if not days:
        return None
    return max(sum(c.values()) for c in days.values()) / 60


def lane_hours_by_lane_and_day(db, since, until):
    """Lane hours per runner and UTC day of the pass end (passes with lane seconds)."""
    out = {}
    for p in passes(db, since, until):
        if p["lane"] <= 0:
            continue
        days = out.setdefault(p["runner"], {})
        days[p["end"][:10]] = days.get(p["end"][:10], 0.0) + p["lane"] / 3600
    return {lane: {d: round(h, 3) for d, h in sorted(days.items())}
            for lane, days in sorted(out.items())}


def lane_hours_peak_lane_day(db, since, until):
    """Q14: lane hours of the busiest (lane, day) in the window (OR-DT.ORCH.15).

    The 8 h/day limit holds per lane, not for the sum of all lanes: three lanes
    busy for six hours each are a healthy day, one lane busy for nine is a
    queue. Returns (hours, lane, day) or None."""
    best = None
    for lane, days in lane_hours_by_lane_and_day(db, since, until).items():
        for day, hours in days.items():
            if best is None or hours > best[0]:
                best = (hours, lane, day)
    return best


def _flake_data(db):
    return (_count(db, "SELECT count(*) FROM flakes") > 0
            or _count(db, "SELECT count(*) FROM journal WHERE event='flake_isolated'") > 0)


def flake_share(db, since, until):
    """Q15: share of red passes that are red because of a flake, percent.

    A red pass (verdict RED) is a flake when its run carries a `flake_isolated`
    with verdict FLAKY, or when every test its RED `test_result` rows name is a
    test the flakes table counts as flaky -- matched by `_flake_keys`, the
    register's own rule for every kind. Returns (value, reason); `n/a` while
    the store has no flake data at all (flake isolation, W8) or the window no
    red pass."""
    if not _flake_data(db):
        return None, "no flake data yet (flakes table and flake_isolated empty, W8)"
    red = [p for p in passes(db, since, until) if p["verdict"] == "RED"]
    if not red:
        return None, "no red pass in the window"
    flaky = {r[0] for r in db.execute("SELECT test FROM flakes WHERE flaky > 0")}
    hits = 0
    for p in red:
        rows = _rows(db, "SELECT event, data FROM journal WHERE run=? AND event IN"
                         " ('flake_isolated', 'test_result') ORDER BY seq", (p["run"],))
        if any(r["event"] == "flake_isolated" and r["data"].get("verdict") == "FLAKY"
               for r in rows):
            hits += 1
            continue
        names = [k for r in rows if r["event"] == "test_result"
                 and r["data"].get("verdict") == "RED" for k in _flake_keys(r["data"])]
        if names and all(flaky.intersection(k) for k in names):
            hits += 1
    return round(100 * hits / len(red), 1), None


def _flake_keys(d):
    """The register keys each failed test of a `test_result` body may stand under
    (re-check tb-s3-store2 m-3): `flakes.test_key` -- the register's own rule --
    for every kind (nextest `<binary>::<name>`, unittest `<module>.<Class>.<test>`).
    A scenario case counts first under its station key `<station>:case:<id>`
    (M-1); a bare `case:<id>` (a register from before M-1) counts only while
    the name itself carries no station."""
    part, station = d.get("part") or "", d.get("station") or ""
    for n in d.get("failed_names") or []:
        key = _flakes.test_key(part, n)
        if key.startswith("case:") and station:
            yield (f"{station}:{key}", key)
        else:
            yield (key,)


def cache_quota_by_mode(db, since, until):
    """Q16 detail: CACHED stations over all stations of the passes, percent per mode."""
    cached, total = {}, {}
    for p in passes(db, since, until):
        n = sum(p["counts"].values())
        if n <= 0:
            continue
        cached[p["mode"]] = cached.get(p["mode"], 0) + p["counts"].get("cached", 0)
        total[p["mode"]] = total.get(p["mode"], 0) + n
    return {m: round(100 * cached[m] / total[m], 1) for m in sorted(total)}


def parallelism(db, since, until):
    """Q17: p50 of critical_path_s / lane_seconds over the passes with lane seconds.

    1.0 is a sequential pass; the lower, the more stations ran side by side.
    Pre passes are left out (they run locally, without a lane)."""
    ratios = [p["crit"] / p["lane"] for p in passes(db, since, until)
              if p["lane"] > 0 and p["mode"] != "pre"]
    return _p50(ratios)


def turnarounds(db, since, until):
    """Builder turnarounds pre -> gate -> merge of the strand gates in the window.

    A strand pass is paired with the earliest unclaimed `pre` pass of the same
    repo that started at most 24 h before it and whose `rev` is the gate's
    `base` or `rev` (the tree the builder checked before committing). The merge
    is the first `merge_done` naming the gate's strand after the gate (W9);
    without one the turnaround ends at the gate. Returns dicts with repo,
    strand, gate_run, pre_s, gate_s, merge_s, total_s, at (the pre start)."""
    window = passes(db, _shift(since, hours=-TURNAROUND_MATCH_H), until)
    pres = sorted((p for p in window if p["mode"] == "pre"), key=lambda p: p["start"])
    gates = sorted((p for p in window if p["mode"] == "strand" and p["end"] >= since),
                   key=lambda p: p["start"])
    merges = _rows(db, "SELECT at, data FROM journal WHERE event='merge_done' AND at >= ?"
                       " ORDER BY seq", (since,))
    claimed, out = set(), []
    for g in gates:
        lo = _shift(g["start"], hours=-TURNAROUND_MATCH_H)
        cands = [p for p in pres if p["run"] not in claimed and p["repo"] == g["repo"]
                 and lo <= p["start"] <= g["start"] and p["rev"] in (g["base"], g["rev"])]
        if not cands:
            continue
        claimed.update(p["run"] for p in cands)
        pre = cands[0]
        merge_at = next((m["at"] for m in merges if g["strand"]
                         and m["data"].get("strand") == g["strand"] and m["at"] >= g["end"]),
                        None)
        end = merge_at or g["end"]
        out.append({"repo": g["repo"], "strand": g["strand"], "gate_run": g["run"],
                    "pre_s": pre["secs"], "gate_s": _secs(g["start"], g["end"]),
                    "merge_s": _secs(g["end"], merge_at) if merge_at else None,
                    "total_s": _secs(pre["start"], end), "at": pre["start"]})
    return out


def turnaround_p50_p90(db, since, until):
    """p50 and p90 of the turnaround totals (seconds); (None, None) without any."""
    totals = [t["total_s"] for t in turnarounds(db, since, until)]
    return _p50(totals), _p90(totals)


def lane_wait(db, since, until):
    """Waiting for the lane: p50/p90 of `pass_planned.lock_wait_secs` of the lane
    passes, plus the `lease_offered` rows of the window (W3 lease queue)."""
    waits = [p["lock_wait"] for p in passes(db, since, until) if p["lane"] > 0]
    offers = _count(db, "SELECT count(*) FROM journal WHERE event='lease_offered'"
                        " AND at >= ? AND at < ?", (since, until))
    return _p50(waits), _p90(waits), offers


def rounds_per_workpackage(db, since, until):
    """Mean and max of `pass_ended.rounds` (rounds of one workpackage)."""
    rounds = [p["rounds"] for p in passes(db, since, until)]
    if not rounds:
        return None, None
    return round(sum(rounds) / len(rounds), 2), max(rounds)


def hygiene_aborts(db, since, until):
    """ABORTED hygiene checks, aborting watchdogs and ABORTED passes in the window."""
    rows = _rows(db, "SELECT event, data FROM journal WHERE event IN ('hygiene_checked',"
                     " 'watchdog_fired', 'pass_ended') AND at >= ? AND at < ?", (since, until))
    return sum(1 for r in rows
               if (r["event"] in ("hygiene_checked", "pass_ended")
                   and r["data"].get("verdict") == "ABORTED")
               or (r["event"] == "watchdog_fired" and r["data"].get("action") == "aborted"))


def quarter(db, since, until):
    """The four store columns of `wave_retro.py`: Q14 h/day, Q15 %, Q16 %, Q17 ratio."""
    peak = lane_hours_peak_lane_day(db, since, until)
    return {"Q14": None if peak is None else peak[0],
            "Q15": flake_share(db, since, until)[0],
            "Q16": cache_quota_by_mode(db, since, until).get("integration"),
            "Q17": parallelism(db, since, until)}


def _row(qid, metric, value, unit, source, reason=None):
    return {"id": qid, "metric": metric, "value": value, "unit": unit, "source": source,
            "reason": reason if value in (None, [None, None]) else None}


def table(db, since, until):
    """The retro rows of the window: {id, metric, value, unit, source, reason}."""
    ps = passes(db, since, until)
    days = lane_minutes_by_day(db, since, until)
    hours = {k: round(sum(d[k] for d in days.values()) / 60, 2)
             for k in ("red", "green", "cached")}
    flake, flake_reason = flake_share(db, since, until)
    quota = cache_quota_by_mode(db, since, until)
    w50, w90, offers = lane_wait(db, since, until)
    t = turnarounds(db, since, until)
    totals = [x["total_s"] for x in t]
    mean, top = rounds_per_workpackage(db, since, until)
    drift = _rule_station_drift(db, until)
    peak = lane_hours_peak_lane_day(db, since, until)
    total = lane_hours_peak_day(db, since, until)
    ratio = parallelism(db, since, until)
    return [
        _row("Q14", "lane_hours_peak_day_per_lane", None if peak is None else peak[0], "h",
             "pass_ended.lane_seconds per runner and UTC day of the pass end, busiest "
             "lane day" + ("" if peak is None else f" ({peak[1]}, {peak[2]})"),
             "no pass with lane seconds in the window"),
        _row(None, "lane_hours_peak_day_sum", None if total is None else round(total, 2), "h",
             "the same seconds summed over all lanes, busiest day (no limit)",
             "no pass with lane seconds in the window"),
        _row(None, "lane_hours_red_green_cached", [hours["red"], hours["green"],
                                                   hours["cached"]] if days else None, "h",
             "the same seconds split by pass verdict (cached = every station CACHED)",
             "no pass with lane seconds in the window"),
        _row(None, "passes_red_total", [sum(1 for p in ps if p["verdict"] in RED_VERDICTS),
                                        len(ps)], "n",
             "pass_ended rows of the window, red = RED/ABORTED/ASK"),
        _row("Q15", "flake_share_of_red_passes", flake, "%",
             "red passes with a FLAKY flake_isolated or only known flaky failed tests",
             flake_reason),
        _row("Q16", "cache_quota_integration", quota.get("integration"), "%",
             "CACHED over all stations of the integration passes (pass_ended.stations)",
             "no integration pass in the window"),
        _row(None, "cache_quota_by_mode", quota or None, "%",
             "the same per mode", "no pass with stations in the window"),
        _row("Q17", "critical_path_over_lane_s_p50", None if ratio is None
             else round(ratio, 3), "ratio",
             "p50 of critical_path_s / lane_seconds per pass (pre left out)",
             "no lane pass in the window"),
        _row(None, "lane_wait_s_p50_p90", [w50, w90] if w50 is not None else None, "s",
             f"pass_planned.lock_wait_secs of the lane passes; lease_offered rows: {offers}",
             "no lane pass in the window"),
        _row(None, "turnaround_p50_p90_s", [_p50(totals), _p90(totals)] if totals else None,
             "s", f"pre -> strand gate -> merge_done, {len(t)} paired gates "
                  f"({sum(1 for x in t if x['merge_s'] is not None)} with a merge)",
             "no strand gate with a matching pre pass in the window"),
        _row(None, "rounds_per_workpackage_mean_max",
             [mean, top] if mean is not None else None, "rounds",
             "pass_ended.rounds", "no pass in the window"),
        _row(None, "hygiene_aborts", hygiene_aborts(db, since, until), "n",
             "hygiene_checked/pass_ended ABORTED plus watchdog_fired action=aborted"),
        _row(None, "stations_with_p50_drift", len(drift), "n",
             "station_drift rule at the end of the window (5 vs. 20 runs, > +20 %)"),
    ]


# --- finding rules --------------------------------------------------------------

def _plans_all(db, now):
    return {r["run"]: r["data"] for r in _rows(
        db, "SELECT run, data FROM journal WHERE event='pass_planned' AND at <= ?", (now,))}


def _plan_of(db, run, cache=None):
    """The `pass_planned` data of `run` (journal index on run), memoised in `cache`."""
    if cache is not None and run in cache:
        return cache[run]
    row = db.execute("SELECT data FROM journal WHERE run=? AND event='pass_planned'"
                     " ORDER BY seq LIMIT 1", (run,)).fetchone()
    plan = json.loads(row[0]) if row and row[0] else {}
    if cache is not None:
        cache[run] = plan
    return plan


def _series_key(plan, station):
    return f"{plan.get('repo') or 'unknown'}/{plan.get('mode') or 'unknown'}/{station}"


def _drift_hit(subject, rows):
    if len(rows) < DRIFT_RECENT + DRIFT_BASE:
        return None
    recent = rows[-DRIFT_RECENT:]
    base = rows[-(DRIFT_RECENT + DRIFT_BASE):-DRIFT_RECENT]
    now_p50, base_p50 = _p50([s for _, s in recent]), _p50([s for _, s in base])
    if base_p50 <= 0 or now_p50 - base_p50 < DRIFT_MIN_DELTA_S:
        return None
    drift = (now_p50 / base_p50 - 1) * 100
    if drift > 20:
        return {"subject": subject, "value": round(drift, 1), "threshold": 20,
                "since": recent[0][0]}
    return None


def _green_secs(d):
    return d.get("verdict") == "GREEN" and d.get("secs")


def _drift_series_of_run(db, now, run):
    """#1136: the series `run` touched, each read newest-first and only as far
    back as the rule looks (DRIFT_RECENT + DRIFT_BASE green rows)."""
    plans = {}
    plan = _plan_of(db, run, plans)
    stations = sorted({r["data"]["station"] for r in _rows(
        db, "SELECT data FROM journal WHERE run=? AND event='station_ended' ORDER BY seq",
        (run,)) if _green_secs(r["data"]) and r["data"].get("station")})
    series = {}
    for station in stations:
        key = _series_key(plan, station)
        rows = []
        for at_, other, data in db.execute(
                "SELECT at, run, data FROM journal WHERE event = 'station_ended'"
                " AND json_extract(data, '$.station') = ? AND at <= ? ORDER BY seq DESC",
                (station, now)):   # index journal_station (store migration 3)
            d = json.loads(data) if data else {}
            if not _green_secs(d) or _series_key(_plan_of(db, other, plans), station) != key:
                continue
            rows.append((at_, float(d["secs"])))
            if len(rows) >= DRIFT_RECENT + DRIFT_BASE:
                break
        series[key] = rows[::-1]
    return series


def _rule_station_drift(db, now, run=None):
    if run is not None:
        series = _drift_series_of_run(db, now, run)
    else:
        plans = _plans_all(db, now)
        series = {}
        for r in _rows(db, "SELECT seq, at, run, data FROM journal WHERE event='station_ended'"
                           " AND at <= ? ORDER BY seq", (now,)):
            d = r["data"]
            if not _green_secs(d):
                continue
            key = _series_key(plans.get(r["run"], {}), d["station"])
            series.setdefault(key, []).append((r["at"], float(d["secs"])))
    return [hit for subject, rows in sorted(series.items())
            if (hit := _drift_hit(subject, rows)) is not None]


def _integration_tail(db, now, repo):
    """#1136: the last CACHE_RUNS integration `pass_ended` rows of `repo`, newest-first read."""
    plans, rows = {}, []
    for at_, other, data in db.execute(
            "SELECT at, run, data FROM journal WHERE event='pass_ended' AND at <= ?"
            " ORDER BY seq DESC", (now,)):
        plan = _plan_of(db, other, plans)
        if plan.get("mode") != "integration" or (plan.get("repo") or "unknown") != repo:
            continue
        rows.append({"at": at_, "run": other, "data": json.loads(data) if data else {}})
        if len(rows) >= CACHE_RUNS:
            break
    return rows[::-1]


def _quota_hit(repo, last):
    counts = [_counts(r["data"].get("stations")) for r in last]
    total = sum(sum(c.values()) for c in counts)
    if total <= 0:
        return None
    quota = 100 * sum(c.get("cached", 0) for c in counts) / total
    if quota < 50:
        return {"subject": f"{repo}/integration", "value": round(quota, 1),
                "threshold": 50, "since": last[0]["at"]}
    return None


def _rule_cache_quota_low(db, now, run=None):
    if _count(db, "SELECT count(*) FROM cache") == 0:
        return []
    if run is not None:
        plan = _plan_of(db, run)
        if plan.get("mode") != "integration":
            return []
        repo = plan.get("repo") or "unknown"
        by_repo = {repo: _integration_tail(db, now, repo)}
    else:
        plans = _plans_all(db, now)
        by_repo = {}
        for r in _rows(db, "SELECT at, run, data FROM journal WHERE event='pass_ended'"
                           " AND at <= ? ORDER BY seq", (now,)):
            plan = plans.get(r["run"], {})
            if plan.get("mode") == "integration":
                by_repo.setdefault(plan.get("repo") or "unknown", []).append(r)
    out = []
    for repo, rows in sorted(by_repo.items()):
        if len(rows) < CACHE_RUNS:
            continue
        hit = _quota_hit(repo, rows[-CACHE_RUNS:])
        if hit is not None:
            out.append(hit)
    return out


def _rule_flake_rate_high(db, now):
    out = []
    for test, runs, flaky, first in db.execute(
            "SELECT test, runs, flaky, first_at FROM flakes WHERE runs > 0 ORDER BY test"):
        rate = 100 * (flaky or 0) / runs
        if rate > 5:
            out.append({"subject": test, "value": round(rate, 1), "threshold": 5,
                        "since": first})
    return out


def _rule_coverage_stale(db, now):
    row = db.execute("SELECT at FROM journal WHERE event='coverage_built' AND at <= ?"
                     " ORDER BY at DESC LIMIT 1", (now,)).fetchone()
    if row is None:
        return []
    age = _secs(row[0], now) / 86400
    if age <= 7:
        return []
    return [{"subject": "coverage", "value": round(age, 1), "threshold": 7, "since": row[0]}]


def _stable(fp):
    return {k: v for k, v in fp.items() if k not in VOLATILE_FINGERPRINT}


def _rule_minimum_stale(db, now):
    runners = {r["run"]: r["data"].get("runner") for r in _rows(
        db, "SELECT run, data FROM journal WHERE event='run_dispatched' AND at <= ?", (now,))}
    prints = {}
    for r in _rows(db, "SELECT seq, at, run, data FROM journal WHERE event='lane_fingerprint'"
                       " AND at <= ? ORDER BY seq", (now,)):
        runner = runners.get(r["run"]) or r["data"].get("runner")
        if runner:
            prints.setdefault(runner, []).append((r["seq"], r["at"], _stable(r["data"])))
    newest = {}
    for r in _rows(db, "SELECT seq, at, data FROM journal WHERE event='minimum_measured'"
                       " AND at <= ? ORDER BY seq", (now,)):
        newest[(r["data"].get("stage"), r["data"].get("runner"))] = r
    out = []
    for (stage, runner), m in sorted(newest.items(), key=lambda kv: str(kv[0])):
        rows = prints.get(runner, [])
        before = [p for p in rows if p[0] < m["seq"]]
        if not before:
            continue
        changed = next((p for p in rows if p[0] > m["seq"] and p[2] != before[-1][2]), None)
        if changed:
            out.append({"subject": f"{stage}@{runner}", "value": 1, "threshold": 0,
                        "since": changed[1]})
    return out


def _rule_lane_reserve_breached(db, now):
    hits = {}
    for r in _rows(db, "SELECT at, event, data FROM journal WHERE event IN"
                       " ('watchdog_fired', 'hygiene_checked') AND at > ? AND at <= ?"
                       " ORDER BY seq", (_shift(now, hours=-24), now)):
        d = r["data"]
        if (r["event"] == "watchdog_fired" and d.get("kind") == "disk") or (
                r["event"] == "hygiene_checked" and d.get("verdict") == "ABORTED"):
            hits.setdefault(d.get("runner") or "unknown", []).append(r["at"])
    return [{"subject": runner, "value": len(ats), "threshold": 2, "since": ats[0]}
            for runner, ats in sorted(hits.items()) if len(ats) >= 2]


def _rule_stray_target_old(db, now):
    out = []
    for r in db.execute("SELECT id, runner, created_at, used_at, closed_at FROM workspaces"
                        " WHERE kind IN ('target', 'tree')"
                        " AND state NOT IN ('evicted', 'collected') ORDER BY id"):
        stamps = [s for s in (r["created_at"], r["used_at"], r["closed_at"]) if s]
        if not stamps:
            continue
        touched = max(stamps)
        hours = _secs(touched, now) / 3600
        if hours > 24:
            out.append({"subject": f"{r['runner']}:{r['id']}", "value": round(hours, 1),
                        "threshold": 24, "since": touched})
    return out


def _current_wave(db):
    for (doc,) in db.execute("SELECT doc FROM items WHERE type='workpackage'"
                             " ORDER BY created_at DESC, id DESC LIMIT 50"):
        try:
            wave = json.loads(doc or "{}").get("wave")
        except ValueError:
            continue
        if wave:
            return wave
    return None


def _rule_turnaround_spread(db, now):
    wave = _current_wave(db)
    since = None
    if wave:
        try:
            since, _ = wave_window(wave, db)
        except ValueError:
            since = None
    subject = wave if since else "last-7d"
    since = since or _shift(now, days=-7)
    totals = [t["total_s"] for t in turnarounds(db, since, _shift(now, seconds=1))]
    if len(totals) < TURNAROUND_MIN:
        return []
    p50, p90 = _p50(totals), _p90(totals)
    if p90 > 2 * p50:
        return [{"subject": subject, "value": p90, "threshold": round(2 * p50, 1),
                 "since": since}]
    return []


RULE_FUNCS = {
    "station_drift": _rule_station_drift,
    "cache_quota_low": _rule_cache_quota_low,
    "flake_rate_high": _rule_flake_rate_high,
    "coverage_stale": _rule_coverage_stale,
    "minimum_stale": _rule_minimum_stale,
    "lane_reserve_breached": _rule_lane_reserve_breached,
    "stray_target_old": _rule_stray_target_old,
    "turnaround_spread": _rule_turnaround_spread,
    # coverage_skipped_no_lane: emitted by night.py only
}


# --- findings: evaluate, list, resolve, proposals ----------------------------------

def open_findings(db):
    """Every `process_finding` without a later `finding_resolved` naming its seq."""
    resolved = set()
    for (data,) in db.execute("SELECT data FROM journal WHERE event='finding_resolved'"):
        try:
            resolved.add(int(json.loads(data or "{}").get("seq")))
        except (TypeError, ValueError):
            continue
    out = []
    for r in _rows(db, "SELECT seq, at, data FROM journal WHERE event='process_finding'"
                       " ORDER BY seq"):
        if r["seq"] in resolved:
            continue
        d = r["data"]
        out.append({"seq": r["seq"], "at": r["at"], "rule": d.get("rule"),
                    "subject": d.get("subject"), "value": d.get("value"),
                    "threshold": d.get("threshold"), "since": d.get("since")})
    return out


# Rules that read journal series take `run=`: the hook judges only the series
# the ended run touched (#1136); the others read bounded tables.
SCOPED_RULES = frozenset({"station_drift", "cache_quota_low"})


def evaluate(db, now=None, run=None):
    """Run every rule; journal one `process_finding` per new (rule, subject).

    With `run`, the series rules look only at the series that run touched,
    each read newest-first as far back as the rule looks (#1136).

    Returns the new seqs. Never raises: a broken rule is skipped, a store that
    cannot be read yields []."""
    try:
        now = _at(now)
        is_open = {(f["rule"], f["subject"]) for f in open_findings(db)}
    except Exception:  # noqa: BLE001 -- a finding is never a blocker
        return []
    new = []
    for rule in RULES:
        fn = RULE_FUNCS.get(rule["rule"])
        if fn is None:
            continue
        try:
            hits = (fn(db, now, run=run) if run is not None and rule["rule"] in SCOPED_RULES
                    else fn(db, now))
        except Exception:  # noqa: BLE001
            continue
        for hit in hits:
            key = (rule["rule"], hit["subject"])
            if key in is_open:
                continue
            try:
                seq = _events.emit(db, "process_finding", "finding",
                                   {"rule": rule["rule"], **hit}, at=now)
            except Exception:  # noqa: BLE001
                continue
            is_open.add(key)
            new.append(seq)
    return new


def resolve(db, seq, note):
    """Write `finding_resolved {seq, rule, subject, note}`; returns its seq.

    Refuses (ValueError) a seq that is no `process_finding` or is resolved."""
    seq = int(seq)
    row = db.execute("SELECT data FROM journal WHERE seq=? AND event='process_finding'",
                     (seq,)).fetchone()
    if row is None:
        raise ValueError(f"no process_finding with seq {seq}")
    if not any(f["seq"] == seq for f in open_findings(db)):
        raise ValueError(f"finding {seq} is already resolved")
    if not str(note or "").strip():
        raise ValueError("a resolution needs a note")
    d = json.loads(row[0] or "{}")
    return _events.emit(db, "finding_resolved", "note",
                        {"seq": seq, "rule": d.get("rule"), "subject": d.get("subject"),
                         "note": str(note).strip()})


def proposal_lines(findings):
    """`proposal: <rule> -> <sentence> (<subject>)`, sorted, one per finding."""
    sentences = {r["rule"]: r["proposal"] for r in RULES}
    lines = {f"proposal: {f.get('rule')} -> "
             f"{sentences.get(f.get('rule'), 'no rule entry; read the finding by hand.')}"
             f" ({f.get('subject')})" for f in findings}
    return sorted(lines)


def wave_window(wave_dir_name, db, root=None, runner=None):
    """(since, until) of a wave: from 00:00 UTC of the date in its folder name
    (`<name>-YYYY-MM-DD`) until now. The journal has no wave-close event yet
    (V0 section 3), so `until` is now; `pipeline retro --until` sets it.
    A wave without a date in its name (GH #1294, `nachzug-0702`) starts where
    the kit recorded it (`wave.wave_window_start`: `wave start` rev or the
    `wave open` row); no recorded start is refused as before."""
    dates = WAVE_DATE.findall(wave_dir_name or "")
    if dates:
        return f"{dates[-1]}T00:00:00Z", _store.now()
    from . import wave as _wave  # noqa: PLC0415 -- wave imports retro lazily too
    row = db.execute("PRAGMA database_list").fetchone() if db is not None else None
    root = root or pathlib.Path(__file__).resolve().parents[2]
    start, why = _wave.wave_window_start(wave_dir_name, root, (row[2] if row else None) or None,
                                         runner)
    if start is None:
        raise ValueError(f"no date in the wave name {wave_dir_name!r} and {why}")
    return start, _store.now()


# --- hook and metrics -------------------------------------------------------------

def _push(db, row, lines):
    target = _metrics.url()
    if not target or not lines:
        return
    try:
        _metrics.post(target, lines)
    except Exception as exc:  # noqa: BLE001 -- metrics never fail a run
        _events.emit(db, "metrics_failed", "finding",
                     {"run": row.get("run"), "url_host": _metrics.url_host(target),
                      "error": f"{type(exc).__name__}: {exc}"[:200]}, run=row.get("run"))


def finding_line(row, original=None):
    """The `pipe_finding` line of a `process_finding` (open=1) or `finding_resolved` (0)."""
    d = original if original is not None else row["data"]
    opened = row["event"] == "process_finding"
    return _metrics.line("pipe_finding", (("rule", d.get("rule")),
                                          ("subject", d.get("subject"))),
                         (("open", 1 if opened else 0), ("value", d.get("value")),
                          ("threshold", d.get("threshold"))), row["at"])


def turnaround_line(t, at):
    return _metrics.line("pipe_turnaround", (("repo", t["repo"]), ("strand", t["strand"])),
                         (("pre_s", t["pre_s"]), ("gate_s", t["gate_s"]),
                          ("merge_s", t["merge_s"]), ("total_s", t["total_s"])), at)


def _turnaround_of(db, row):
    until = _shift(row["at"], seconds=1)
    rows = turnarounds(db, _shift(row["at"], hours=-TURNAROUND_MATCH_H), until)
    if row["event"] == "pass_ended":
        return [t for t in rows if t["gate_run"] == row["run"]]
    strand = (row.get("data") or {}).get("strand")
    return [t for t in rows if strand and t["strand"] == strand and t["merge_s"] is not None]


def hook(db, row):
    """events.HOOKS entry; never raises."""
    try:
        event = row.get("event")
        if event == "pass_ended":
            evaluate(db, run=row.get("run"))
        if not _metrics.url():
            return
        if event == "process_finding":
            _push(db, row, [finding_line(row)])
        elif event == "finding_resolved":
            orig = db.execute("SELECT data FROM journal WHERE seq=?",
                              (row["data"].get("seq"),)).fetchone()
            _push(db, row, [finding_line(row, json.loads(orig[0]) if orig else row["data"])])
        elif event in ("pass_ended", "merge_done"):
            _push(db, row, [turnaround_line(t, row["at"]) for t in _turnaround_of(db, row)])
    except Exception:  # noqa: BLE001 -- a hook never fails the run
        pass


def install():
    if hook not in _events.HOOKS:
        _events.HOOKS.append(hook)


# --- report and CLI -----------------------------------------------------------------

def _thresholds():
    """Q14..Q17 thresholds from scripts/retro/thresholds.json (store_metrics), if present."""
    try:
        spec = json.loads(THRESHOLDS.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return {}
    return {m["id"]: m for m in spec.get("store_metrics", [])}


def missing_data(db):
    """What the store does not carry yet, as English sentences for the report."""
    out = []
    if _count(db, "SELECT count(*) FROM cache") == 0:
        out.append("cache table empty: Q16 counts CACHED stations of a cache that does not"
                   " exist yet (W3/W5).")
    if not _flake_data(db):
        out.append("no flake data: Q15 is n/a until flake isolation runs (W8).")
    if _count(db, "SELECT count(*) FROM journal WHERE event='merge_done'") == 0:
        out.append("no merge_done: turnarounds end at the strand gate (W9).")
    if _count(db, "SELECT count(*) FROM journal WHERE event LIKE 'lease_%'") == 0:
        out.append("no lease events: lane wait is lock_wait_secs only (W3).")
    seq = _count(db, "SELECT count(*) FROM journal WHERE event='pass_ended' AND"
                     " json_extract(data, '$.lane_seconds') > 0 AND"
                     " json_extract(data, '$.critical_path_s') ="
                     " json_extract(data, '$.lane_seconds')")
    if seq:
        out.append(f"{seq} passes carry critical_path_s = lane_seconds (backfill, sequential"
                   " runner): Q17 = 1.0 for them.")
    out.append("items.doc.wave is not written yet: the wave is a time window, not a tag.")
    return out


def report(db, wave, since, until, before_days=7.0):
    before = (_shift(since, days=-before_days), since)
    after_rows, before_rows = table(db, since, until), table(db, *before)
    rows = []
    for a, b in zip(after_rows, before_rows):
        rows.append({**a, "before": b["value"]})
    found = open_findings(db)
    return {"wave": wave, "generated": _store.now(),
            "window": {"since": since, "until": until},
            "before_window": {"since": before[0], "until": before[1]},
            "passes": {"after": len(passes(db, since, until)),
                       "before": len(passes(db, *before))},
            "rows": rows,
            "quarter": {"after": quarter(db, since, until), "before": quarter(db, *before)},
            "open_findings": found, "proposals": proposal_lines(found),
            "missing": missing_data(db)}


def _fmt(value):
    if value is None:
        return "n/a"
    if isinstance(value, float):
        return f"{value:.2f}".rstrip("0").rstrip(".") if value % 1 else f"{value:.0f}"
    if isinstance(value, list):
        return " / ".join(_fmt(v) for v in value)
    if isinstance(value, dict):
        return ", ".join(f"{k} {_fmt(v)}" for k, v in value.items())
    return str(value)


def _threshold_text(spec):
    if not spec:
        return "—"
    sign = {"<=": "≤", "<": "<", ">=": "≥"}.get(spec.get("cmp"), spec.get("cmp"))
    return f"{sign} {spec['threshold']:g} {spec.get('unit', '')}".strip()


def markdown(rep):
    """The report as Markdown, at most 40 lines."""
    limits = _thresholds()
    title = rep["wave"] or f"{rep['window']['since']} .. {rep['window']['until']}"
    out = [
        f"# Retro data — {title}",
        "",
        f"Generated {rep['generated']} by `pipeline retro` from the pipeline journal"
        " (read-only). After = the wave's window, before = the window ahead of it"
        " (backfill), both from the same store.",
        "",
        f"After: {rep['window']['since']} .. {rep['window']['until']}"
        f" ({rep['passes']['after']} passes) · before: {rep['before_window']['since']} .."
        f" {rep['before_window']['until']} ({rep['passes']['before']} passes)",
        "",
        "| Q | metric | unit | before | after | threshold | source |",
        "|---|---|---|---|---|---|---|",
    ]
    for r in rep["rows"]:
        source = r["source"] + (f" — {r['reason']}" if r.get("reason") else "")
        out.append(f"| {r['id'] or ''} | {r['metric']} | {r['unit']} | {_fmt(r['before'])} |"
                   f" {_fmt(r['value'])} | {_threshold_text(limits.get(r['id']))} |"
                   f" {source} |")
    out += ["", f"Open findings: {len(rep['open_findings'])}."]
    out += rep["proposals"][:5]
    out += ["", "Missing data:"]
    out += [f"- {m}" for m in rep["missing"]]
    out.append("")
    return "\n".join(out[:40]) + ("\n" if len(out) > 40 else "")


def _cmd_retro(args):
    try:
        db = _store.open_ro(args.db)
    except FileNotFoundError as exc:
        print(f"retro: {exc}", file=sys.stderr)
        return 1
    try:
        since = until = None
        if args.wave:
            since, until = wave_window(args.wave, db)
        since = args.since or since
        until = args.until or until or _store.now()
        if not since:
            print("retro: name a wave or --since", file=sys.stderr)
            return 2
        rep = report(db, args.wave, since, until, args.before_days)
    finally:
        _store.close(db)
    text = markdown(rep)
    if args.json:
        pathlib.Path(args.json).write_text(json.dumps(rep, indent=2, sort_keys=True) + "\n",
                                           encoding="utf-8")
    if args.md:
        pathlib.Path(args.md).write_text(text, encoding="utf-8")
    print(text, end="")
    return 0


def _cmd_finding(args):
    if args.action == "resolve":
        db = _store.open(args.db)
        try:
            seq = resolve(db, args.seq, args.note)
        except ValueError as exc:
            print(f"finding: {exc}", file=sys.stderr)
            return 1
        finally:
            _store.close(db)
        print(f"FINDING resolved={args.seq} seq={seq}")
        return 0
    try:
        db = _store.open_ro(args.db)
    except FileNotFoundError as exc:
        print(f"finding: {exc}", file=sys.stderr)
        return 1
    try:
        found = open_findings(db)
    finally:
        _store.close(db)
    for f in found:
        print(f"{f['seq']} {f['at']} {f['rule']} {f['subject']} value={f['value']}"
              f" threshold={f['threshold']} since={f['since']}")
    for line in proposal_lines(found):
        print(line)
    return 0


def register(sub):
    """`pipeline retro <wave>|--since` and `pipeline finding resolve|list` (V0 section 9)."""
    p = sub.add_parser("retro", help="retro numbers of a wave or a window (read-only)")
    p.add_argument("wave", nargs="?", default=None, help="wave folder name, e.g. wave-x-2026-10-08")
    p.add_argument("--since", default=None, help="RFC 3339 UTC start (overrides the wave date)")
    p.add_argument("--until", default=None, help="RFC 3339 UTC end (default now)")
    p.add_argument("--before-days", type=float, default=7.0,
                   help="length of the comparison window before `since` (days)")
    p.add_argument("--json", default=None, help="write the data as JSON")
    p.add_argument("--md", default=None, help="write the Markdown table (<= 40 lines)")
    p.add_argument("--db", default=None, help="store path (default $MECLAW_PIPELINE_DB)")
    p.set_defaults(func=_cmd_retro)

    f = sub.add_parser("finding", help="list or resolve process findings")
    fs = f.add_subparsers(dest="action", required=True)
    r = fs.add_parser("resolve", help="write finding_resolved for <seq>")
    r.add_argument("seq", type=int)
    r.add_argument("--note", required=True)
    r.add_argument("--db", default=None, help="store path (default $MECLAW_PIPELINE_DB)")
    r.set_defaults(func=_cmd_finding)
    ls = fs.add_parser("list", help="the open findings and their proposals")
    ls.add_argument("--db", default=None, help="store path (default $MECLAW_PIPELINE_DB)")
    ls.set_defaults(func=_cmd_finding)


install()
