"""`pipeline export-orga <wave>|--run <id>` -- a wave (or one pass) in Orga's form.

Two files, nothing is sent anywhere (Orga is not used yet, R-DT-2: the files
are the template and the proof that the pipeline's data is Orga-shaped):

* ``<x>.graph.json`` -- the graph Orga's linter reads (``validate_dag.py``):
  ``{member_ref, basis, items[{id, key, type, state, audience, audience_set,
  due_at, starts_at, ends_at, parent, doc, evidence}], edges[{src, dst, kind,
  data}], parties[]}``.
* ``<x>.runs.json`` -- ``task_runs``, ``task_run_results``, ``journal``
  (``note|finding|status``) and ``calibration`` rows in Orga's column names,
  plus a ``meta`` block (window, counts, what was derived or dropped).

``<x>`` is the wave name, or the pass id for ``--run``. Not ``in_capture``:
its entries carry neither ``doc.exec`` nor edges nor runs (E-DT-21).

THE MAPPING (plan part W11, "Export-Orga-Mapping")

* A workpackage (a pass) stays a ``workpackage``: title ``<repo> <mode>
  <strand>``, state word for word; its ``rev`` becomes a ``commit`` evidence
  entry.
* A station stays a ``task`` with ``doc.exec`` holding Orga's seven keys only
  (``EXEC_KEYS``); ``audience`` is ``self`` everywhere.
* A task without a ``doc.exec`` in the store (the backfill wrote none) gets one
  derived from its station name and pass: ``runner`` = its run's runner,
  ``root`` = ``/<repo>``, ``test_argv`` = the station's first command or
  ``scripts/gate.sh <mode> --only <station>``; ``x_pipeline.derived`` names it.
* Fields Orga does not carry stay in ``doc.x_pipeline`` -- exactly the list of
  E-DT-21 (``X_PIPELINE_FIELDS``) plus the originals of what the export maps
  (``MAPPING_FIELDS``): evidence kinds ``log``/``receipt`` -> ``file`` and
  ``cache`` -> ``external`` (original in ``x_pipeline.evidence``, E-DT-6); a
  ``test_argv`` element over ``EXEC_ARG_MAX`` characters -> ``@argv:<station>``
  (original in ``x_pipeline.argv``, E-DT-24); a ``timeout_min`` over 120 ->
  120 (original in ``x_pipeline.timeout_min``).
* Edges: the store's ``depends_on``/``blocks`` edges between exported items,
  ``depends_on`` data cut to Orga's ``{dep, lag_min, when}``; a station
  without a ``depends_on`` edge follows the station before it in the pass's
  plan with ``when: {outcome: any}`` (a gate runs every station and collects
  the findings). ``belongs_to`` is not exported -- the hierarchy is ``parent``.
* Calibration: the store's rows (minutes rounded up, seconds half up); a task
  without a row gets one derived from its last ``done`` run that was not
  CACHED/SKIP (``x_pipeline.derived`` = ``["calibration"]``; seconds =
  ``build_s + run_s``, else ``ended_at - started_at`` as the backfill wrote them).

The store is opened read-only by the command; ``build`` never writes.
"""

import json
import math
import os
import pathlib
import re
import sys

from . import store

FORMAT = "meclaw.pipeline.orga-export/1"

# Orga's field contract of doc.exec (shared/exec.py), the closed guard grammar
# (README "Conditions on dependencies") and the evidence kinds.
EXEC_KEYS = ("runner", "root", "workspace", "test_argv", "timeout_min", "retries", "evidence")
EVIDENCE_KINDS = ("file", "message", "commit", "fact", "object", "external", "url", "turn")
EXEC_ARG_MAX = 1000
EXEC_ARGV_MAX = 32
EXEC_TIMEOUT = (1, 120)
EXEC_RETRIES = (0, 3)
EXEC_WORKSPACE = re.compile(r"^[A-Za-z0-9-]{1,40}$")
EDGE_DATA_KEYS = ("dep", "lag_min", "when")
JOURNAL_KINDS = ("note", "finding", "status")

# orga-eingaben.md E-DT-21: the fields Orga needs in addition, word for word.
X_PIPELINE_FIELDS = ("needs", "inputs", "stage", "round", "verdict", "cache_from", "keep_ws")
# The originals of what the export maps, and the mark of what it derived.
MAPPING_FIELDS = ("argv", "evidence", "timeout_min", "derived")

