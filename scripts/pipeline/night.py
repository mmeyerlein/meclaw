"""The night report as a watchdog (W11, R-AG-1): no poll, no cron.

Arming. After every `pass_ended` the events hook asks
`systemctl --user is-active pipe-night.timer`. A pending timer is kept (a
second run moves nothing); else a one-shot timer is armed::

    systemd-run --user --collect --unit pipe-night
        --on-calendar=<YYYY-MM-DD> 02:00:00        (02:00 after the run's local day)
        --timer-property=RemainAfterElapse=no      (unloaded once it fired)
        --setenv=MECLAW_PIPELINE_DB=<store>
        <python> <scripts/pipeline.py> night

A day without runs arms nothing. The calendar names one date, so the timer
fires once and is gone (`RemainAfterElapse=no`; with the default an elapsed
timer stays "active" and would block every later arming). The hook arms only
for the coordinator's store -- `~/.local/state/meclaw/pipeline/pipeline.db` of
the login user, whatever `$MECLAW_PIPELINE_DB` says, because tests point that
variable at temp stores -- unless `MECLAW_PIPELINE_NIGHT=1`;
`MECLAW_PIPELINE_NIGHT=0` disables it. The timer command is `pipeline night`
of the main tree when this core runs in a linked worktree that may be gone by
02:00 (`timer_script`). The timer never starts cargo.

Report. `pipeline night [--date D] [--out-root DIR]` covers the runs whose
local day lies after the day of the newest `night_report` and up to D (default:
yesterday, local). No `pass_ended` in that window -> no file, no journal row,
not even a rule pass. Else it runs `retro.evaluate`, writes `<date>.md` (at
most 60 lines, English) to `plans/<wave>/nacht/` of this repo (wave = `wave` of
the newest workpackage doc), without a wave next to the store (`nacht/`), and
journals `night_report {date, path, runs, findings}`. Contents: one row per
day (runs, lane minutes, red reasons classified by `RED_REASONS`), open
findings (`retro.open_findings`), top 5 stations by lane seconds, cache quota
per mode, and `retro.proposal_lines` -- rule output, never model text, sorted.
The report is a file and a journal row plus one stdout line for the systemd
journal; it sends nothing to anybody. A Grafana annotation follows when
`MECLAW_GRAFANA_URL` and `GRAFANA_ADMIN_PASS` are in the environment (as for
`grafana_push.py`); any failure there is ignored.

Coverage. If a `merge_done` is newer than the newest `coverage_built`, the
harness or binary list of the tree no longer matches the newest map
(`binaries_changed`, review Z-I1), or master moved past its `rev`, and W6 (`scripts/pipeline/coverage.py`) exists and
a build lane is on (a `runners` row whose state is set and not `off`, not the
owner's machine), the night
runs `pipeline coverage build --lane <that runner>` before the report and writes its
outcome (built, skipped, failed with exit and reason, review Z-I gap I2); otherwise it journals
`process_finding rule=coverage_skipped_no_lane`. It never powers a lane. A build with red
tests (exit `coverage.RED_EXIT`, welle-tb S1 C-Beleg) is reported `RED on lane X` with the
names and is a `process_finding rule=coverage_red` -- the build journals it; when the
night's store has none open, the night journals it itself. Red is never swallowed.
The outcome also names how many browser locks the installed map measured
(`browser_cover.night_line`, S1b point 4); an unmeasured lock is no red -- the
integration rule runs it (`browser_plan`).
"""

import argparse
import json
import os
import pathlib
import pwd
import re
import subprocess
import sys
from datetime import date as _date
from datetime import datetime, timedelta, timezone

from . import events as _events
from . import retro
from . import browser_cover
from .coverage import RED_EXIT as COVERAGE_RED_EXIT
from . import store as _store

REPO = pathlib.Path(__file__).resolve().parents[2]
UNIT = "pipe-night"
TIMER = UNIT + ".timer"
ENV_NIGHT = "MECLAW_PIPELINE_NIGHT"
MAX_LINES = 60
DAY_ROWS = 7          # newest days shown one per row; older ones fold into one row
FINDING_ROWS = 10
PROPOSAL_ROWS = 10
TOP_STATIONS = 5
LOST_AFTER_H = 6      # a planned run without `pass_ended` older than this is lost
RED_VERDICTS = ("RED", "ABORTED")
WAVE_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]*$")
COVERAGE_RULE = "coverage_skipped_no_lane"
COVERAGE_RED_RULE = "coverage_red"

