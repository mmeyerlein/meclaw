"""Receipt v2: the gate receipt as a projection of a run's journal events.

The receipt is never written beside the events; ``build(db, run)`` projects
it from the journal alone, and ``project(rows)`` does the same over a stream,
so the receipt a stream produced and the one rebuilt later are the same
document (locked by a test).

Every v1 key stays as it was -- ``mode rev base dirty lock_wait_secs archive
plan started finished stations verdict owner_decisions`` -- because the export
(``make_export.py``) reads ``rev dirty mode verdict plan stations``.
``dirty`` comes from the required field of ``pass_planned`` and a station's
``scope`` from the required field of ``station_ended`` (OR-DT.W0.6);
``lock_wait_secs``, ``archive``, ``narrowing`` and ``testing_ruling`` (the
``MECLAW_TESTING_RULES_RULING`` of the pass, R-TV-W21 c) are options of
``pass_planned`` with the defaults of ``events.OPTIONAL``. Added:
``schema`` (``meclaw.gate.receipt/2``), ``repo``, ``lane``, ``run``,
``rounds``, ``critical_path_s``, ``lane_seconds``, ``cache_hits``,
``fingerprint``, ``minimum``, ``plan_reason``, and the two the summary lines
need: ``secs`` (the pass) and ``cache_known`` (stations the plan expected to
serve from the cache).

``stations`` is the state of the LAST round each station ran in; every round
is kept in ``rounds``. A v1 reader takes ``legacy(receipt)``: CACHED reads
as GREEN there (with ``cache_from``), FLAKY as RED (with ``flaky``) -- a flake
is a finding (OR-TV-2, VG TV-F1) -- except a FLAKY station with ``legacy:
true`` (every test on ``.config/flaky-legacy.toml``, VG TV-F4), which reads
GREEN; an unknown overall verdict (ABORTED) stays what it is and
is never green to that reader.

The JSON schema is ``receipt-v2.schema.json`` beside this file.
"""

import copy
import json
import pathlib
import sys

from . import events as _events

SCHEMA = "meclaw.gate.receipt/2"
SCHEMA_FILE = pathlib.Path(__file__).with_name("receipt-v2.schema.json")
V1_KEYS = ("mode", "rev", "base", "dirty", "lock_wait_secs", "archive", "plan",
           "started", "finished", "stations", "verdict", "owner_decisions")
GREEN_VERDICTS = ("GREEN", "CACHED")


def counts_green(station):
    """GREEN, CACHED, or a FLAKY station whose tests are all legacy (VG TV-F1/F4)."""
    return station.get("verdict") in GREEN_VERDICTS or (
        station.get("verdict") == "FLAKY" and station.get("legacy") is True)


def _empty():
    return {
        "schema": SCHEMA,
        "mode": None, "rev": None, "base": None, "dirty": False, "lock_wait_secs": 0,
        "archive": None, "plan": [], "started": None, "finished": None,
        "stations": [], "verdict": "RUNNING", "owner_decisions": [],
        "repo": None, "lane": None, "run": None, "rounds": [], "secs": None,
        "critical_path_s": None, "lane_seconds": None, "cache_hits": 0,
        "cache_known": 0, "fingerprint": None, "minimum": None, "plan_reason": {},
        "testing_ruling": None,
    }


def _round_verdict(stations):
    # A FLAKY station is a finding (VG TV-F1): it makes its round RED unless legacy.
    seen = {"RED" if s["verdict"] == "FLAKY" and not counts_green(s) else s["verdict"]
            for s in stations}
    for verdict in ("RED", "ASK", "ABORTED"):
        if verdict in seen:
            return verdict
    return "GREEN"


def _add_tests(total, more):
    if not more:
        return total
    total = dict(total or {"total": 0, "passed": 0, "failed": 0, "flaky": 0, "skipped": 0})
    for key in total:
        total[key] += int(more.get(key) or 0)
    return total