# Pipeline evidence kinds Orga does not know (E-DT-6).
KIND_MAP = {"log": "file", "receipt": "file", "cache": "external"}
NODE_MAX = 2000              # Orga's lint scope; a bigger graph is cut there
SIZE_WARN = 5 * 1024 * 1024
CHAIN_GUARD = {"dep": "fs", "lag_min": 0, "when": {"outcome": "any"}}
NO_CALIBRATION = ("CACHED", "SKIP")
COMMIT = re.compile(r"^[0-9a-fA-F]{7,64}$")


def _load(text, default=None):
    if text is None:
        return default
    try:
        return json.loads(text)
    except (TypeError, ValueError):
        return default


def _ceil_min(secs):
    return int(math.ceil(float(secs) / 60.0))


def _half_up(secs):
    return int(math.floor(float(secs) + 0.5))


def _kind(kind):
    if kind in EVIDENCE_KINDS:
        return kind
    return KIND_MAP.get(kind, "external")


def _dedup(seq):
    out = []
    for x in seq:
        if x not in out:
            out.append(x)
    return out


def _stamp(value):
    """A datetime or text -> RFC 3339 UTC text."""
    if value is None or isinstance(value, str):
        return value
    return value.strftime("%Y-%m-%dT%H:%M:%SZ")


def wave_window(wave, db):
    """(since, until) of a wave: `retro.wave_window` (part A) when it is there,
    else from the date in the directory name (UTC midnight) until now."""
    try:
        from . import retro  # noqa: PLC0415 -- part A, optional
        since, until = retro.wave_window(wave, db)
        return _stamp(since), _stamp(until)
    except (ImportError, AttributeError):
        pass
    m = re.search(r"(\d{4}-\d{2}-\d{2})$", wave or "")
    if not m:
        raise ValueError(f"no date in the wave name {wave!r}")
    return f"{m.group(1)}T00:00:00Z", store.now()


def lanes_from_file(path=None):
    """{name: target} of the private host file (`<name> <target> ...` per line)."""
    path = pathlib.Path(path or os.environ.get("MECLAW_LANES_FILE")
                        or pathlib.Path.home() / ".config/meclaw/lanes")
    out = {}
    try:
        text = path.read_text(encoding="utf-8")
    except OSError:
        return out
    for line in text.splitlines():
        parts = line.split("#", 1)[0].split()
        if len(parts) >= 2:
            out[parts[0]] = parts[1]
    return out


def _runner_map(lanes):
    """`local` -> the lane whose target is `local` (this machine), if any."""
    here = [n for n, t in (lanes or {}).items() if t == "local"]
    return {"local": here[0]} if len(here) == 1 else {}


# --- selection --------------------------------------------------------------

def _passes(db, wave, run, since, until):
    if run is not None:
        rows = db.execute(
            "SELECT * FROM items WHERE type='workpackage' AND (id=? OR key=? OR "
            "json_extract(doc,'$.run')=?) ORDER BY created_at, id", (run, run, run)).fetchall()
        if not rows:
            raise LookupError(f"no pass {run!r} in the store")
        return rows
    return db.execute(
        "SELECT * FROM items WHERE type='workpackage' AND ((created_at>=? AND created_at<?) "
        "OR json_extract(doc,'$.wave')=?) ORDER BY created_at, id", (since, until, wave)).fetchall()


def _children(db, ids):
    out = []
    ids = list(ids)
    for i in range(0, len(ids), 500):
        chunk = ids[i:i + 500]
        marks = ",".join("?" * len(chunk))
        out += db.execute(f"SELECT * FROM items WHERE parent IN ({marks}) ORDER BY created_at, id",
                          chunk).fetchall()
    return out


def _rows_in(db, sql, col_values):
    out = []
    vals = list(col_values)
    for i in range(0, len(vals), 500):
        chunk = vals[i:i + 500]
        out += db.execute(sql.format(marks=",".join("?" * len(chunk))), chunk).fetchall()
    return out


# --- mapping ----------------------------------------------------------------