# The hook's runner; tests replace it with a fake.
RUNNER = subprocess.run

# Red reasons, first match wins. A red run is a `pass_ended` with verdict RED
# or ABORTED; a lost run (planned, never ended) is always `foreign`.
#
#   class    rule
#   hygiene  the pass or a station is ABORTED (or carries an abort reason), a
#            `watchdog_fired` belongs to the run, or a `hygiene_checked` of the
#            run has verdict ABORTED -- the lane, not the code, stopped it
#   foreign  a red station exited 255 (ssh) or names a lane/ssh/lost reason,
#            or the run is lost -- the code was never judged
#   flake    every red station is either isolated as FLAKY (`flake_isolated`)
#            or all its failed test names stand in `flakes`
#   defect   everything else: the code is red
RED_REASONS = (
    ("hygiene", "aborted, watchdog_fired or hygiene_checked verdict ABORTED"),
    ("foreign", "red station exit 255 / lane, ssh or lost reason, lost run or store record"),
    ("flake", "every red station isolated FLAKY or its failed tests in flakes"),
    ("defect", "otherwise"),
)
CLASSES = tuple(c for c, _ in RED_REASONS)
FOREIGN_REASON = re.compile(r"^(lane|ssh|lost)")


# --- arming -----------------------------------------------------------------

def home_store():
    """The coordinator's store of the login user (not `$HOME`, not the env)."""
    home = pathlib.Path(pwd.getpwuid(os.getuid()).pw_dir)
    return home / ".local/state/meclaw/pipeline/pipeline.db"


def db_path(db):
    """The file behind a connection, or None (in-memory)."""
    for row in db.execute("PRAGMA database_list"):
        if row[1] == "main" and row[2]:
            return pathlib.Path(row[2])
    return None


def _same(a, b):
    try:
        return pathlib.Path(a).resolve() == pathlib.Path(b).resolve()
    except OSError:
        return False


def next_0200(now_local):
    """02:00 on the local day after `now_local` (a run at 01:00 still waits a day)."""
    day = now_local.date() + timedelta(days=1)
    return f"{day.isoformat()} 02:00:00"


def timer_script(repo=REPO):
    """`scripts/pipeline.py` of the main tree when `repo` is a linked worktree whose
    main tree has the night command; else the one of `repo`."""
    own = repo / "scripts" / "pipeline.py"
    git = repo / ".git"
    try:
        if git.is_file():
            text = git.read_text().strip()
            if text.startswith("gitdir:"):
                gitdir = pathlib.Path(text.split(":", 1)[1].strip())
                if not gitdir.is_absolute():
                    gitdir = (repo / gitdir).resolve()
                if gitdir.parent.name == "worktrees":
                    main = gitdir.parent.parent.parent
                    if (main / "scripts" / "pipeline" / "night.py").is_file():
                        return main / "scripts" / "pipeline.py"
    except OSError:
        pass
    return own


def _python(*args):
    """The kit's one Python (`lane.PYTHON`, re-check tv-bn M7m M2): the timer
    unit and the coverage build reach the narrow point."""
    from . import lane
    return lane.python_argv(*args)


def timer_argv(when, path, script=None):
    return ["systemd-run", "--user", "--collect", "--unit", UNIT,
            f"--on-calendar={when}", "--timer-property=RemainAfterElapse=no",
            f"--setenv=MECLAW_PIPELINE_DB={path}",
            "--description=meclaw pipeline night report",
            *_python(script or timer_script(), "night")]


def _now(now):
    if now is None:
        return datetime.now(timezone.utc)
    if isinstance(now, str):
        return _store.parse_at(now)
    return now if now.tzinfo else now.astimezone()


def arm(db, now=None, runner=subprocess.run, store_path=None):
    """Arm the one-shot night timer; `armed|kept|skipped|disabled|failed`."""
    mode = os.environ.get(ENV_NIGHT, "").strip()
    if mode == "0":
        return "disabled"
    path = pathlib.Path(store_path) if store_path else db_path(db)
    if path is None or (mode != "1" and not _same(path, home_store())):
        return "skipped"
    try:
        res = runner(["systemctl", "--user", "is-active", TIMER],
                     capture_output=True, text=True, check=False, timeout=10)
        if res.returncode == 0 and (res.stdout or "").strip() == "active":
            return "kept"
        when = next_0200(_now(now).astimezone())
        res = runner(timer_argv(when, path), capture_output=True, text=True,
                     check=False, timeout=30)
        return "armed" if res.returncode == 0 else "failed"
    except Exception:  # noqa: BLE001 -- arming never fails a run
        return "failed"


