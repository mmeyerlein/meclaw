"""Metrics of the pipeline: Influx lines to VictoriaMetrics (V0 section 8).

Three measurements come from the journal:

| measurement    | from            | tags                                         | fields |
|----------------|-----------------|----------------------------------------------|--------|
| `pipe_station` | `station_ended` | repo, mode, station, runner, verdict, stage  | secs, build_s, run_s, round, cached, slow_n |
| `pipe_pass`    | `pass_ended`    | repo, mode, runner, verdict, diff            | secs, lane_seconds, critical_path_s, rounds, cache_hits, stations, minimum_s, x, flaky, lost |
| `pipe_flake`   | `flake_isolated`| repo, station, test                          | outcome (1 = FLAKY), rate |

The time of a line is the `at` of its event in ns, so a second push of the
same rows writes the same points (idempotent). Repo, mode, stage and diff come
from the run's `pass_planned`, the runner from its `run_dispatched`.

The target is `$MECLAW_PIPELINE_METRICS_URL` (the whole write URL with
`?db=devtempo`; empty = off). A push is one HTTP POST with a 3 s timeout; a
failed push becomes the event `metrics_failed {run, url_host, error}` and the
run goes on -- metrics never fail a run. Importing this module hangs `hook`
into `events.HOOKS`; `pipeline metrics push --since <at>` sends what a broken
target missed.
"""

import json
import os
import pathlib
import re
import sys
import urllib.parse
import urllib.request

from . import events as _events
from . import store as _store

URL_ENV = "MECLAW_PIPELINE_METRICS_URL"
TIMEOUT_S = 3
CHUNK = 5000
SOURCES = ("station_ended", "pass_ended", "flake_isolated")
STATION_COUNTS = ("green", "red", "cached", "flaky", "skip", "note", "ask", "aborted")


def url():
    return os.environ.get(URL_ENV, "").strip()


def url_host(target):
    """The host of a target URL -- never the URL itself (it may carry credentials)."""
    return urllib.parse.urlsplit(target).hostname or "?"


def _tag(value):
    text = str(value) if value not in (None, "") else "unknown"
    return re.sub(r"([,= ])", r"\\\1", text)


def _num(value):
    if isinstance(value, bool):
        return "1" if value else "0"
    if isinstance(value, int):
        return str(value)
    return repr(float(value))


def ns(at):
    dt = _store.parse_at(at)
    return int(dt.timestamp()) * 1_000_000_000 + dt.microsecond * 1000


def line(measurement, tags, fields, at):
    """One Influx line; tags in the given order, fields without a value left out."""
    tag_part = ",".join(f"{k}={_tag(v)}" for k, v in tags)
    field_part = ",".join(f"{k}={_num(v)}" for k, v in fields if v is not None)
    return f"{measurement},{tag_part} {field_part} {ns(at)}"


def context(db, run):
    """repo/mode/stage/diff from `pass_planned`, runner from `run_dispatched`."""
    ctx = {}
    for event, data in db.execute(
            "SELECT event, data FROM journal WHERE run=? AND event IN"
            " ('pass_planned', 'run_dispatched') ORDER BY seq", (run,)):
        data = json.loads(data) if data else {}
        if event == "pass_planned":
            for k in ("repo", "mode", "stage", "diff"):
                if data.get(k) is not None:
                    ctx[k] = data[k]
        elif data.get("runner") is not None:
            ctx["runner"] = data["runner"]
    # pipe_pass: the gate ended the pass ABORTED (store_record_lost, m-n2)
    ctx["lost"] = run in _events.store_lost_runs(db, [run])
    # pipe_flake: the register's rate of every test this run isolated (W8)
    tests = [json.loads(d).get("test") for (d,) in db.execute(
        "SELECT data FROM journal WHERE run=? AND event='flake_isolated'", (run,)) if d]
    if tests:
        marks = ",".join("?" * len(tests))
        ctx["flake_rates"] = {t: (f / r if r else 0.0) for t, f, r in db.execute(
            f"SELECT test, flaky, runs FROM flakes WHERE test IN ({marks})", tests)}
    return ctx