def _evidence_entries(run):
    """A task_runs row's evidence -> Orga evidence entries (kinds mapped)."""
    if run is None:
        return [], []
    raw = _load(run["evidence"])
    raw = raw if isinstance(raw, list) else ([raw] if isinstance(raw, dict) else [])
    entries, kinds = [], []
    for e in raw:
        kind = e.get("kind")
        kinds.append(kind)
        if kind == "cache":
            target = "run:" + str(e.get("run") or run["cache_from"] or "")
        else:
            target = e.get("path") or e.get("target") or e.get("run")
        entry = {"kind": _kind(kind)}
        if target:
            entry["target"] = str(target)
        if e.get("text"):
            entry["text"] = str(e["text"])
        entries.append(entry)
    if not entries and run["cache_from"]:
        entries.append({"kind": "external", "target": f"run:{run['cache_from']}"})
        kinds.append("cache")
    return _dedup(entries), _dedup(kinds)


def _argv(argv, name, xp):
    argv = [str(a) for a in argv if str(a).strip()]
    out = [a if len(a) <= EXEC_ARG_MAX else f"@argv:{name}" for a in argv]
    if len(out) > EXEC_ARGV_MAX:
        out = out[:EXEC_ARGV_MAX - 1] + [f"@argv:{name}"]
    if out != argv:
        xp["argv"] = argv
    return out


def _task(item, wp_doc, runs, runner_map):
    """(doc, evidence, runner) of a station task."""
    doc = _load(item["doc"], {}) if isinstance(item["doc"], str) else (item["doc"] or {})
    doc = doc if isinstance(doc, dict) else {}
    ex = doc.get("exec") if isinstance(doc.get("exec"), dict) else {}
    station = doc.get("station") if isinstance(doc.get("station"), dict) else {}
    name = station.get("name") or (doc["station"] if isinstance(doc.get("station"), str) else None) \
        or item["title"]
    last = runs[-1] if runs else None
    xp, derived = {}, []

    runner = ex.get("runner") or (last["runner"] if last else None) or wp_doc.get("lane") or "unknown"
    if runner in runner_map:
        runner = runner_map[runner]
        derived.append("exec.runner")
    out = {"runner": runner}
    if not ex:
        derived.append("exec")
    root = ex.get("root")
    if not (isinstance(root, str) and root.startswith("/")):
        root = "/" + str(wp_doc.get("repo") or "repo")
    out["root"] = root
    if isinstance(ex.get("workspace"), str) and EXEC_WORKSPACE.match(ex["workspace"]):
        out["workspace"] = ex["workspace"]
    argv = ex.get("test_argv")
    if not (isinstance(argv, list) and argv):
        cmds = station.get("cmds") or []
        argv = cmds[0].get("argv") if cmds and isinstance(cmds[0], dict) else None
    if not (isinstance(argv, list) and argv):
        mode = wp_doc.get("mode") or "strand"
        from . import lane   # the kit's one Python (re-check tv-bn M7m M2, M7p)
        argv = (lane.python_argv("scripts/pipeline.py", "pre") if mode == "pre"
                else ["scripts/gate.sh", mode, "--only", str(name)])
    out["test_argv"] = _argv(argv, name, xp)
    t = ex.get("timeout_min")
    if isinstance(t, (int, float)) and not isinstance(t, bool):
        clamped = int(min(max(int(t), EXEC_TIMEOUT[0]), EXEC_TIMEOUT[1]))
        if clamped != t:
            xp["timeout_min"] = t
        out["timeout_min"] = clamped
    r = ex.get("retries", 0)
    out["retries"] = int(min(max(int(r), EXEC_RETRIES[0]), EXEC_RETRIES[1])) \
        if isinstance(r, (int, float)) and not isinstance(r, bool) else 0

    entries, run_kinds = _evidence_entries(last)
    kinds = ex.get("evidence") if isinstance(ex.get("evidence"), list) and ex.get("evidence") \
        else (run_kinds or ["log"])
    mapped = _dedup(_kind(k) for k in kinds)
    if mapped != list(kinds):
        xp["evidence"] = list(kinds)
    out["evidence"] = mapped

    for field in ("needs", "inputs", "stage", "keep_ws"):
        if station.get(field) is not None:
            xp[field] = station[field]
    if runs:
        xp["round"] = max(int(r["round"] or 1) for r in runs)
        if last["verdict"]:
            xp["verdict"] = last["verdict"]
        if last["cache_from"]:
            xp["cache_from"] = last["cache_from"]
    if derived:
        xp["derived"] = derived
    return {"exec": out, "x_pipeline": xp}, entries, runner