def hook(db, row):
    """events.HOOKS entry: a `pass_ended` arms the night timer; never raises.

    The time is the wall clock, not the row's `at`: a backfilled run of last
    month must not name a calendar day that has passed (it would never fire)."""
    if row.get("event") != "pass_ended":
        return
    try:
        arm(db, runner=RUNNER)
    except Exception:  # noqa: BLE001
        pass


def install():
    if hook not in _events.HOOKS:
        _events.HOOKS.append(hook)


# --- journal reads ------------------------------------------------------------

def _rows(db, sql, args=()):
    out = []
    for r in db.execute(sql, args):
        row = dict(r)
        row["data"] = json.loads(row["data"]) if row.get("data") else {}
        out.append(row)
    return out


def _local_day(at):
    return _store.parse_at(at).astimezone().date()


def last_report_day(db):
    row = db.execute("SELECT data FROM journal WHERE event='night_report'"
                     " ORDER BY seq DESC LIMIT 1").fetchone()
    if not row or not row[0]:
        return None
    try:
        return _date.fromisoformat(json.loads(row[0])["date"])
    except (KeyError, ValueError):
        return None


def window(db, day, now):
    """`(ended, lost)`: `pass_ended` rows and lost runs `{run, at}` with a local day
    after the last report's day and up to `day`. A pass the store lost a record
    of reads ABORTED whatever its stream said (re-check tb-s3-store2 m-n2)."""
    since = last_report_day(db)

    def inside(at):
        d = _local_day(at)
        return d <= day and (since is None or d > since)

    ended = [r for r in _rows(db, "SELECT seq, at, run, data FROM journal"
                                  " WHERE event='pass_ended' ORDER BY seq") if inside(r["at"])]
    gone = _events.store_lost_runs(db, [r["run"] for r in ended])
    ended = [dict(r, data=dict(r["data"], verdict="ABORTED")) if r["run"] in gone else r
             for r in ended]
    done = {r[0] for r in db.execute("SELECT DISTINCT run FROM journal WHERE event='pass_ended'")}
    cutoff = now - timedelta(hours=LOST_AFTER_H)
    lost = [{"run": r["run"], "at": r["at"]} for r in _rows(
        db, "SELECT seq, at, run, data FROM journal WHERE event='pass_planned' ORDER BY seq")
        if r["run"] not in done and inside(r["at"]) and _store.parse_at(r["at"]) < cutoff]
    return ended, lost


def _flake_tests(db):
    """The register keys the flakes table counts as flaky -- Q15's set
    (`retro.flake_share`). Rows exist for every test ever red, so a row alone
    is no flake (re-check tb-s3-store2 m-3, night side)."""
    return {r[0] for r in db.execute("SELECT test FROM flakes WHERE flaky > 0")}


def classify(db, runs):
    """`{run: class}` for red runs by `RED_REASONS` (journal and `flakes` only)."""
    known = _flake_tests(db)
    gone = _events.store_lost_runs(db, runs)
    out = {}
    for run in runs:
        if run in gone:  # the store, not the code (re-check tb-s3-store2 m-n2)
            out[run] = "foreign"
            continue
        rows = _events.read_run(db, run)
        ended = [r for r in rows if r["event"] == "pass_ended"]
        verdict = ended[-1]["data"].get("verdict") if ended else None
        last = {}
        for r in rows:
            if r["event"] == "station_ended":
                last[r["data"].get("station")] = r["data"]
        reds = [d for d in last.values() if d.get("verdict") in RED_VERDICTS]
        aborted = verdict == "ABORTED" or any(
            d.get("verdict") == "ABORTED" or d.get("reason") in _store.ABORT_REASONS
            for d in last.values())
        hygiene = aborted or any(
            r["event"] == "watchdog_fired"
            or (r["event"] == "hygiene_checked" and r["data"].get("verdict") == "ABORTED")
            for r in rows)
        if hygiene:
            out[run] = "hygiene"
            continue
        if not ended or any(d.get("exit") == 255 or FOREIGN_REASON.match(str(d.get("reason") or ""))
                            for d in reds):
            out[run] = "foreign"
            continue
        isolated = {(r["data"].get("station")) for r in rows
                    if r["event"] == "flake_isolated" and r["data"].get("verdict") == "FLAKY"}
        # Each failed test as the register keys it may stand under, by Q15's
        # rule (`retro._flake_keys` over `flakes.test_key`): raw names missed
        # every nextest flake (`d` against `c::d`) and every legacy scenario
        # line (re-check tb-s3-store2 m-3, night side).
        failed = {}
        for r in rows:
            if r["event"] == "test_result" and r["data"].get("verdict") == "RED":
                failed.setdefault(r["data"].get("station"), []).extend(
                    retro._flake_keys(r["data"]))

        def flaky(d):
            name = d.get("station")
            if name in isolated:
                return True
            keys = [(n,) for n in d.get("failed_names") or []] + failed.get(name, [])
            return bool(keys) and all(known.intersection(k) for k in keys)

        out[run] = "flake" if reds and all(flaky(d) for d in reds) else "defect"
    return out