def project(rows, ledgers=()):
    """Receipt v2 from event rows (journal rows or stream lines) in order.

    Without a `store_record_lost` finding in the rows the run's line in a
    ledger (the archive root of its `pass_planned`, plus `ledgers`) reads the
    pass ABORTED (re-check tb-s3-store2 m-n3)."""
    doc = _empty()
    rounds = {}          # n -> round dict
    current = None       # the round events land in
    log, tests, parts, cache_from = {}, {}, {}, {}
    last = {}            # station -> its station dict of the last round
    order = []
    hits = 0
    lost = False         # a `store_record_lost` finding of this run (M-2, #1161)

    def round_for(n, run, at):
        nonlocal current
        if n not in rounds:
            rounds[n] = {"n": n, "run": run, "started": at, "finished": None,
                         "stations": [], "verdict": "RUNNING"}
        current = rounds[n]
        return current

    for row in rows:
        event, data, at = row["event"], row.get("data") or {}, row["at"]
        run = data.get("run") or row.get("run")
        if event == "pass_planned":
            doc.update(mode=data["mode"], repo=data["repo"], rev=data["rev"],
                       base=data["base"], plan=list(data["stations"]),
                       plan_reason=data["reason"], cache_known=data["cache_known"],
                       # the coordinator's clock, before guard and push (R3 M5)
                       started=at, run=run, dirty=data["dirty"],
                       **{k: data.get(k, v)
                          for k, v in _events.OPTIONAL["pass_planned"].items()})
            if data.get("full_pass"):  # a full pass after odd effects (full_pass, #1165)
                doc["full_pass"] = data["full_pass"]
            if data.get("graph_build_secs") is not None:  # 3a (#1199): the auto build
                doc["graph_build_secs"] = data["graph_build_secs"]
        elif event == "run_dispatched":
            doc["lane"] = data["runner"]
            round_for(data["round"], run, at)
        elif event == "lane_fingerprint":
            doc["fingerprint"] = data
        elif event == "station_started":
            name = data["station"]
            tests.pop(name, None)
            parts.pop(name, None)
            cache_from.pop(name, None)
        elif event == "station_log":
            log[data["station"]] = data["path"]
        elif event == "test_result":
            name = data["station"]
            tests[name] = _add_tests(tests.get(name), data.get("tests"))
            parts[name] = parts.get(name, 0) + 1
        elif event == "cache_hit":
            hits += 1
            cache_from[data["station"]] = data["from_run"]
        elif event == "station_ended":
            name = data["station"]
            rnd = current or round_for(1, run, at)
            station = {
                "name": name, "scope": data["scope"],
                "secs": data["secs"], "verdict": data["verdict"], "log": log.get(name),
                "build_s": data.get("build_s"), "run_s": data.get("run_s"),
                "round": rnd["n"],
                "cache_from": data.get("cache_from", cache_from.get(name)),
                "tests": tests.get(name), "parts": parts.get(name, 0),
            }
            for extra in ("reason", "in_total", "isolated", "exit", "cached_parts", "legacy",
                          "cause"):
                if extra in data:
                    station[extra] = data[extra]
            rnd["stations"] = [s for s in rnd["stations"] if s["name"] != name] + [station]
            rnd["finished"] = at
            if name not in last:
                order.append(name)
            last[name] = station
        elif event == "owner_decision":
            doc["owner_decisions"].append({"station": data["station"],
                                           "decision": data["decision"],
                                           "reason": data["reason"], "at": at})
        elif event == "minimum_measured":
            doc["minimum"] = {"stage": data["stage"], "seconds": data["seconds"],
                              "x": data["x"]}
        elif event == "pass_ended":
            doc.update(verdict=data["verdict"], finished=at, secs=data["secs"],
                       lane_seconds=data["lane_seconds"],
                       critical_path_s=data["critical_path_s"])
            hits = data["cache_hits"]
        elif event == "process_finding" and data.get("rule") == _events.STORE_LOST_RULE:
            lost = True
    if not lost and doc.get("run") and doc.get("verdict") not in (None, "ABORTED"):
        paths = _events.lost_ledgers([doc.get("archive")]) + list(ledgers)
        lost = doc["run"] in _events.ledger_runs(paths)
    if lost:  # the gate's verdict, not the stream's: the pass does not count
        doc["verdict"] = "ABORTED"
    for rnd in rounds.values():
        rnd["verdict"] = _round_verdict(rnd["stations"])
    doc["rounds"] = [rounds[n] for n in sorted(rounds)]
    doc["stations"] = [last[name] for name in order]
    doc["cache_hits"] = hits
    return doc


def dumps(doc):
    """THE serializer of a receipt (#1152): the archive and `pipeline receipt`
    write the same bytes -- sorted keys, two-space indent, a final newline."""
    return json.dumps(doc, indent=2, sort_keys=True) + "\n"


def build(db, run):
    """`pipeline receipt <run>`: the receipt from the journal (and the ledger m-n3)."""
    return project(_events.read_run(db, run), _events.lost_ledgers(db=db))


def legacy(doc):
    """The v1 reading of a v2 receipt: CACHED reads GREEN, FLAKY RED unless legacy."""
    out = copy.deepcopy(doc)
    for stations in [out["stations"]] + [r["stations"] for r in out.get("rounds", [])]:
        for station in stations:
            if station["verdict"] == "CACHED":
                station["verdict"] = "GREEN"
            elif station["verdict"] == "FLAKY":
                station["verdict"] = "GREEN" if station.get("legacy") is True else "RED"
                station["flaky"] = True
    return out