def _workpackage(item, rounds):
    doc = _load(item["doc"], {}) or {}
    repo, mode = doc.get("repo") or "-", doc.get("mode") or "-"
    title = f"{repo} {mode} {doc.get('strand') or '-'}"
    xp = {"stage": doc.get("stage") or mode}
    if rounds:
        xp["round"] = max(rounds)
    evidence = []
    rev = doc.get("rev")
    if isinstance(rev, str) and COMMIT.match(rev):
        evidence.append({"kind": "commit", "target": rev, "text": f"{repo} {mode} rev"})
    return title, {"x_pipeline": xp}, evidence, doc


def _graph_item(row, title, doc, evidence, parent):
    return {"id": row["id"], "key": row["key"], "type": row["type"], "title": title,
            "state": row["state"], "reason": row["reason"], "audience": "self",
            "audience_set": None, "due_at": None, "starts_at": None, "ends_at": None,
            "parent": parent, "doc": doc, "evidence": evidence,
            "created_at": row["created_at"], "updated_at": row["updated_at"]}


def _edge_data(data):
    data = data if isinstance(data, dict) else {}
    return {k: data[k] for k in EDGE_DATA_KEYS if k in data}


def _plan_order(wp_doc, tasks):
    plan = [p for p in (wp_doc.get("plan") or []) if isinstance(p, str)]
    pos = {name: i for i, name in enumerate(plan)}

    def num(t):
        try:
            return int(str(t["key"]).rsplit("-", 1)[1])
        except (IndexError, ValueError):
            return 0
    return sorted(tasks, key=lambda t: (pos.get(t["title"], len(plan)), num(t), t["id"]))