def _station_stats(db, runs):
    """`(lane seconds by station, cache counts by mode)` over the runs."""
    lane, cache = {}, {}
    for run in runs:
        mode = "unknown"
        for r in _events.read_run(db, run):
            if r["event"] == "pass_planned":
                mode = r["data"].get("mode") or "unknown"
            elif r["event"] == "station_ended":
                d = r["data"]
                name = d.get("station") or "?"
                n, secs = lane.get(name, (0, 0.0))
                lane[name] = (n + 1, secs + float(d.get("secs") or 0))
                if d.get("verdict") in ("GREEN", "RED", "CACHED", "FLAKY", "ABORTED"):
                    hit, total = cache.get(mode, (0, 0))
                    cache[mode] = (hit + (d.get("verdict") == "CACHED"), total + 1)
    return lane, cache


# --- coverage -----------------------------------------------------------------

def coverage_cli():
    """`scripts/pipeline.py` when W6's `coverage.py` exists next to this module."""
    if (pathlib.Path(__file__).resolve().parent / "coverage.py").is_file():
        return REPO / "scripts" / "pipeline.py"
    return None


def lane_name(db):
    """The lane the night's coverage build goes to: the first runner that is
    on (state set and not `off`), by name -- never the owner's machine (transport `local`,
    class `egon`), which `coverage build` refuses anyway."""
    row = db.execute("SELECT name FROM runners WHERE state IS NOT NULL AND state != ''"
                     " AND state != 'off' AND transport IS NOT 'local'"
                     " AND class IS NOT 'egon' ORDER BY name LIMIT 1").fetchone()
    return row[0] if row else None


def _map_and_metadata():
    """The newest coverage map and the cached `cargo metadata` of this tree (or None)."""
    from . import gate as _gate
    from . import narrow as _narrow
    cov, _why = _narrow.newest_map()
    try:
        with open(_gate.metadata_path(str(REPO)), encoding="utf-8") as fh:
            meta = json.load(fh)
    except (OSError, ValueError):
        meta = None
    return cov, meta if isinstance(meta, dict) else None


def binaries_changed(cov, metadata):
    """`(changed, gone, new)`: binaries of the map the tree lacks (a fold, a
    rename) and test harnesses of the tree the map lacks. Without a map or
    metadata nothing is claimed (review Z-I1: the W6 map went stale in silence)."""
    if not cov or not isinstance(metadata, dict):
        return False, [], []
    from . import plan as _plan
    tree = {b for p in _plan._members(metadata) for b, _n in _plan._binaries(p)}
    mapped = {p.get("binary") for p in cov.get("parts") or []}
    gone = sorted(mapped - tree)
    new = sorted(b for b in tree - mapped if "::" in b and "::bin/" not in b)
    return bool(gone or new), gone, new