def validate(doc, schema_file=SCHEMA_FILE):
    """Check a receipt against the required keys and types of the schema file."""
    schema = json.loads(pathlib.Path(schema_file).read_text(encoding="utf-8"))
    errors = _check(doc, schema, "receipt")
    return errors


_TYPES = {"object": dict, "array": list, "string": str, "boolean": bool,
          "integer": int, "number": (int, float), "null": type(None)}


def _check(value, schema, where):
    errors = []
    types = schema.get("type")
    if types:
        types = types if isinstance(types, list) else [types]
        ok = any(isinstance(value, _TYPES[t]) and not (t in ("integer", "number")
                                                       and isinstance(value, bool))
                 for t in types)
        if not ok:
            return [f"{where}: {type(value).__name__} is not {'|'.join(types)}"]
    if "enum" in schema and value not in schema["enum"]:
        errors.append(f"{where}: {value!r} not in {schema['enum']}")
    if isinstance(value, dict):
        for key in schema.get("required", []):
            if key not in value:
                errors.append(f"{where}: missing {key}")
        for key, sub in schema.get("properties", {}).items():
            if key in value:
                errors += _check(value[key], sub, f"{where}.{key}")
    if isinstance(value, list) and "items" in schema:
        for i, item in enumerate(value):
            errors += _check(item, schema["items"], f"{where}[{i}]")
    return errors


def _secs(value):
    """Whole seconds for the summary lines: int(x + 0.5), halves go up.

    Not ``round()``: Python rounds halves to the even neighbour (2.5 -> 2).
    """
    if value is None:
        return 0
    return int(value + 0.5)


def gate_lines(doc):
    """The `GATE` lines and the `GATE-SUMMARY` line, as scripts/gate.sh prints them.

    `<green>/<total>` counts judgements: GREEN (and CACHED, and a legacy
    FLAKY, which are green) and RED (and every other FLAKY, VG TV-F1); SKIP/NOTE/ASK/ABORTED are in neither half, except a
    station marked `in_total` (a browser station without its browser).
    """
    lines, green, total = [], 0, 0
    for st in doc["stations"]:
        reason = st.get("reason") or ""
        lines.append(f"GATE {st['name']} [{st.get('scope', '')}] {_secs(st['secs'])}s "
                     f"{st['verdict']}{' ' + reason if reason else ''}")
        if counts_green(st):
            green += 1
            total += 1
        elif st["verdict"] in ("RED", "FLAKY") or st.get("in_total"):
            total += 1
    rev = str(doc.get("rev") or "")[:7]
    lines.append(f"GATE-SUMMARY {doc['mode']} {rev} {green}/{total} "
                 f"{_secs(doc.get('secs'))}s {doc['verdict']}")
    return lines


def summary_lines(doc, reason_path=None):
    """GATE-PLAN, the GATE lines, GATE-SUMMARY and GATE-LANE (V0 section 5)."""
    rev = str(doc.get("rev") or "")[:7]
    head = (f"GATE-PLAN {doc['mode']} {rev} stations={len(doc.get('plan') or [])} "
            f"cached={doc.get('cache_known') or 0} reason={reason_path or '-'}")
    tail = (f"GATE-LANE {doc.get('lane') or '-'} crit={_secs(doc.get('critical_path_s'))} "
            f"lane_s={_secs(doc.get('lane_seconds'))} rounds={len(doc.get('rounds') or [])}")
    return [head] + gate_lines(doc) + [tail]


def _cmd_receipt(args):
    from . import store
    try:
        db = store.open_ro(args.db)
    except FileNotFoundError as exc:
        print(f"receipt: {exc}", file=sys.stderr)
        return 1
    try:
        rows = _events.read_run(db, args.run)
        ledgers = _events.lost_ledgers(db=db)
    finally:
        store.close(db)
    if not rows:
        print(f"receipt: no events for {args.run}", file=sys.stderr)
        return 1
    doc = project(rows, ledgers)
    if args.legacy:
        doc = legacy(doc)
    if args.lines:
        print("\n".join(summary_lines(doc)))
    else:
        sys.stdout.write(dumps(doc))
    return 0


def register(sub):
    """`pipeline receipt <run>` (V0 section 9)."""
    p = sub.add_parser("receipt", help="build the v2 receipt of a run from the journal")
    p.add_argument("run")
    p.add_argument("--db", default=None, help="store path (default $MECLAW_PIPELINE_DB)")
    p.add_argument("--lines", action="store_true", help="print the summary lines")
    p.add_argument("--legacy", action="store_true", help="print the v1 reading")
    p.set_defaults(func=_cmd_receipt)
