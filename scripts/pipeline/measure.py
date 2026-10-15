"""meclaw pipeline -- `measure minimum`: what our hardware can do at best.

    python3 scripts/pipeline.py measure minimum [--lane <name>] [--only A3,A5] [--report <path>]

WHY: a target number without a floor is a guess. This module measures
ATOMS -- one build or test step alone on an empty lane, three times, the
median counts -- and computes from them the MINIMUM of every gate stage:
the critical path over the stage's parallel branches. The acceptance form
of the pipeline is then `actual <= minimum * (1 + x)`, x being the
unavoidable overhead share (ssh, process start, receipt) plus a 10 % margin.

WHAT IT WRITES: one row per atom (`stage` = `atom:<id>`) and -- with
`--atoms-from <tsv>`, the medians of the whole atom set in stage keys, which
the freshly measured atoms override -- one row per stage into the store table
`minimum` of the W0 store (`store.open`), one
`minimum_measured` journal event per row through `events.emit`, and -- when `MECLAW_PIPELINE_METRICS_URL` is set -- one
`pipe_minimum` line per stage. A metrics failure never fails the run.

FOREIGN LOAD: a run that starts while something else keeps the lane busy
measures that something, not the floor. Before every run the lane's
`/proc/loadavg` is read; more than two runnable processes besides ours
(field 4) reject the run, and so does a load1 above 2 at the very first
run of a pass (later runs see the decay of our own previous build in load1,
which is not foreign). A rejected run is kept in the report, never counted.

No test of this module starts cargo: every lane call goes through one
`runner` callable that the tests replace.
"""
from __future__ import annotations

import datetime as _dt
import json
import os
import pathlib
import shlex
import statistics
import subprocess
import sys
import urllib.request

from . import events as _events
from . import store as _store

FOREIGN_LOAD_MAX = 2.0
MARGIN = 0.10
DEFAULT_ROOT = os.environ.get("MECLAW_LANE_ROOT", "/srv/target")
REPO_ROOT = pathlib.Path(__file__).resolve().parents[2]

# --- the atoms the command can re-measure ------------------------------------
# Each atom is a prep step (untimed) and one timed shell command, run in the
# lane's tree with the measurement target. Filtersets come from
# `gate_plan.py --format json` of the reference diffs (see `diff_filter`).
ATOMS = {
    "A3": {
        "prep": "touch crates/meclaw-cells/tests/misc_1/gh1103_a_brokered_channel_takes_its_hive_s_handles.rs",
        "cmd": "cargo nextest run --workspace --no-run",
        "timeout": 1200,
    },
    "A2:cells": {
        "prep": "touch crates/meclaw-cells/src/lib.rs",
        "cmd": "cargo nextest run --workspace --no-run",
        "timeout": 1800,
    },
    "A5": {
        "prep": "",
        "cmd": "cargo nextest run --workspace --no-fail-fast -j {nproc} -E {filter}",
        "timeout": 1800,
        "diffs": ["ab1103", "zkfix", "py"],
    },
}

# Reference diffs of A5 (`<base>..<rev>` in this repository).
REFERENCE_DIFFS = {
    "ab1103": "262e2265fcce4a52c27ecd9c8d6f0b7577c7acaa..2b588b0964e66902ff4af74abac68e25d9cce584",
    "zkfix": "cd9c3dde12baeac88a5bc55e637c30c85c1545de..9e01403b167f98334d928bbbca5b22b027e243c0",
    "py": "1e3f78cb5^..1e3f78cb5",
}