def coverage_due(db, runner):
    """`(due, merges_since, since)`: a `merge_done` after the newest map, the
    harness/binary list changed, or master moved."""
    built = db.execute("SELECT seq, at, data FROM journal WHERE event='coverage_built'"
                       " ORDER BY seq DESC LIMIT 1").fetchone()
    after = built[0] if built else 0
    merges = db.execute("SELECT count(*), min(at) FROM journal WHERE event='merge_done'"
                        " AND seq > ?", (after,)).fetchone()
    if merges[0]:
        return True, merges[0], merges[1]
    if binaries_changed(*_map_and_metadata())[0]:
        return True, 0, built[1] if built else None
    if built:
        rev = (json.loads(built[2]) if built[2] else {}).get("rev")
        try:
            res = runner(["git", "-C", str(REPO), "rev-parse", "master"],
                         capture_output=True, text=True, check=False, timeout=10)
            head = (res.stdout or "").strip()
            if res.returncode == 0 and head and rev and head != rev:
                return True, 0, built[1]
        except Exception:  # noqa: BLE001 -- unknown master = nothing due
            pass
    return False, 0, None


# --- report ---------------------------------------------------------------------

def wave_of(db):
    """`wave` of the newest workpackage doc when it is a plain directory name."""
    row = db.execute("SELECT doc FROM items WHERE type='workpackage'"
                     " ORDER BY created_at DESC, rowid DESC LIMIT 1").fetchone()
    try:
        wave = json.loads(row[0]).get("wave") if row and row[0] else None
    except (ValueError, AttributeError):
        wave = None
    if isinstance(wave, str) and WAVE_RE.match(wave) and ".." not in wave:
        return wave
    return None


def report_dir(db, repo=REPO):
    wave = wave_of(db)
    if wave:
        return repo / "plans" / wave / "nacht"
    path = db_path(db)
    return (path.parent if path else _store.default_path().parent) / "nacht"


def _fmt(value):
    if isinstance(value, float):
        return f"{value:g}"
    return str(value)


def render(day, since, ended, lost, classes, findings, proposals, lane, cache, coverage, wave):
    days = {}
    for r in ended:
        d = _local_day(r["at"])
        row = days.setdefault(d, {"runs": 0, "green": 0, "red": 0, "lane_s": 0.0, "lost": 0,
                                  **{c: 0 for c in CLASSES}})
        v = r["data"].get("verdict")
        row["runs"] += 1
        row["green"] += v == "GREEN"
        row["red"] += v in RED_VERDICTS
        row["lane_s"] += float(r["data"].get("lane_seconds") or 0)
        if r["run"] in classes:
            row[classes[r["run"]]] += 1
    for r in lost:
        d = _local_day(r["at"])
        row = days.setdefault(d, {"runs": 0, "green": 0, "red": 0, "lane_s": 0.0, "lost": 0,
                                  **{c: 0 for c in CLASSES}})
        row["lost"] += 1
        row["foreign"] += 1
    order = sorted(days)
    shown, older = order[-DAY_ROWS:], order[:-DAY_ROWS]
    red_total = sum(days[d]["red"] for d in order)
    out = [f"# Night report {day.isoformat()}", "",
           f"window: {order[0].isoformat()} .. {day.isoformat()}"
           f" (after the report of {since.isoformat() if since else 'never'});"
           f" runs {len(ended)}, red {red_total}, lost {len(lost)}; wave {wave or 'none'}",
           "", "## Days", "",
           "| date | runs | green | red | lane min | defect | flake | hygiene | foreign | lost |",
           "|---|---|---|---|---|---|---|---|---|---|"]

    def cells(label, rows):
        tot = {k: sum(r[k] for r in rows) for k in rows[0]}
        return (f"| {label} | {tot['runs']} | {tot['green']} | {tot['red']} |"
                f" {tot['lane_s'] / 60:.1f} | {tot['defect']} | {tot['flake']} |"
                f" {tot['hygiene']} | {tot['foreign']} | {tot['lost']} |")

    if older:
        out.append(cells(f"older ({len(older)} days)", [days[d] for d in older]))
    for d in shown:
        out.append(cells(d.isoformat(), [days[d]]))
    out += ["", f"## Open findings ({len(findings)})", ""]
    for f in findings[:FINDING_ROWS]:
        out.append(f"- #{f.get('seq')} {f.get('rule')} {f.get('subject')}:"
                   f" {_fmt(f.get('value'))} vs {_fmt(f.get('threshold'))} since {f.get('since')}")
    if len(findings) > FINDING_ROWS:
        out.append(f"- ... {len(findings) - FINDING_ROWS} more (pipeline finding list)")
    if not findings:
        out.append("- none")
    out += ["", f"## Top {TOP_STATIONS} stations by lane seconds", "",
            "| station | runs | lane s |", "|---|---|---|"]
    top = sorted(lane.items(), key=lambda kv: (-kv[1][1], kv[0]))[:TOP_STATIONS]
    out += [f"| {name} | {n} | {secs:.0f} |" for name, (n, secs) in top] or ["| none | 0 | 0 |"]
    out += ["", "## Cache quota", ""]
    for mode in sorted(cache):
        hit, total = cache[mode]
        out.append(f"- {mode}: {hit}/{total} ({100.0 * hit / total if total else 0:.1f} %)")
    if not cache:
        out.append("- no stations")
    out += ["", "## Proposals", ""]
    out += ["- " + p for p in proposals[:PROPOSAL_ROWS]]
    if len(proposals) > PROPOSAL_ROWS:
        out.append(f"- ... {len(proposals) - PROPOSAL_ROWS} more proposals")
    if not proposals:
        out.append("- none")
    out += ["", f"coverage: {coverage}"]
    if len(out) > MAX_LINES:
        out = out[:MAX_LINES - 1] + ["(truncated)"]
    return out


