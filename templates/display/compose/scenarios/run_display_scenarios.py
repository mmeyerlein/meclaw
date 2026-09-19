#!/usr/bin/env python3
"""The scenarios of display-hive.md § 11 against the curator (`../compose.py`).

    python3 run_display_scenarios.py                 model + curator (pure) + curator (cell)
    python3 run_display_scenarios.py S-012 Q-03      only these ids
    python3 run_display_scenarios.py --model-only    run_model.py against pass.py
    python3 run_display_scenarios.py --pure-only     scenarios against compose.run_pass
    python3 run_display_scenarios.py --cell-only     scenarios through compose.pass_read
    python3 run_display_scenarios.py --subprocess     one scenario, compose.py per pass
    python3 run_display_scenarios.py --race           two writes on one stale state row

Three numbers, one station (development-rules § 10):

    MODEL   116/116   run_model.py against pass.py      (the document against itself)
    PURE    116/116   scenarios against compose.run_pass (the verbatim copy inside the cell)
    CURATOR 114/114   scenarios through pass_read        (the cell's real entry; Q-07 and
                                                          S-062 are colony-only -> SKIP)

Exit 0 only when every number is full. The cell mode carries the state row and the
object tree between passes exactly as the store and the display would.
"""
import importlib.util
import json
import os
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
COMPOSE = os.path.join(HERE, "..", "compose.py")
COLONY_ONLY = {"Q-07": "710_the_screen_hears_only_what_is_said.rs",
               "S-062": "710_the_screen_hears_only_what_is_said.rs"}


def load(path, name):
    spec = importlib.util.spec_from_file_location(name, path)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def resolve(fixtures, given):
    if isinstance(given, str):
        given = {"fixture": given}
    settings, screens, setup = {}, {}, []
    if given.get("fixture"):
        s, c, u = resolve(fixtures, fixtures[given["fixture"]])
        settings.update(s)
        screens.update(c)
        setup.extend(u)
    settings.update(given.get("settings") or {})
    screens.update(given.get("screens") or {})
    setup.extend(given.get("setup") or [])
    return settings, screens, setup


def evaluate(model, state, key):
    name, arg = key, None
    if "(" in key:
        name, rest = key.split("(", 1)
        arg = rest[:-1] or None
    fn = getattr(model, name)
    return fn(state, arg) if arg is not None else fn(state)


def norm(value):
    """Tuples answer a list in JSON; floats compare rounded (like `run_model.py`)."""
    if isinstance(value, float):
        return round(value, 4)
    if isinstance(value, (list, tuple)):
        return [norm(v) for v in value]
    if isinstance(value, dict):
        return {k: norm(v) for k, v in value.items()}
    return value


def check(model, state, expect):
    out = []
    for key, exp in (expect or {}).items():
        try:
            got = evaluate(model, state, key)
        except Exception as e:  # noqa: BLE001 -- a helper that raises is a finding
            out.append("%s raised %r" % (key, e))
            continue
        if norm(exp) != norm(got):
            out.append("%s expected %s got %s"
                       % (key, json.dumps(norm(exp)), json.dumps(norm(got))))
    return out


# --- pure: the scenarios against compose.run_pass ----------------------------

def run_pure(compose, fixtures, sc):
    settings, screens, setup = resolve(fixtures, sc["given"])
    state = compose.empty_state(settings, screens)
    for step in setup:
        state = compose.run_pass(state, step["event"], step["now"])
    failures = []
    for i, step in enumerate(sc.get("steps") or []):
        state = compose.run_pass(state, step["event"], step["now"])
        failures += ["step %d: %s" % (i + 1, f) for f in check(compose, state, step.get("expect"))]
    return failures + check(compose, state, sc.get("expect"))


# --- cell: the scenarios through pass_read, rows + state row + tree carried --