def _station_total(stations):
    if isinstance(stations, dict):
        return sum(int(stations.get(k) or 0) for k in STATION_COUNTS)
    return len(stations or ())


def row_line(row, ctx):
    """The metric line of one journal row (`station_ended`/`pass_ended`/`flake_isolated`)."""
    d, ev = row["data"], row["event"]
    if ev == "station_ended":
        return line("pipe_station", (
            ("repo", ctx.get("repo")), ("mode", ctx.get("mode")), ("station", d["station"]),
            ("runner", ctx.get("runner")), ("verdict", d["verdict"]),
            ("stage", d.get("stage") or ctx.get("stage") or ctx.get("mode")),
        ), (
            ("secs", d["secs"]), ("build_s", d.get("build_s")), ("run_s", d.get("run_s")),
            ("round", d.get("round", 1)), ("cached", 1 if d["verdict"] == "CACHED" else 0),
            ("slow_n", d.get("slow_n", 0)),
        ), row["at"])
    if ev == "pass_ended":
        lost = bool(ctx.get("lost"))  # the gate's verdict, not the stream's (m-n2)
        return line("pipe_pass", (
            ("repo", ctx.get("repo")), ("mode", ctx.get("mode")),
            ("runner", ctx.get("runner")), ("verdict", "ABORTED" if lost else d["verdict"]),
            ("diff", ctx.get("diff") or "other"),
        ), (
            ("secs", d["secs"]), ("lane_seconds", d["lane_seconds"]),
            ("critical_path_s", d["critical_path_s"]), ("rounds", d["rounds"]),
            ("cache_hits", d["cache_hits"]), ("stations", _station_total(d["stations"])),
            ("minimum_s", d.get("minimum_s")), ("x", d.get("x")),
            # passes a flake turned green (panel 6); left out at 0
            ("flaky", (d["stations"].get("flaky") or None)
             if isinstance(d["stations"], dict) else None),
            ("lost", 1 if lost else None),
        ), row["at"])
    if ev == "flake_isolated":
        return line("pipe_flake", (
            ("repo", ctx.get("repo")), ("station", d["station"]), ("test", d["test"]),
        ), (
            ("outcome", 1 if d["verdict"] == "FLAKY" else 0),
            ("rate", d.get("rate", ctx.get("flake_rates", {}).get(d["test"]))),
        ), row["at"])
    return None


def post(target, lines):
    body = ("\n".join(lines) + "\n").encode()
    req = urllib.request.Request(target, data=body, method="POST",
                                 headers={"Content-Type": "text/plain; charset=utf-8"})
    with urllib.request.urlopen(req, timeout=TIMEOUT_S) as resp:
        if resp.status >= 300:
            raise OSError(f"HTTP {resp.status}")


def _lost_pass(db, row):
    """The `pass_ended` row a `store_record_lost` finding corrects, or None."""
    if row["event"] != "process_finding" or not row.get("run") \
            or (row.get("data") or {}).get("rule") != _events.STORE_LOST_RULE:
        return None
    hit = db.execute("SELECT * FROM journal WHERE run=? AND event='pass_ended'"
                     " ORDER BY seq DESC LIMIT 1", (row["run"],)).fetchone()
    if hit is None:
        return None
    out = dict(hit)
    out["data"] = json.loads(out["data"]) if out["data"] else {}
    return out


def hook(db, row):
    """events.HOOKS entry: push the line of a finished station or pass.

    A `store_record_lost` finding pushes its pass's line once more, ABORTED
    with `lost` = 1 at the pass's time (re-check tb-s3-store2 m-n2): the
    GREEN point pushed at ingest stays in the target, and a panel counting
    passes by verdict drops the GREEN points of the runs `lost` names."""
    target = url()
    if not target:
        return
    corrected = _lost_pass(db, row)
    if corrected is not None:
        row = corrected
    elif row["event"] not in SOURCES:
        return
    # The breaker: after the first failure of a run the rest of it is skipped
    # (one timeout per run, not one per event); `push --since` sends it later.
    if db.execute("SELECT 1 FROM journal WHERE run=? AND event='metrics_failed' LIMIT 1",
                  (row["run"],)).fetchone():
        return
    try:
        post(target, [row_line(row, context(db, row["run"]))])
    except Exception as exc:  # noqa: BLE001 -- any failure is an event, never a crash
        _events.emit(db, "metrics_failed", "finding",
                     {"run": row["run"], "url_host": url_host(target),
                      "error": f"{type(exc).__name__}: {exc}"[:200]}, run=row["run"])