def build(db, wave=None, run=None, since=None, until=None, lanes=None):
    """(graph, runs) of a wave's window or of one pass; reads only."""
    if run is None and (since is None or until is None):
        w_since, w_until = wave_window(wave, db)
        since, until = since or w_since, until or w_until
    runner_map = _runner_map(lanes)
    passes = _passes(db, wave, run, since, until)
    wp_ids = [p["id"] for p in passes]
    tasks = _children(db, wp_ids)
    task_ids = [t["id"] for t in tasks]
    runs = _rows_in(db, "SELECT * FROM task_runs WHERE item IN ({marks}) "
                        "ORDER BY started_at, attempt, id", task_ids)
    by_task = {}
    for r in runs:
        by_task.setdefault(r["item"], []).append(r)

    items, parties, by_parent, pass_runs = [], [], {}, set()
    wp_docs = {}
    for t in tasks:
        by_parent.setdefault(t["parent"], []).append(t)
    for p in passes:
        rounds = [int(r["round"] or 1) for t in by_parent.get(p["id"], [])
                  for r in by_task.get(t["id"], [])]
        title, doc, evidence, raw = _workpackage(p, rounds)
        wp_docs[p["id"]] = raw
        if raw.get("run"):
            pass_runs.add(raw["run"])
        items.append(_graph_item(p, title, doc, evidence, None))
    resolved = {}
    for t in tasks:
        doc, evidence, runner = _task(t, wp_docs[t["parent"]], by_task.get(t["id"], []), runner_map)
        resolved[t["id"]] = runner
        items.append(_graph_item(t, t["title"], doc, evidence, t["parent"]))
        parties.append({"item": t["id"], "who": f"lane:{runner}", "role": "executing"})

    ids = set(wp_ids) | set(task_ids)
    edges, dropped_edges = [], 0
    for e in _rows_in(db, "SELECT * FROM edges WHERE src IN ({marks}) ORDER BY src, dst, kind", ids):
        if e["dst"] not in ids:
            continue
        if e["kind"] not in ("depends_on", "blocks"):
            dropped_edges += 1
            continue
        data = _edge_data(_load(e["data"], {})) if e["kind"] == "depends_on" else None
        edges.append({"src": e["src"], "dst": e["dst"], "kind": e["kind"], "data": data,
                      "_at": e["at"]})
    has_dep = {e["src"] for e in edges if e["kind"] == "depends_on"}
    derived_edges = 0
    for p in passes:
        prev = None
        for t in _plan_order(wp_docs[p["id"]], by_parent.get(p["id"], [])):
            if prev is not None and t["id"] not in has_dep:
                edges.append({"src": t["id"], "dst": prev["id"], "kind": "depends_on",
                              "data": json.loads(json.dumps(CHAIN_GUARD)), "_at": None})
                derived_edges += 1
            prev = t
    stamps = [i["updated_at"] for i in items if i["updated_at"]] + \
        [e["_at"] for e in edges if e["_at"]]
    for e in edges:
        del e["_at"]

    graph = {"member_ref": "human:member", "basis": max(stamps) if stamps else since,
             "items": items, "edges": edges, "parties": parties}

    run_ids = [r["id"] for r in runs]
    task_runs = []
    for r in runs:
        entries, _ = _evidence_entries(r)
        xp = {"round": int(r["round"] or 1), "verdict": r["verdict"]}
        if r["cache_from"]:
            xp["cache_from"] = r["cache_from"]
        task_runs.append({
            "id": r["id"], "item": r["item"], "key": r["key"],
            "runner": runner_map.get(r["runner"], r["runner"]),
            "attempt": r["attempt"], "ws": r["ws"], "state": r["state"],
            "started_at": r["started_at"], "deadline_at": r["deadline_at"],
            "ended_at": r["ended_at"], "reason": r["reason"], "evidence": entries or None,
            "results": _load(r["results"]), "lost": r["lost"] or 0, "x_pipeline": xp})
    results = [{"run": x["run"], "at": x["at"], "call": x["call"], "tool_name": x["tool_name"],
                "entry": _load(x["entry"])}
               for x in _rows_in(db, "SELECT * FROM task_run_results WHERE run IN ({marks}) "
                                     "ORDER BY run, at, call", run_ids)]

    journal, chain_rows = [], 0
    if run is None:
        jrows = db.execute("SELECT * FROM journal WHERE at>=? AND at<? ORDER BY seq",
                           (since, until)).fetchall()
    else:
        jrows = db.execute("SELECT * FROM journal ORDER BY seq").fetchall()
    run_keys = pass_runs | set(run_ids)
    for j in jrows:
        mine = j["item"] in ids or j["run"] in run_keys or \
            (run is None and j["item"] is None and j["run"] is None)
        if not mine:
            continue
        if j["kind"] not in JOURNAL_KINDS:
            chain_rows += 1
            continue
        data = _load(j["data"], {})
        data = data if isinstance(data, dict) else {"value": data}
        data = dict(data, event=j["event"])
        if j["run"] and "run" not in data:
            data["run"] = j["run"]
        journal.append({"id": j["seq"], "item": j["item"], "at": j["at"],
                        "author": "system:pipeline", "author_kind": "agent", "kind": j["kind"],
                        "text": j["text"], "evidence": None, "data": data})

    calibration, have = [], set()
    for c in _rows_in(db, "SELECT * FROM calibration WHERE item IN ({marks}) "
                          "ORDER BY closed_at, key", task_ids):
        have.add(c["item"])
        secs = c["actual_s"]
        est = c["estimate_min"] if c["estimate_min"] is not None else (
            _ceil_min(c["estimate_s"]) if c["estimate_s"] is not None else None)
        calibration.append({
            "item": c["item"], "type": c["type"] or "task",
            "executing": c["executing"] if str(c["executing"] or "").count(":")
            else f"lane:{c['executing']}",
            "estimate_min": None if est is None else int(math.ceil(est)),
            "actual_min": _ceil_min(secs) if secs is not None else (
                None if c["actual_min"] is None else int(math.ceil(c["actual_min"]))),
            "actual_s": _half_up(secs) if secs is not None else None,
            "closed_at": c["closed_at"]})
    for t in tasks:
        if t["id"] in have:
            continue
        done = [r for r in by_task.get(t["id"], []) if r["state"] == "done"
                and r["verdict"] not in NO_CALIBRATION and _run_secs(r) is not None]
        if not done:
            continue
        r = done[-1]
        secs = _run_secs(r)
        calibration.append({"item": t["id"], "type": "task", "executing": f"lane:{resolved[t['id']]}",
                            "estimate_min": None, "actual_min": _ceil_min(secs),
                            "actual_s": _half_up(secs), "closed_at": r["ended_at"],
                            "x_pipeline": {"derived": ["calibration"]}})

    unresolved = sorted({resolved[t] for t in resolved} - set(lanes or {})) if lanes else []
    meta = {"format": FORMAT, "source": "pipeline store, read-only", "wave": wave, "run": run,
            "since": since if run is None else None, "until": until if run is None else None,
            "counts": {"passes": len(passes), "tasks": len(tasks), "edges": len(edges),
                       "edges_derived": derived_edges, "task_runs": len(task_runs),
                       "task_run_results": len(results), "journal": len(journal),
                       "calibration": len(calibration)},
            "dropped": {"edges_not_orga": dropped_edges, "journal_chain": chain_rows},
            "runners_not_in_host_file": unresolved}
    runs_doc = {"meta": meta, "task_runs": task_runs, "task_run_results": results,
                "journal": journal, "calibration": calibration}
    return graph, runs_doc