def now() -> str:
    return _dt.datetime.now(_dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


# --- pure arithmetic ----------------------------------------------------------

def median_of_three(values: list[float]) -> float:
    """The median of exactly three accepted runs; never a mean."""
    if len(values) != 3:
        raise ValueError(f"median_of_three needs three values, got {len(values)}")
    return statistics.median(values)


def foreign_load(sample: dict, first: bool) -> bool:
    """True when `sample` ({load1, running}) shows load that is not ours."""
    if sample.get("running", 0) > FOREIGN_LOAD_MAX:
        return True
    return first and sample.get("load1", 0.0) > FOREIGN_LOAD_MAX


def critical_path(durations: dict[str, float], after: dict[str, list[str]]) -> tuple[float, list[str]]:
    """Longest path through a DAG: `after[n]` are the nodes n waits for.

    Nodes without an edge run in parallel from the start, so the minimum of a
    stage is its longest chain, never the sum of its stations.
    """
    memo: dict[str, tuple[float, list[str]]] = {}

    def best(n: str, seen: tuple = ()) -> tuple[float, list[str]]:
        if n in seen:
            raise ValueError(f"cycle at {n}")
        if n not in memo:
            prev = [best(p, seen + (n,)) for p in after.get(n, [])]
            base = max(prev, default=(0.0, []), key=lambda t: t[0])
            memo[n] = (base[0] + durations[n], base[1] + [n])
        return memo[n]

    return max((best(n) for n in durations), default=(0.0, []), key=lambda t: t[0])


def x_from_overhead(overhead_s: float, minimum_s: float, margin: float = MARGIN) -> float:
    """x = unavoidable overhead / minimum + margin, rounded to two places."""
    if minimum_s <= 0:
        raise ValueError("minimum must be positive")
    return round(overhead_s / minimum_s + margin, 2)


def stage_graph(stage: str, a: dict[str, float]) -> tuple[dict[str, float], dict[str, list[str]]]:
    """Durations and `after` edges of one stage from the atom medians `a`.

    Keys of `a`: A1, A3, A2:<crate>, A2sel, A2c, A4:nproc, A5:<diff>, A6:<station>,
    A7:dry|warm|audit_full, A8:browser_max|memory_max|builder_max (the slowest
    single case, never a station sum), A9:max. A missing atom is a missing
    branch (0 s). `merge-cold` is the upper bound of a merge (no cache: cold
    build, then the zk-fix subset); its lower bound is the strand stage.
    """
    g = lambda k: a.get(k, 0.0)  # noqa: E731
    py = max([v for k, v in a.items() if k.startswith("A6:") and k[3:] in PY_FORM] or [0.0])
    if stage == "pre":
        return {"form": py, "check": g("A2c"), "own": g("A5:py")}, {}
    if stage == "strand-small":
        return ({"build": g("A3"), "run": g("A5:ab1103"), "clippy": g("A6:clippy"), "python": py},
                {"run": ["build"]})
    if stage == "strand-lib":
        return ({"build": g("A2sel") or g("A2:cells"), "run": g("A5:zkfix"), "clippy": g("A6:clippy"), "python": py},
                {"run": ["build"]})
    if stage == "merge-cold":
        return {"build": g("A1"), "run": g("A5:zkfix")}, {"run": ["build"]}
    if stage == "integration":
        return ({"build": g("A2:cells"), "run": g("A5:zkfix"), "browser": g("A8:browser_max"),
                 "memory": g("A8:memory_max"), "python": g("A9:max"), "export-dry": g("A7:dry")},
                {"run": ["build"]})
    if stage == "release":
        return ({"build": g("A2:cells"), "run": g("A4:nproc"), "export": g("A7:warm") + g("A7:audit_full"),
                 "labs": max(g("A8:browser_max"), g("A8:memory_max"), g("A8:builder_max")),
                 "python": g("A9:max"), "deny": g("A6:deny"), "doctests": g("A6:doctests")},
                {"run": ["build"]})
    if stage == "rerun":
        return {"build": g("A3"), "run": g("A5:py")}, {"run": ["build"]}
    raise ValueError(f"unknown stage {stage}")


PY_FORM = ("precheck", "roadmap-anchors", "adr-anchors", "claims", "tree-rules", "catalogue", "shellcheck")
STAGES = ("pre", "strand-small", "strand-lib", "merge-cold", "integration", "release", "rerun")
OVERHEAD_ATOMS = ("A10:pre", "A10:lane")


def stage_minimum(stage: str, atoms: dict[str, float], overhead_s: float) -> dict:
    dur, after = stage_graph(stage, atoms)
    secs, path = critical_path(dur, after)
    return {"stage": stage, "seconds": round(secs, 2), "critical_path": path,
            "overhead_s": overhead_s, "x": x_from_overhead(overhead_s, secs) if secs > 0 else None}


def load_atoms(path: pathlib.Path) -> dict[str, float]:
    """`<key>\t<seconds>` lines (blank and `#` lines skipped) -> atom medians in stage keys."""
    atoms = {}
    for line in pathlib.Path(path).read_text().splitlines():
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        key, secs = line.split("\t")[:2]
        atoms[key.strip()] = float(secs)
    return atoms


def stage_rows(atoms: dict[str, float], source: str = "") -> list[dict]:
    """One row per stage with data; overhead A10:pre for `pre` (local), A10:lane for the rest."""
    missing = [k for k in OVERHEAD_ATOMS if k not in atoms]
    if missing:
        raise ValueError(f"stage rows need the overhead atoms {', '.join(missing)}")
    rows = []
    for stage in STAGES:
        row = stage_minimum(stage, atoms, atoms["A10:pre"] if stage == "pre" else atoms["A10:lane"])
        if row["seconds"] > 0:
            rows.append(dict(row, source=source))
    return rows


# --- store, journal, metrics --------------------------------------------------

def write_rows(db, rows: list[dict], runner: str, repo: str = "meclaw") -> None:
    """One `minimum` row and one `minimum_measured` event per row, one transaction."""
    at = _store.now()
    with _store.tx(db):
        for r in rows:
            db.execute("INSERT INTO minimum (stage, runner, repo, seconds, critical_path, overhead_s, x,"
                       " measured_at, source) VALUES (?,?,?,?,?,?,?,?,?)",
                       (r["stage"], runner, repo, r["seconds"], json.dumps(r.get("critical_path", [])),
                        r.get("overhead_s"), r.get("x"), at, r.get("source", "")))
            data = {"stage": r["stage"], "runner": runner, "seconds": r["seconds"],
                    "critical_path": r.get("critical_path", []), "overhead_s": r.get("overhead_s"), "x": r.get("x")}
            _events.emit(db, "minimum_measured", "status", data, at=at)


def influx_lines(rows: list[dict], runner: str, repo: str = "meclaw") -> list[str]:
    ns = int(_dt.datetime.now(_dt.timezone.utc).timestamp() * 1e9)
    out = []
    for r in rows:
        if r["stage"].startswith("atom:"):
            continue
        fields = [f"seconds={r['seconds']}"]
        if r.get("overhead_s") is not None:
            fields.append(f"overhead_s={r['overhead_s']}")
        if r.get("x") is not None:
            fields.append(f"x={r['x']}")
        out.append(f"pipe_minimum,repo={repo},stage={r['stage']},runner={runner} {','.join(fields)} {ns}")
    return out


def push_metrics(lines: list[str]) -> str | None:
    url = os.environ.get("MECLAW_PIPELINE_METRICS_URL")
    if not url or not lines:
        return None
    try:
        req = urllib.request.Request(url.rstrip("/") + "/write?db=devtempo", data="\n".join(lines).encode())
        urllib.request.urlopen(req, timeout=10).read()
        return None
    except Exception as e:  # metrics never block a run
        return str(e)


# --- lanes and runs -------------------------------------------------------------

def lane_target(name: str) -> str:
    f = pathlib.Path(os.environ.get("MECLAW_LANES_FILE") or pathlib.Path.home() / ".config/meclaw/lanes")
    for line in f.read_text().splitlines():
        w = line.split("#", 1)[0].split()
        if w and w[0] == name:
            return w[1]
    raise SystemExit(f"measure: no lane {name} in {f}")


def ssh_runner(target: str):
    def run(script: str, timeout: int) -> tuple[int, str]:
        from . import lane  # the word check of the narrow point (OR-TV.tv-bn.M7e-1)
        try:
            # the script travels on stdin, so its words are checked here; the
            # argv comes from the narrow point (re-check tv-bn M7e B-M1)
            lane.remote_words_known(script)
            argv = lane.ssh_argv(target, "bash -s")
        except lane.LaneError as exc:
            raise SystemExit(f"measure: {exc}")
        p = subprocess.run(argv, input=script,
                           capture_output=True, text=True, timeout=timeout + 60)
        return p.returncode, p.stdout + p.stderr
    return run


def timed_script(tree: str, target_dir: str, prep: str, cmd: str, log: str) -> str:
    """The lane side of one run: load sample, untimed prep, timed command.
    The verdict's names set here travel (GH #1210, `lane_sync.sh VERDICT_ENV`):
    ssh carries no environment, and a browser proof without
    MECLAW_REQUIRE_BROWSERS SKIPs green -- a short run, a floor of nothing.
    The switch is on unless the caller opted out (GH #1209, `lane.test_run_env`;
    the opt-out is a NOTE on stderr and, re-check M7c M1, a mark in the
    atom's result and its report row -- `measure_atom`)."""
    from . import lane  # the kit's one list
    env, note = lane.test_run_env()
    if note:
        print(note, file=sys.stderr)
    words = "".join(" " + w for w in lane.verdict_env_words(env))
    return (f"export PATH=$HOME/.cargo/bin:$PATH CARGO_TARGET_DIR={shlex.quote(target_dir)} TMPDIR=/dev/shm/meclaw-tests{words}\n"
            f"mkdir -p $TMPDIR {shlex.quote(os.path.dirname(log))}; cd {shlex.quote(tree)} || exit 9\n"
            "echo M0_LOAD $(cut -d' ' -f1,4 /proc/loadavg)\n"
            f"{prep}\n"
            f"s=$EPOCHREALTIME; ( {cmd} ) >{shlex.quote(log)} 2>&1; rc=$?; e=$EPOCHREALTIME\n"
            "echo M0_TIME $s $e $rc\n")


def parse_run(out: str) -> dict:
    res: dict = {}
    for line in out.splitlines():
        w = line.split()
        if w[:1] == ["M0_LOAD"] and len(w) >= 3:
            res["load1"] = float(w[1])
            res["running"] = int(w[2].split("/")[0]) - 1
        if w[:1] == ["M0_TIME"] and len(w) >= 4:
            res["secs"] = round(float(w[2]) - float(w[1]), 2)
            res["rc"] = int(w[3])
    return res


# A run shorter than this measured an empty command (M0 nit: 0.01 s, rc 0).
MIN_RUN_S = 0.1


def measure_atom(atom_id: str, cmd: str, prep: str, timeout: int, runner, tree: str, target_dir: str,
                 log_dir: str, runs: int = 3, first: bool = False) -> dict:
    """`runs` accepted runs of one atom; rejected runs are retried once each.

    A run is rejected under foreign load, with a non-zero exit code, or when it
    took no time (an empty run measures nothing). Fewer accepted runs than
    `runs` give no median -- a floor from a partial pass is no floor.
    """
    from . import lane  # the kit's one rule of the browser switch
    accepted, rejected, logs = [], [], []
    browsers = lane.browsers_opt_out()   # re-check M7c M1: in the result, not only on stderr
    attempt = 0
    while len(accepted) < runs and attempt < runs * 2:
        attempt += 1
        log = f"{log_dir}/{atom_id.replace(':', '-')}-{attempt}.log"
        rc, out = runner(timed_script(tree, target_dir, prep, cmd, log), timeout)
        r = parse_run(out)
        if "secs" not in r:
            raise RuntimeError(f"{atom_id}: lane run gave no time (rc {rc}): {out[-400:]}")
        r["log"] = log
        if r.get("rc", 0) != 0 or r["secs"] <= 0:
            rejected.append(dict(r, reason="failed"))
            continue
        if r["secs"] < MIN_RUN_S:  # M0 nit: a constant command, nothing measured
            rejected.append(dict(r, reason="empty"))
            continue
        if foreign_load(r, first and attempt == 1):
            rejected.append(dict(r, reason="foreign_load"))
            continue
        accepted.append(r)
        logs.append(log)
    vals = [r["secs"] for r in accepted]
    if len(vals) < runs:
        med = None
    else:
        med = median_of_three(vals) if runs == 3 else statistics.median(vals)
    return {"atom": atom_id, "median": med, "values": vals, "rejected": rejected, "logs": logs,
            "browsers": browsers}


def diff_filter(name: str, repo: pathlib.Path = REPO_ROOT) -> str:
    rng = REFERENCE_DIFFS[name]
    files = subprocess.run(["git", "-C", str(repo), "diff", "--name-only", rng],
                           capture_output=True, text=True, check=True).stdout
    plan = subprocess.run([sys.executable, str(repo / "scripts/gate_plan.py"), "--mode", "strand",
                           "--files-from", "-", "--format", "json"], input=files,
                          capture_output=True, text=True, check=True).stdout
    tests = [s for s in json.loads(plan)["stations"] if s["name"] == "tests"]
    if not tests:
        raise SystemExit(f"measure: diff {name} plans no tests station")
    return tests[0]["cmd"][2]


# --- report -----------------------------------------------------------------------

def report_table(results: list[dict], lane: str) -> str:
    """Atom | Lane | Median | three values | source log -- every row names its logs."""
    out = ["| Atom | Lane | Median (s) | Values (s) | Source |", "|---|---|---|---|---|"]
    for r in results:
        logs = r.get("logs") or [x["log"] for x in r.get("rejected") or [] if x.get("log")]
        if not logs:
            raise ValueError(f"{r['atom']}: a number without a log")
        vals = " / ".join(f"{v:.1f}" for v in r["values"])
        med = "—" if r["median"] is None else f"{r['median']:.1f}"
        rej = f" (rejected: {len(r['rejected'])})" if r.get("rejected") else ""
        rej += f" {r['browsers']}" if r.get("browsers") else ""
        out.append(f"| {r['atom']} | {lane} | {med} | {vals}{rej} | `{logs[0]}` … |")
    return "\n".join(out) + "\n"


# --- command ------------------------------------------------------------------------

def measure_atoms(args) -> list[dict]:
    """Re-measure the atoms of `--only` on the lane; one result per atom (and A5 diff)."""
    lane = args.lane
    runner = ssh_runner(lane_target(lane))
    tree = args.tree or f"{args.root}/wt/{args.strand}"
    target_dir = args.target_dir or f"{args.root}/bench/{args.strand}/target"
    log_dir = f"{args.root}/bench/{args.strand}/logs/measure-{now().replace(':', '')}"
    only = [s.strip() for s in args.only.split(",")] if args.only else list(ATOMS)
    _, out = runner("nproc", 30)
    nproc = int(out.split()[0])
    results, first = [], True
    for atom in only:
        spec = ATOMS.get(atom)
        if spec is None:
            raise SystemExit(f"measure: unknown atom {atom} (known: {', '.join(ATOMS)})")
        for d in spec.get("diffs", [None]):
            cmd = spec["cmd"].format(nproc=nproc, filter=shlex.quote(diff_filter(d)) if d else "")
            aid = f"{atom}:{d}" if d else atom
            res = measure_atom(aid, cmd, spec["prep"], spec["timeout"], runner, tree, target_dir, log_dir,
                               runs=args.runs, first=first)
            first = False
            results.append(res)
            print(f"measure: {aid} median {res['median']} s {res['values']}"
                  + (f" {res['browsers']}" if res.get("browsers") else ""), flush=True)
    return results


def cmd_minimum(args) -> int:
    if args.stages_only and not args.atoms_from:
        raise SystemExit("measure: --stages-only needs --atoms-from")
    atoms = load_atoms(pathlib.Path(args.atoms_from)) if args.atoms_from else {}
    results = [] if args.stages_only else measure_atoms(args)
    rows = [{"stage": f"atom:{r['atom']}", "seconds": r["median"], "critical_path": r["values"],
             "source": r["logs"][0] if r["logs"] else ""} for r in results if r["median"] is not None]
    atoms.update({r["atom"]: r["median"] for r in results if r["median"] is not None})
    if args.atoms_from:
        rows += stage_rows(atoms, source=args.atoms_from)
    db = _store.open()
    try:
        write_rows(db, rows, f"lane:{args.lane}")
    finally:
        _store.close(db)
    lines = influx_lines(rows, args.lane)
    print(f"measure: {len(rows)} rows, {len(lines)} pipe_minimum lines", flush=True)
    err = push_metrics(lines)
    if err:
        print(f"measure: metrics push failed ({err}); the store has the rows", file=sys.stderr)
    if results:
        table = report_table(results, args.lane)
        if args.report:
            pathlib.Path(args.report).write_text(table)
        print(table, end="")
    return 0


def register(sub) -> None:
    p = sub.add_parser("measure", help="measure the floor of every gate stage (M0)")
    s = p.add_subparsers(dest="measure_cmd", required=True)
    m = s.add_parser("minimum", help="re-measure atoms on an empty lane, median of three")
    m.add_argument("--lane", required=True, help="lane name from the host file")
    m.add_argument("--only", default="", help="comma list of atoms (default: all re-measurable)")
    m.add_argument("--report", default="", help="write the markdown table here")
    m.add_argument("--runs", type=int, default=3)
    m.add_argument("--atoms-from", default="",
                   help="TSV <key>\\t<seconds> of all atom medians in stage keys; adds one row per stage")
    m.add_argument("--stages-only", action="store_true",
                   help="measure nothing, write the stage rows from --atoms-from (no lane contact)")
    m.add_argument("--root", default=DEFAULT_ROOT, help="lane root (default $MECLAW_LANE_ROOT or /srv/target)")
    m.add_argument("--strand", default="m0", help="names wt/<strand> and bench/<strand>/ on the lane")
    m.add_argument("--tree", default="", help="tree on the lane (default <root>/wt/<strand>)")
    m.add_argument("--target-dir", default="", help="cargo target on the lane (default <root>/bench/<strand>/target)")
    m.set_defaults(func=cmd_minimum)