class Cell:
    """What the store and the display hold between two passes of the cell."""

    def __init__(self, compose, settings, screens):
        self.c = compose
        self.rows = {}          # (owner, view_id) -> row, the `views` table
        self.state_row = None   # the one state row (OR-H2)
        self.have = {}          # the object tree the display holds
        self.params = dict(settings)
        self.params["screens"] = screens
        self.last = None        # the last state pass_read computed (for the helpers)
        self.last_plan = None   # the plan of the last pass (for --subprocess)
        self.state_rows = []    # the state row of every pass (for --subprocess)

    def oid_parts(self, oid):
        # scenario ids are `view.<owner>.<view_id>`: the cell's own id shape
        _, owner, view_id = oid.split(".", 2)
        return owner, view_id

    def one_pass(self, event, now):
        c = self.c
        c.read_knobs(self.params)
        kind = event["kind"]
        if kind == "app_write":
            owner, view_id = self.oid_parts(event["oid"])
            sent = dict(event.get("view") or {})
            ttl = sent.pop("ttl_ms", 0)
            children = sent.pop("children", None)
            row = {"owner": owner, "view_id": view_id, "region": "main", "ord": 0,
                   "kind": "component",
                   "content": json.dumps(c.window_node(sent, children)),
                   "components": "[]", "ttl_ms": ttl, "updated_at": now}
            self.rows[(owner, view_id)] = row
        elif kind == "app_withdraw":
            owner, view_id = self.oid_parts(event["oid"])
            self.rows.pop((owner, view_id), None)
        plan = {"views": list(self.rows.values()), "state": self.state_row,
                "define": [], "now": now, "event": event}
        body = {"messages": [{"origin": "tool", "type": "tool_result", "id": "d-query",
                              "text": json.dumps({"objects": list(self.have.values())})}]}
        self.last_plan = plan
        ctx = {"display_origin": "read", "display_views": json.dumps(plan, sort_keys=True)}
        emissions = c.pass_read(body, ctx)
        for em in emissions:
            head = em.get("header") or {}
            route = head.get("route") or em.get("route")
            if route == "patch":
                for call in c.calls_of(em):
                    c.apply_call(self.have, call)
            if route == "views" and json.loads(head.get("display_request") or "{}").get("state"):
                self.state_row = c.state_row_of(em)
        self.state_rows.append(self.state_row)
        self.last = c.last_state()   # the state pass_read computed, kept for the helpers
        return self.last


def run_cell(compose, fixtures, sc):
    settings, screens, setup = resolve(fixtures, sc["given"])
    cell = Cell(compose, settings, screens)
    for step in setup:
        cell.one_pass(step["event"], step["now"])
    failures = []
    state = cell.last or compose.empty_state(settings, screens)
    for i, step in enumerate(sc.get("steps") or []):
        state = cell.one_pass(step["event"], step["now"])
        failures += ["step %d: %s" % (i + 1, f) for f in check(compose, state, step.get("expect"))]
    return failures + check(compose, state, sc.get("expect"))


# --- subprocess: the same scenario, compose.py spawned once per pass -------
#
# The import path and the subprocess path must answer the same, because the colony runs
# the second one: a `code` cell spawns the script with the pass's document on stdin. Warm,
# a spawn costs about 42 ms, so this runs ONE scenario rather than all of them.

SUBPROCESS_DEFAULT = "Q-20"


def run_subprocess(fixtures, sc):
    """Every pass of one scenario through `python3 compose.py`, as state rows."""
    compose = load(COMPOSE, "compose_sub")
    settings, screens, setup = resolve(fixtures, sc["given"])
    cell = Cell(compose, settings, screens)
    out = []
    for step in list(setup) + list(sc.get("steps") or []):
        # The in-process cell keeps the rows and the tree; the subprocess is handed the
        # same plan and the same body, and only its answer is read.
        doc = {"params": cell.params,
               "body": {"messages": [{"origin": "tool", "type": "tool_result", "id": "d-query",
                                      "text": json.dumps({"objects": list(cell.have.values())})}]},
               "envelope": {"header": {"hop": {"operation": "query"}, "context": None}}}
        cell.one_pass(step["event"], step["now"])
        doc["envelope"]["header"]["context"] = {
            "display_origin": "read",
            "display_views": json.dumps(cell.last_plan, sort_keys=True)}
        proc = subprocess.run([sys.executable, os.path.abspath(COMPOSE)],
                              input=json.dumps(doc), capture_output=True, text=True)
        if proc.returncode != 0:
            return ["compose.py exited %d: %s" % (proc.returncode, proc.stderr.strip()[:200])]
        answer = json.loads(proc.stdout or "[]")
        emissions = answer if isinstance(answer, list) else [answer]
        row = None
        for em in emissions:
            head = em.get("header") or {}
            if head.get("route") == "views" and json.loads(head.get("display_request") or "{}").get("state"):
                row = compose.state_row_of(em)
        out.append(row)
    failures = []
    for i, row in enumerate(out):
        mine = cell.state_rows[i]
        if (row or {}).get("content") != (mine or {}).get("content"):
            failures.append("pass %d: the subprocess computed another state" % (i + 1))
    return failures