def install():
    if hook not in _events.HOOKS:
        _events.HOOKS.append(hook)


def lines(db, since=None, runs=None):
    """The metric lines of every journal row since `at` (or of the given runs)."""
    sql = f"SELECT * FROM journal WHERE event IN ({','.join('?' * len(SOURCES))})"
    args = list(SOURCES)
    if since:
        sql += " AND at >= ?"
        args.append(since)
    if runs is not None:
        runs = list(runs)
        if not runs:
            return []
        sql += f" AND run IN ({','.join('?' * len(runs))})"
        args += runs
    out, ctxs = [], {}
    for r in db.execute(sql + " ORDER BY seq", args):
        row = dict(r)
        row["data"] = json.loads(row["data"]) if row["data"] else {}
        if row["run"] not in ctxs:
            ctxs[row["run"]] = context(db, row["run"])
        out.append(row_line(row, ctxs[row["run"]]))
    return out


def push(target, all_lines):
    for i in range(0, len(all_lines), CHUNK):
        post(target, all_lines[i:i + CHUNK])
    return len(all_lines)


# --- the diff class of a pass (tag `diff`) ----------------------------------

def _path_deps(root, crate):
    toml = pathlib.Path(root) / "crates" / crate / "Cargo.toml"
    try:
        text = toml.read_text()
    except OSError:
        return set()
    text = text.split("[dev-dependencies]")[0]
    return set(re.findall(r'path\s*=\s*"\.\./([A-Za-z0-9_-]+)"', text))


def lib_crates(root, top="meclaw-cells"):
    """`top` and every crate it depends on by path (dev-dependencies left out)."""
    seen, todo = set(), [top]
    while todo:
        crate = todo.pop()
        if crate not in seen:
            seen.add(crate)
            todo += sorted(_path_deps(root, crate) - seen)
    return seen


def diff_class(files, libs):
    """`lib` if the diff carries Rust sources of a lib crate, `small` for at most
    three files in at most one crate, else `other` (V0 section 8)."""
    crates = set()
    for f in files:
        parts = f.split("/")
        if parts[0] == "crates" and len(parts) > 2:
            crates.add(parts[1])
            if parts[1] in libs and parts[2] == "src" and f.endswith(".rs"):
                return "lib"
    if len(files) <= 3 and len(crates) <= 1:
        return "small"
    return "other"


def _cmd_metrics(args):
    target = args.url or url()
    db = _store.open_ro(args.db)
    try:
        out = lines(db, since=args.since)
    finally:
        _store.close(db)
    if args.dry_run:
        for item in out:
            print(item)
        return 0
    if not target:
        print(f"metrics: {URL_ENV} is empty -- nothing pushed", file=sys.stderr)
        return 2
    try:
        push(target, out)
    except Exception as exc:  # noqa: BLE001
        print(f"METRICS pushed=0 url_host={url_host(target)} error={type(exc).__name__}",
              file=sys.stderr)
        return 1
    print(f"METRICS pushed={len(out)} url_host={url_host(target)}")
    return 0


def register(sub):
    """`pipeline metrics push --since <at>` (idempotent: same rows, same points)."""
    p = sub.add_parser("metrics", help="push the metric lines of the journal")
    p.add_argument("action", choices=("push",))
    p.add_argument("--since", default=None, help="RFC 3339 UTC; rows with at >= since")
    p.add_argument("--db", default=None, help="store path (default $MECLAW_PIPELINE_DB)")
    p.add_argument("--url", default=None, help=f"write URL (default ${URL_ENV})")
    p.add_argument("--dry-run", action="store_true", help="print the lines, push nothing")
    p.set_defaults(func=_cmd_metrics)


install()