def _run_secs(r):
    """A run's seconds: build_s + run_s, else ended_at - started_at (the backfill)."""
    if r["run_s"] is not None or r["build_s"] is not None:
        return float(r["run_s"] or 0) + float(r["build_s"] or 0)
    if r["started_at"] and r["ended_at"]:
        try:
            return max(0.0, (store.parse_at(r["ended_at"]) - store.parse_at(r["started_at"]))
                       .total_seconds())
        except ValueError:
            return None
    return None


def _write(path, doc):
    text = json.dumps(doc, indent=1, sort_keys=True) + "\n"
    path.write_text(text, encoding="utf-8")
    return len(text.encode("utf-8"))


def export(db, wave=None, run=None, out_dir=None, since=None, until=None, lanes=None):
    """Write `<x>.graph.json` and `<x>.runs.json` into `out_dir`; returns both paths."""
    if out_dir is None:
        raise ValueError("export needs an out_dir")
    if wave is None and run is None:
        raise ValueError("export needs a wave or a run")
    graph, runs = build(db, wave=wave, run=run, since=since, until=until, lanes=lanes)
    out = pathlib.Path(out_dir)
    out.mkdir(parents=True, exist_ok=True)
    name = run if run is not None else wave
    g, r = out / f"{name}.graph.json", out / f"{name}.runs.json"
    size = _write(g, graph)
    _write(r, runs)
    if size > SIZE_WARN or len(graph["items"]) > NODE_MAX:
        print(f"export-orga: {g} has {size} bytes and {len(graph['items'])} items "
              f"(Orga lints the first {NODE_MAX})", file=sys.stderr)
    return g, r


# --- command ----------------------------------------------------------------

def repo_root():
    return pathlib.Path(__file__).resolve().parents[2]


def _cmd_export(args):
    if not args.wave and not args.run:
        print("export-orga: name a wave or --run <id>", file=sys.stderr)
        return 2
    try:
        db = store.open_ro(args.db)
    except FileNotFoundError as exc:
        print(f"export-orga: {exc}", file=sys.stderr)
        return 1
    try:
        if args.out_dir:
            out = pathlib.Path(args.out_dir)
        elif args.wave:
            out = repo_root() / "plans" / args.wave / "orga-export"
        else:
            out = store.default_path().parent / "orga-export"
        g, r = export(db, wave=args.wave, run=args.run, out_dir=out, since=args.since,
                      until=args.until, lanes=lanes_from_file())
    except (LookupError, ValueError) as exc:
        print(f"export-orga: {exc}", file=sys.stderr)
        return 1
    finally:
        store.close(db)
    graph = json.loads(g.read_text(encoding="utf-8"))
    print(f"EXPORT-ORGA {g} items={len(graph['items'])} edges={len(graph['edges'])} "
          f"bytes={g.stat().st_size}")
    print(f"EXPORT-ORGA {r} bytes={r.stat().st_size}")
    return 0


def register(sub):
    """`pipeline export-orga <wave>|--run <id>` (plan part W11 item 4)."""
    p = sub.add_parser("export-orga", help="write a wave or a pass in Orga's graph form")
    p.add_argument("wave", nargs="?", default=None, help="wave directory name, e.g. welle-x-2026-10-08")
    p.add_argument("--run", default=None, help="one pass: its run id, item id or key")
    p.add_argument("--out-dir", default=None, help="default plans/<wave>/orga-export")
    p.add_argument("--since", default=None, help="window start (default: the wave's)")
    p.add_argument("--until", default=None, help="window end (default: the wave's)")
    p.add_argument("--db", default=None, help="store path (default $MECLAW_PIPELINE_DB)")
    p.set_defaults(func=_cmd_export)


def main(argv=None):
    import argparse  # noqa: PLC0415
    ap = argparse.ArgumentParser(prog="orga_export")
    register(ap.add_subparsers(dest="cmd", required=True))
    args = ap.parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