def annotate(day, path, runs, findings):
    """A Grafana annotation of the report; `posted|skipped|failed`, never raises."""
    base = os.environ.get("MECLAW_GRAFANA_URL", "").strip()
    password = os.environ.get("GRAFANA_ADMIN_PASS", "")
    if not base or not password:
        return "skipped"
    try:
        from . import grafana_push
        api = grafana_push.Grafana(base, os.environ.get("GRAFANA_ADMIN_USER") or "admin",
                                   password)
        status, _ = api.call("POST", "/api/annotations", {
            "dashboardUID": "dev-tempo", "tags": ["night_report"],
            "time": int(datetime.now(timezone.utc).timestamp() * 1000),
            "text": f"night report {day}: runs {runs}, open findings {findings} ({path})"})
        return "posted" if 200 <= status < 300 else "failed"
    except Exception:  # noqa: BLE001
        return "failed"


def report(db, now=None, out_root=None, runner=subprocess.run, date=None):
    """Write the night report of `date` (default yesterday, local); its path or None."""
    now_dt = _now(now)
    day = (_date.fromisoformat(date) if isinstance(date, str)
           else date or (now_dt.astimezone() - timedelta(days=1)).date())
    since = last_report_day(db)
    ended, lost = window(db, day, now_dt)
    if not ended:
        return None
    at = now_dt.astimezone(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    try:
        retro.evaluate(db, now=now_dt)
    except Exception:  # noqa: BLE001 -- a broken rule never stops the report
        pass
    red = [r["run"] for r in ended if r["data"].get("verdict") in RED_VERDICTS]
    classes = classify(db, red)
    due, merges, cov_since = coverage_due(db, runner)
    if not due:
        coverage = "not due"
    elif coverage_cli() and lane_name(db):
        coverage = build_coverage(db, coverage_cli(), lane_name(db), runner)
    else:
        coverage = "skipped, " + ("no build lane on" if coverage_cli() else "W6 coverage.py missing")
        open_now = [f for f in _safe(retro.open_findings, db)
                    if f.get("rule") == COVERAGE_RULE and f.get("subject") == "coverage"]
        if not open_now:
            _events.emit(db, "process_finding", "finding",
                         {"rule": COVERAGE_RULE, "subject": "coverage", "value": merges,
                          "threshold": 0, "since": cov_since or at}, at=at)
    findings = sorted(_safe(retro.open_findings, db), key=lambda f: (f.get("seq") or 0))
    proposals = sorted(_safe(retro.proposal_lines, findings))
    lane, cache = _station_stats(db, [r["run"] for r in ended])
    target = pathlib.Path(out_root) if out_root else report_dir(db)
    wave = wave_of(db)
    lines = render(day, since, ended, lost, classes, findings, proposals, lane, cache,
                   coverage, wave)
    target.mkdir(parents=True, exist_ok=True)
    path = target / f"{day.isoformat()}.md"
    path.write_text("\n".join(lines) + "\n")
    counts = {c: sum(1 for v in classes.values() if v == c) for c in CLASSES}
    counts["foreign"] += len(lost)
    _events.emit(db, "night_report", "status",
                 {"date": day.isoformat(), "path": str(path), "runs": len(ended),
                  "findings": len(findings), "red": counts, "lost": len(lost)}, at=at)
    annotate(day.isoformat(), path, len(ended), len(findings))
    return path


def build_coverage(db, cli, lane, runner):
    """Run `pipeline coverage build --lane <lane>` and say what came of it
    (review Z-I gap I2): `built on lane X`, `skipped on lane X (exit 3): ...`
    (lane not up), `failed on lane X (exit n): ...`. The exit is never
    swallowed; the report line is the build's own outcome."""
    argv = _python(cli, "coverage", "build", "--lane", lane)
    store_file = db_path(db)
    if store_file is not None:
        # The build reports into the night's store, never the default one.
        argv += ["--db", str(store_file)]
    seq = _journal_seq(db)
    try:
        res = runner(argv, capture_output=True, text=True, check=False)
    except Exception as exc:  # noqa: BLE001 -- a broken start is a failed build, said so
        return "failed on lane %s (not started): %s" % (lane, exc)
    rc = res.returncode
    # S1b point 4: what the installed map measured of the browser locks
    locks = browser_cover.night_line(getattr(res, "stdout", None))
    locks = "; %s" % locks if locks else ""
    if rc == 0:
        return "built on lane %s (pipeline coverage build)%s" % (lane, locks)
    lines = [ln for ln in (res.stderr or "").splitlines() if ln.strip()]
    why = lines[-1].strip() if lines else "no message"
    if rc == COVERAGE_RED_EXIT:
        _coverage_red_finding(db, lane, why, seq)
        return "RED on lane %s (exit %d): %s%s" % (lane, rc, why, locks)
    word = "skipped" if rc == 3 else "failed"
    return "%s on lane %s (exit %d): %s%s" % (word, lane, rc, why, locks)


def _journal_seq(db):
    try:
        return db.execute("SELECT COALESCE(MAX(seq), 0) FROM journal").fetchone()[0]
    except Exception:  # noqa: BLE001 -- no journal: every finding counts as new
        return 0


def _coverage_red_finding(db, lane, why, since_seq=0):
    """A RED build is a finding (welle-tb S1 C-Beleg): the build journals
    `coverage_red` into this store; if THIS build journaled none (one that wrote
    elsewhere or not at all), the night journals one itself -- never two, never
    none. An older open coverage_red never stands in (re-check H2 m3)."""
    try:
        rows = db.execute("SELECT data FROM journal WHERE event='process_finding' AND seq > ?",
                          (since_seq,)).fetchall()
    except Exception:  # noqa: BLE001 -- unreadable: journal one, never none
        rows = []
    if any(json.loads(r[0] or "{}").get("rule") == COVERAGE_RED_RULE for r in rows):
        return
    _events.emit(db, "process_finding", "finding",
                 {"rule": COVERAGE_RED_RULE, "subject": "coverage", "value": 1, "threshold": 0,
                  "since": _store.now(), "lane": lane, "why": why})


def _safe(fn, arg):
    try:
        return list(fn(arg) or [])
    except Exception:  # noqa: BLE001
        return []


# --- CLI ------------------------------------------------------------------------

def _cmd_night(args):
    db = _store.open(args.db)
    try:
        path = report(db, out_root=args.out_root, date=args.date)
        row = db.execute("SELECT seq, data FROM journal WHERE event='night_report'"
                         " ORDER BY seq DESC LIMIT 1").fetchone()
    finally:
        db.close()
    if path is None:
        print(f"NIGHT date={args.date or 'yesterday'} report=none (no runs since the last report)")
        return 0
    data = json.loads(row[1])
    print(f"NIGHT date={data['date']} report={path} seq={row[0]} runs={data['runs']}"
          f" findings={data['findings']}")
    return 0


def register(sub):
    p = sub.add_parser("night", help="night report (runs only after a day with runs)")
    p.add_argument("--date", default=None, help="report day YYYY-MM-DD (default: yesterday)")
    p.add_argument("--out-root", default=None, help="directory for <date>.md")
    p.add_argument("--db", default=None, help="store path (default $MECLAW_PIPELINE_DB)")
    p.set_defaults(func=_cmd_night)


def main(argv=None):
    p = argparse.ArgumentParser(prog="pipeline")
    register(p.add_subparsers(dest="cmd", required=True))
    args = p.parse_args(["night"] + list(sys.argv[1:] if argv is None else argv))
    return args.func(args)


install()