# --- race: two writes in one breath, both handed the SAME state row ----------
#
# The model assumes sequential passes; the cell promises it (`reconcile`, OR-H0.9). The
# scenarios never test it, because a scenario IS a sequence -- so this stands beside them:
# the second write is given the state row of the pass BEFORE the first one, the way a
# colony hands it over when two writes are in the air at once.

RACE_SETTINGS = {"default_screen": "monitor"}
RACE_SCREENS = {"monitor": {"display_type": "monitor", "inputs": ["pointer"]}}


def run_race(compose):
    cell = Cell(compose, RACE_SETTINGS, RACE_SCREENS)
    first = {"kind": "app_write", "oid": "view.ambient.clock",
             "view": {"title": "Clock", "context": "ambient", "relevance": "0.9",
                      "topic": "clock"}}
    second = {"kind": "app_write", "oid": "view.ambient.weather",
              "view": {"title": "Weather", "context": "ambient", "relevance": "0.9",
                       "topic": "weather:berlin"}}
    stale = cell.state_row                       # none at all: the pass before the first
    cell.one_pass(first, 1000)
    cell.state_row = stale                       # the race: the second is handed it again
    state = cell.one_pass(second, 1100)
    failures = []
    for oid, at in (("view.ambient.clock", 1000), ("view.ambient.weather", 1100)):
        view = state["views"].get(oid)
        if view is None:
            failures.append("%s is gone from the state" % oid)
            continue
        if view["curator"]["since"] != at:
            failures.append("%s since %s, expected %s" % (oid, view["curator"]["since"], at))
        if not compose.in_state(view):
            failures.append("%s is not computed" % oid)
    return failures


def main(argv):
    only = {a for a in argv if not a.startswith("--")}
    with open(os.path.join(HERE, "scenarios.json")) as f:
        doc = json.load(f)
    fixtures, scs = doc.get("fixtures") or {}, doc["scenarios"]
    rc = 0
    if "--pure-only" not in argv and "--cell-only" not in argv:
        model = load(os.path.join(HERE, "run_model.py"), "run_model")
        n = sum(1 for _ in scs)
        rc |= model.main(["run_model.py"] + sorted(only))
        print("MODEL %s" % ("see above" if only else "%d/%d" % (n, n) if rc == 0 else "FAIL"))
    if "--model-only" in argv:
        return rc
    if "--race" in argv:
        compose = load(COMPOSE, "compose")
        failures = run_race(compose)
        print(("FAIL RACE: %s" % "; ".join(failures)) if failures
              else "RACE ok -- two writes on one stale state row lose no window")
        return rc | int(bool(failures))
    if "--subprocess" in argv:
        compose = load(COMPOSE, "compose")
        ids = only or {SUBPROCESS_DEFAULT}
        for sc in scs:
            if sc["id"] not in ids:
                continue
            failures = run_subprocess(fixtures, sc)
            print(("FAIL %s: %s" % (sc["id"], "; ".join(failures))) if failures
                  else "SUBPROCESS %s ok -- import and subprocess answer the same" % sc["id"])
            rc |= int(bool(failures))
        return rc
    compose = load(COMPOSE, "compose")
    for label, runner, skip in (("PURE", run_pure, {}), ("CURATOR", run_cell, COLONY_ONLY)):
        if label == "PURE" and "--cell-only" in argv:
            continue
        if label == "CURATOR" and "--pure-only" in argv:
            continue
        total = passed = 0
        for sc in scs:
            if only and sc["id"] not in only:
                continue
            if sc["id"] in skip:
                print("SKIP %s %s -- colony only: %s" % (sc["id"], sc["title"], skip[sc["id"]]))
                continue
            total += 1
            failures = runner(compose, fixtures, sc)
            if failures:
                print("FAIL %s %s: %s" % (sc["id"], sc["title"], "; ".join(failures)))
            else:
                passed += 1
                print("PASS %s %s" % (sc["id"], sc["title"]))
        print("%s %d/%d" % (label, passed, total))
        rc |= int(passed != total)
    return rc


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
