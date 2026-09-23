#!/usr/bin/env python3
"""The scenarios of display-hive.md § 11 against the curator (`../compose.py`).

    python3 run_display_scenarios.py                  every stage below
    python3 run_display_scenarios.py S-012 Q-03       only these ids (MODEL/PURE/CURATOR)
    python3 run_display_scenarios.py --model-only     run_model.py against pass.py
    python3 run_display_scenarios.py --pure-only      scenarios against compose.run_pass
    python3 run_display_scenarios.py --curator-only   scenarios through the resident cell
    python3 run_display_scenarios.py --rebuild-only   a killed cell draws the same screen
    python3 run_display_scenarios.py --rebuild-write-only  a killed cell woken by a write
    python3 run_display_scenarios.py --snapshot-only  a drawn tree needs no further call
    python3 run_display_scenarios.py --hops-only      one select, one read, one patch a step
    python3 run_display_scenarios.py --idle-only      strokes alone write nothing
    python3 run_display_scenarios.py --boot-only      writes during the boot are kept
    python3 run_display_scenarios.py --repair-only    a refused patch is repaired by one read

The numbers, one station (development-rules § 10):

    MODEL    116/116  run_model.py against pass.py      (the document against itself)
    PURE     116/116  scenarios against compose.run_pass (the verbatim copy inside the cell)
    CURATOR  114/114  scenarios through the cell, driven like a `resident` code cell
                      (`curator_driver.Hive`: compile once, exec per message into one
                      dict, the store and the display in-process; Q-07 and S-062 are
                      colony-only -> SKIP)
    REBUILD  114/114  after the last step: one stroke on the running cell, and the same
                      stroke on a KILLED cell over a copy of the store and the tree -- the
                      display ends up byte-equal, and the killed one asked select, read,
                      then patches (GH #809: "restart = one read")
    REBUILD-WRITE  n/n  the same, woken by the scenario's last app write on a standing
                      window -- once as it was, once touched anew -- instead of a stroke:
                      the boot select goes out before the write, the curator's memory of
                      that window survives (scenarios with no standing window -> SKIP)
    BOOT, REPAIR, SNAPSHOT, HOPS, IDLE: the mechanics of GH #809 (plan D1a § 4, T3-T8)

Exit 0 only when every number is full.
"""
import copy
import importlib.util
import json
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
COMPOSE = os.path.join(HERE, "..", "compose.py")
sys.path.insert(0, HERE)
from curator_driver import Hive  # noqa: E402

COLONY_ONLY = {"Q-07": "710_the_screen_hears_only_what_is_said.rs",
               "S-062": "710_the_screen_hears_only_what_is_said.rs"}
TRIGGERS = ("app_write", "app_withdraw", "verdict", "tap", "hold", "stroke")
HEADER_MAX = 512
DOOR_CODES = ("view_refused", "invalid_view")
MODES = ("--model-only", "--pure-only", "--curator-only", "--rebuild-only",
         "--rebuild-write-only", "--snapshot-only",
         "--hops-only", "--idle-only", "--boot-only", "--repair-only")


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


def params_of(settings, screens):
    params = dict(settings)
    params["screens"] = screens
    return params


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


# --- curator: the scenarios through the resident cell --------------------------

class Run:
    """One scenario through a `Hive`, with what every later stage reads off it."""

    def __init__(self, compose, fixtures, sc):
        self.compose = compose
        self.sc = sc
        settings, screens, self.setup = resolve(fixtures, sc["given"])
        self.settings, self.screens = settings, screens
        self.params = params_of(settings, screens)
        self.hive = Hive(self.params)
        self.failures = []
        self.last_now = 0
        self.steps = []      # (hops before, hops after) per step, setup included
        self.writes = []     # the app writes the cell took, in order (REBUILD-WRITE)

    def one(self, step, label):
        event, now = step["event"], step["now"]
        before_state = copy.deepcopy(self.hive.state())
        mark = len(self.hive.hops)
        out = self.hive.send_event(event, now)
        self.steps.append((mark, len(self.hive.hops)))
        self.last_now = max(self.last_now, now)
        state = self.hive.state()
        door = [em.get("receipt") or {} for em in out
                if (em.get("header") or {}).get("route") == "receipt"
                and (em.get("receipt") or {}).get("error_code") in DOOR_CODES]
        if event.get("kind") == "app_write" and door:
            # The write lane asks the pass's own door (`door_refusal` runs `step2_door`),
            # so a word § 4.6 refuses never reaches the store or a pass: the app hears a
            # receipt. Checked here -- the receipt, no hop, the state untouched -- and the
            # expectations are the model's own pass over the same state.
            if len(self.hive.hops) != mark:
                self.failures.append("%s: a refused write reached the store" % label)
            if state != before_state:
                self.failures.append("%s: a refused write moved the state" % label)
            base = before_state or self.compose.empty_state(self.settings, self.screens)
            state = self.compose.run_pass(base, event, now)
            if not any(entry[0] == event.get("oid") for entry in state["pass"]["refused"]):
                self.failures.append("%s: the cell refused %s, the model's door did not"
                                     % (label, door[0].get("detail")))
            return state
        if event.get("kind") == "app_write":
            self.writes.append(copy.deepcopy(event))
        if state is None:
            self.failures.append("%s: the cell holds no state in memory" % label)
            return self.compose.empty_state()
        if event.get("kind") not in TRIGGERS:
            # § 4.1: nothing else triggers a pass. The cell's answer to such an event is
            # "no pass" -- no internal hop and the state untouched, checked here -- and the
            # expectations are the model's own answer to it (`run_pass` with a non-trigger
            # returns the state with `runs: False`).
            if len(self.hive.hops) != mark:
                self.failures.append("%s: a non-trigger reached the store or the display" % label)
            if state != before_state:
                self.failures.append("%s: a non-trigger moved the state" % label)
            state = self.compose.run_pass(state, event, now)
        return state

    def run(self):
        for i, step in enumerate(self.setup):
            self.one(step, "setup %d" % (i + 1))
        state = self.hive.state() or self.compose.empty_state()
        for i, step in enumerate(self.sc.get("steps") or []):
            state = self.one(step, "step %d" % (i + 1))
            self.failures += ["step %d: %s" % (i + 1, f)
                              for f in check(self.compose, state, step.get("expect"))]
        self.failures += check(self.compose, state, self.sc.get("expect"))
        return self.failures


def run_curator(compose, fixtures, sc):
    return Run(compose, fixtures, sc).run()


# --- rebuild: a killed cell draws the same screen (T1) ---------------------------

AGE_ONLY = []   # scenarios that needed the second stroke (OR-D19), for the report


def diff_of(a, b):
    return sorted(k for k in set(a) | set(b) if a.get(k) != b.get(k))


def run_rebuild(compose, fixtures, sc):
    run = Run(compose, fixtures, sc)
    run.run()
    hive = run.hive
    twin = Hive(run.params)
    twin.table, twin.objects = copy.deepcopy(hive.table), copy.deepcopy(hive.objects)
    twin.pages, twin.components = copy.deepcopy(hive.pages), copy.deepcopy(hive.components)
    twin.kill()
    at = run.last_now + 1000
    hive.send_event({"kind": "stroke"}, at)
    twin.send_event({"kind": "stroke"}, at)
    failures = []
    routes = [h["route"] for h in twin.hops]
    if (routes[:2] != ["views", "read"] or twin.hops[0]["ops"] != ["select"]
            or any(r != "patch" for r in routes[2:])):
        failures.append("the killed cell asked %s, not select, read, patches" % routes)
    if "display.root" not in twin.objects:
        # an empty display on both sides is equal and proves nothing
        return failures + ["the killed cell drew no screen at all"]
    if hive.objects == twin.objects:
        return failures
    first = diff_of(hive.objects, twin.objects)
    # OR-D19: a window that became fresh or started leaving in the last pass before the
    # kill may differ in `age` for one stroke; one second later both must agree.
    hive.send_event({"kind": "stroke"}, at + 1000)
    twin.send_event({"kind": "stroke"}, at + 1000)
    if hive.objects == twin.objects:
        AGE_ONLY.append((sc["id"], first[:3]))
        return failures
    diff = diff_of(hive.objects, twin.objects)
    return failures + ["the rebuilt screen differs in %d objects: %s"
                       % (len(diff), "; ".join("%s %s != %s" % (k, json.dumps(hive.objects.get(k))[:160],
                                                                 json.dumps(twin.objects.get(k))[:160])
                                               for k in diff[:2]))]


# --- rebuild-write: a killed cell woken by an app write keeps its memory (C1) -------

def living_write(run):
    """The scenario's last app write whose window still stands at the end, or None."""
    views = (run.hive.state() or {}).get("views") or {}
    for event in reversed(run.writes):
        v = views.get(event.get("oid"))
        if isinstance(v, dict) and not v.get("withdrawn"):
            return event
    return None


def touched_later(event, at):
    """The same write, touched again: `touched` above what it said and above `at`."""
    out = copy.deepcopy(event)
    view = out.setdefault("view", {})
    try:
        said = int(float(view.get("touched")))
    except (TypeError, ValueError):
        said = 0
    view["touched"] = str(max(at, said + 1))
    return out


def rebuild_write_case(compose, fixtures, sc, touch):
    run = Run(compose, fixtures, sc)
    run.run()
    event = living_write(run)
    if event is None:
        return None
    hive = run.hive
    twin = Hive(run.params)
    twin.table, twin.objects = copy.deepcopy(hive.table), copy.deepcopy(hive.objects)
    twin.pages, twin.components = copy.deepcopy(hive.pages), copy.deepcopy(hive.components)
    twin.kill()
    at = run.last_now + 1000
    if touch:
        event = touched_later(event, at)
    hive.send_event(event, at)
    twin.send_event(event, at)
    failures = []
    routes = [h["route"] for h in twin.hops]
    if not routes or routes[0] != "views" or twin.hops[0]["ops"] != ["select"]:
        # the boot's select goes out BEFORE the write's bundle: it reads the store as it
        # was before the restart, so the queued write is a real pass over the real state
        failures.append("the killed cell asked %s, not the boot select first" % routes)
    for step in (2000, 3000):
        hive.send_event({"kind": "stroke"}, run.last_now + step)
        twin.send_event({"kind": "stroke"}, run.last_now + step)
    if "display.root" not in twin.objects:
        return failures + ["the killed cell drew no screen at all"]
    if hive.objects == twin.objects and hive.table == twin.table:
        return failures
    diff = diff_of(hive.objects, twin.objects)
    if not diff:
        return failures + ["the store differs after the rebuild"]
    return failures + ["%s: the rebuilt screen differs in %d objects: %s"
                       % ("touched" if touch else "same", len(diff),
                          "; ".join("%s %s != %s" % (k, json.dumps(hive.objects.get(k))[:160],
                                                     json.dumps(twin.objects.get(k))[:160])
                                    for k in diff[:2]))]


def run_rebuild_write(compose, fixtures, sc):
    """The last app write of the scenario, once as it was and once touched anew, reaches a
    running cell and a KILLED one over a copy of the store and the tree; two strokes later
    both screens and both stores agree. None when no window stands at the end."""
    same = rebuild_write_case(compose, fixtures, sc, False)
    if same is None:
        return None
    return same + rebuild_write_case(compose, fixtures, sc, True)


# --- snapshot: a drawn tree needs no further call (T3) ---------------------------

def run_snapshot(compose, fixtures, sc):
    run = Run(compose, fixtures, sc)
    failures = []
    for i, step in enumerate(list(run.setup) + list(sc.get("steps") or [])):
        run.one(step, "step %d" % (i + 1))
        hive = run.hive
        if hive.state() is None:
            continue
        want, pages = hive.fn("draw")(hive.ram(), step["now"], "")[:2]
        snap = hive.fn("snapshot")(want)
        calls = hive.fn("patches")(want, snap, [], False)
        calls += hive.fn("page_ops")(want, snap, pages, False)
        if calls:
            failures.append("step %d: %d calls against its own snapshot, first %s"
                            % (i + 1, len(calls), json.dumps(calls[0])[:120]))
        # and the tree the display holds IS that snapshot's drawing
        if hive.ram().get("have") is not None and hive.ram().get("phase") == "live":
            held = {k: {"props": v["props"], "parent": v["parent"], "ord": v["ord"],
                        "component": v["component"]} for k, v in hive.objects.items()}
            mine = hive.ram()["have"]
            if any(held.get(k) is None or held[k]["parent"] != v["parent"]
                   or held[k]["ord"] != v["ord"]
                   or any(held[k]["props"].get(p) != x for p, x in v["props"].items())
                   for k, v in mine.items()):
                failures.append("step %d: the cell's mirror is not what the display holds"
                                % (i + 1))
    return failures


# --- hops: one select and one read per hive, at most one patch per step (T4, T5) ---

def run_hops(compose, fixtures, sc):
    run = Run(compose, fixtures, sc)
    run.run()
    hops = run.hive.hops
    failures = []
    selects = sum(1 for h in hops if h["route"] == "views" and "select" in h["ops"])
    reads = sum(1 for h in hops if h["route"] == "read")
    if selects != 1:
        failures.append("%d selects, not 1" % selects)
    if reads != 1:
        failures.append("%d reads, not 1" % reads)
    for n, (a, b) in enumerate(run.steps):
        patches = sum(1 for h in hops[a:b] if h["route"] == "patch")
        if patches > 1:
            failures.append("step %d sent %d patches" % (n + 1, patches))
    for h in hops:
        size = len(json.dumps(h["header"]))
        if size > HEADER_MAX:
            failures.append("a %s header of %d B" % (h["route"], size))
        if "display_views" in h["header"]:
            failures.append("a %s header carries display_views" % h["route"])
    return failures


def largest_header(compose, fixtures, scs):
    top = (0, "")
    for sc in scs:
        if sc["id"] in COLONY_ONLY:
            continue
        run = Run(compose, fixtures, sc)
        run.run()
        for h in run.hive.hops:
            size = len(json.dumps(h["header"]))
            if size > top[0]:
                top = (size, "%s %s" % (sc["id"], h["route"]))
    return top


# --- idle: strokes alone write nothing (T6, OR-D22) ------------------------------

IDLE_PARAMS = {"default_screen": "tv",
               "screens": {"tv": {"display_type": "tv", "viewing_distance_m": 3.0,
                                  "physical_size_in": 55, "inputs": []}}}


def idle_windows(hive, at):
    for i, name in enumerate(("clock", "weather", "notes")):
        hive.send_event({"kind": "app_write", "oid": "view.ambient.%s" % name,
                         "view": {"title": name, "context": "ambient", "relevance": "0.6",
                                  "pinned": i == 0, "topic": name}}, at + i)


def app_rows(table):
    return sorted((r["owner"], r["view_id"]) for r in table
                  if (r["owner"], r["view_id"]) != ("display", "screen-rest"))


def rest_only(hop):
    return hop["route"] == "views" and (hop["request"] or {}).get("rest") \
        and "write" not in (hop["request"] or {})


def store_writes(hops):
    return [h for h in hops if h["route"] == "views" and "select" not in h["ops"]]


def run_idle(judge):
    params = dict(IDLE_PARAMS, judge="on" if judge else "off")
    hive = Hive(params)
    idle_windows(hive, 1000)
    if judge:
        # an app touch just before the strokes: the pass that calls the judge (OR-D22)
        hive.send_event({"kind": "app_write", "oid": "view.ambient.weather",
                         "view": {"title": "weather", "context": "ambient", "relevance": "0.6",
                                  "topic": "weather", "touched": "5000"}}, 5000)
    table = copy.deepcopy(hive.table)
    mark = len(hive.hops)
    for n in range(20):
        hive.send_event({"kind": "stroke"}, 6000 + (n + 1) * 60000)
    failures = []
    writes = store_writes(hive.hops[mark:])
    if writes:
        failures.append("%d store writes over 20 strokes (first %s)"
                        % (len(writes), json.dumps(writes[0]["request"])))
    if hive.table != table:
        failures.append("the table moved over 20 strokes")
    if "display.root" not in hive.objects:
        failures.append("nothing was drawn, so nothing was proven")
    return failures


# --- boot: writes during the boot are kept (T7) ----------------------------------

def run_boot():
    hive = Hive(IDLE_PARAMS, hold_internal=True)
    view = {"title": "A", "context": "conversation", "relevance": "0.9"}
    hive.send_event({"kind": "app_write", "oid": "view.alex.a", "view": view}, 1000)
    hive.send_event({"kind": "app_write", "oid": "view.alex.b",
                     "view": dict(view, title="B")}, 1100)
    failures = []
    ids = app_rows(hive.table)
    if ids != [("alex", "a"), ("alex", "b")]:
        failures.append("before the flush the table holds %s" % ids)
    mark = len(hive.hops)
    hive.flush()
    after = hive.hops[mark:]
    selects = [h for h in hive.hops if "select" in h["ops"]]
    reads = [h for h in after if h["route"] == "read"]
    patches = [h for h in after if h["route"] == "patch"]
    if len(selects) != 1:
        failures.append("%d selects" % len(selects))
    if len(reads) != 1 or len(patches) != 1:
        failures.append("%d reads and %d patches after the flush" % (len(reads), len(patches)))
    for oid in ("view.alex.a", "view.alex.b"):
        if not any(k == oid or k.startswith(oid + "/") for k in hive.objects):
            failures.append("%s is not on the display" % oid)
    ids = app_rows(hive.table)
    if ids != [("alex", "a"), ("alex", "b")]:
        failures.append("the table holds %s" % ids)
    return failures


def boot_retry_hive(fail):
    """Two new windows inside the boot window; with `fail`, the first boot select fails."""
    # the judge on: an app write that is a first write asks it (source (a)); a replayed
    # row and its repeat would not -- that is what the retry has to keep apart
    hive = Hive(dict(IDLE_PARAMS, judge="on"), hold_internal=True)
    view = {"title": "A", "context": "conversation", "relevance": "0.9", "touched": "1000"}
    hive.send_event({"kind": "app_write", "oid": "view.alex.a", "view": view}, 1000)
    if fail:
        selects = [i for i, h in enumerate(hive.held)
                   if (h["envelope"]["header"]["context"].get("display_request") or "")
                   == json.dumps({"boot": "rows"})]
        failed = hive.held[selects[0]]
        failed["body"] = {"messages": []}
        failed["envelope"]["header"]["hop"] = {"operation": "select", "rows_affected": 0,
                                               "duration_ms": 0, "error_code": "db_error"}
        hive.flush()                     # the select fails: the cell is cold again
    hive.send_event({"kind": "app_write", "oid": "view.alex.b",
                     "view": dict(view, title="B", touched="1100")}, 1100)
    hive.flush()
    return hive


def run_boot_retry():
    """A boot select that fails is asked again by the next message, and that select reads
    the writes queued before it: those are left out of the replay (`seen`), so their own
    passes are first writes again -- the screen is the one of a boot that never failed."""
    hive, control = boot_retry_hive(True), boot_retry_hive(False)
    failures = []
    selects = [h for h in hive.hops if "select" in h["ops"]]
    if len(selects) != 2:
        failures.append("%d selects, not the failed one and its retry" % len(selects))
    if hive.objects != control.objects:
        diff = diff_of(hive.objects, control.objects)
        failures.append("the screen after the retry differs from one without it in %s"
                        % ", ".join(diff[:3]))
    if hive.state()["views"] != control.state()["views"]:
        failures.append("the curator's views differ from a boot that never failed")
    if hive.state()["judge"] != control.state()["judge"]:
        failures.append("the judge was asked %s, not %s as by a boot that never failed"
                        % (json.dumps(hive.state()["judge"], sort_keys=True),
                           json.dumps(control.state()["judge"], sort_keys=True)))
    return failures


def run_boot_row_gone():
    """A row that left the store while the cell was down (OR-D.D1.12): the boot knows the
    window only from the rest row, and without its row there are no hints to draw a
    `leaving` frame with -- so the boot takes it off the display at once, no `leaving`
    in between. Withdrawn through the door, the same window leaves over a `leaving` pass
    (707_the_state_is_reconciled_with_the_store holds that half)."""
    hive = Hive(IDLE_PARAMS)
    view = {"title": "A", "context": "conversation", "relevance": "0.9"}
    hive.send_event({"kind": "app_write", "oid": "view.alex.a", "view": view}, 1000)
    hive.send_event({"kind": "app_write", "oid": "view.alex.b",
                     "view": dict(view, title="B")}, 1100)
    failures = []
    if "view.alex.b" not in hive.objects:
        return ["B never stood on the display"]
    hive.kill()
    hive.table_del("alex", "b")
    mark = len(hive.hops)
    hive.send_event({"kind": "stroke"}, 5000)
    patches = [h for h in hive.hops[mark:] if h["route"] == "patch"]
    if len(patches) != 1:
        return ["%d patches after the boot, not 1" % len(patches)]
    leaving = [c for c in patches[0]["calls"]
               if str(c.get("id") or "").startswith("view.alex.b")
               and (c.get("props") or {}).get("age") == "leaving"]
    if leaving:
        failures.append("the boot drew B leaving: %s" % json.dumps(leaving[0])[:200])
    if any(k == "view.alex.b" or k.startswith("view.alex.b/") for k in hive.objects):
        failures.append("B still stands on the display after the boot")
    if "view.alex.a" not in hive.objects:
        failures.append("A was lost with it")
    if "view.alex.b" in (hive.state() or {}).get("views", {}):
        failures.append("the curator still keeps B")
    return failures


# --- repair: a refused patch is repaired by one read (T8, OR-D7) -----------------

def run_repair():
    def script(hive, refuse):
        view = {"title": "A", "context": "conversation", "relevance": "0.9"}
        hive.send_event({"kind": "app_write", "oid": "view.alex.a", "view": view}, 1000)
        hive.refuse_next_patch = refuse
        hive.send_event({"kind": "app_write", "oid": "view.alex.b",
                         "view": dict(view, title="B")}, 2000)
    control, hive = Hive(IDLE_PARAMS), Hive(IDLE_PARAMS)
    script(control, False)
    script(hive, True)
    failures = []
    routes = [h["route"] for h in hive.hops if not rest_only(h)]
    if "patch" not in routes:
        return ["the cell never drew: %s" % routes]
    # boot select, boot read, patch A, write B, the refused patch, ONE read, the repair
    tail = routes[routes.index("patch") + 1:]
    want = ["views", "patch", "read", "patch"]
    if tail[:4] != want or tail.count("read") != 1:
        failures.append("after the first patch the hops were %s, not %s" % (tail, want))
    if hive.objects != control.objects:
        diff = sorted(k for k in set(hive.objects) | set(control.objects)
                      if hive.objects.get(k) != control.objects.get(k))
        failures.append("the repaired display differs in %s" % ", ".join(diff[:4]))
    return failures


def run_repair_clock():
    """The clock through a repair: a pass whose patch the display refused, and a stroke
    while the mirror is being read again, still take back the order before theirs. At
    the end the clock holds exactly the one order the root names, and nothing was taken
    back twice (a leftover order is a stroke for nothing, plan D1a review M2)."""
    held, failures = set(), []

    def clock(out):
        for em in out:
            if (em.get("header") or {}).get("route") != "due":
                continue
            sid = str(em.get("schedule_id") or "")
            if em.get("op") == "add":
                held.add(sid)
            elif sid in held:
                held.discard(sid)
            else:
                failures.append("the order %s was taken back twice" % sid[:8])

    hive = Hive(IDLE_PARAMS)
    view = {"title": "A", "context": "conversation", "relevance": "0.9"}
    clock(hive.send_event({"kind": "app_write", "oid": "view.alex.a", "view": view}, 1000))
    hive.refuse_next_patch, hive.hold_internal = True, True
    clock(hive.send_event({"kind": "app_write", "oid": "view.alex.b",
                           "view": dict(view, title="B")}, 2000))
    while hive.held and hive.ram()["phase"] != "repair":
        clock(hive.send(hive.held.pop(0)))    # the refused patch: the cell reads again
    if hive.ram()["phase"] != "repair":
        return failures + ["the refused patch did not start a repair"]
    clock(hive.send_event({"kind": "stroke"}, 9000))   # a pass while the read is out
    clock(hive.flush())
    hive.hold_internal = False
    due = str(hive.objects.get("display.root", {}).get("props", {}).get("due") or "")
    if held != {due}:
        failures.append("the clock holds %s, the root names %s"
                        % (sorted(x[:8] for x in held), due[:8]))
    return failures


GLASS = {"display-pane", "display-panel"}
PANEL = [{"component": "display-panel", "key": "c.prose", "props": {"title": "Notes"},
          "children": [{"component": "display-text", "key": "p.0", "props": {"body": "x"}}]}]


def run_repair_refused_leg():
    """A leg the display refuses on EVERY try (an app nests glass in glass) is no mirror
    fault: one read cannot cure it, so the repair does not ask again after its own patch was
    refused. Unbounded, each repair's patch was refused, read and refused again until the
    chain's ttl ran out -- and the last read died with it, leaving the cell in `repair`
    and the screen frozen for every write after (wave Display integration pass: ttl 24 to
    0 in six repairs, `710_the_colony_holds_in_both_engines_browser` red 6/6)."""
    hive = Hive(IDLE_PARAMS)
    hive.glass = set(GLASS)
    view = {"title": "A", "context": "conversation", "relevance": "0.9"}
    hive.send_event({"kind": "app_write", "oid": "view.alex.a", "view": view}, 1000)
    mark = len(hive.hops)
    hive.set_now(2000)
    # what an app sends that hangs a panel into its window (the stage's `tall` act)
    hive.send(hive._lane("in_view", {
        "messages": [], "view_id": "b", "region": "main", "ord": 0, "kind": "component",
        "content": {"component": "display-pane", "key": "c.win", "children": PANEL,
                    "props": dict(view, title="B", touched="2000", pane_id="pane-b")},
        "components": [], "ttl_ms": 0}, "alex"))
    failures = []
    refused = [h for h in hive.hops[mark:] if h["route"] == "patch"]
    if len(refused) < 1:
        return ["the panel was never sent to the display"]
    reads = [h for h in hive.hops[mark:] if h["route"] == "read"]
    if len(reads) != 1:
        failures.append("%d reads for one refused leg, not 1" % len(reads))
    if hive.exhausted:
        failures.append("%d emissions died on the hop budget" % hive.exhausted)
    if hive.ram().get("phase") != "live":
        failures.append("the cell stands in %r, not live" % hive.ram().get("phase"))
    hive.send_event({"kind": "app_write", "oid": "view.alex.c",
                     "view": dict(view, title="C")}, 3000)
    if "view.alex.c" not in hive.objects:
        failures.append("the write after the refusal was never drawn")
    return failures


def run_repair_lost_read():
    """A repair read whose answer never comes (its chain ran out of ttl, or the child died
    under it) is asked again by the next event that comes REPAIR_RETRY_MS or more later:
    without that the cell waits in `repair` for good and draws nothing ever again."""
    hive = Hive(IDLE_PARAMS)
    view = {"title": "A", "context": "conversation", "relevance": "0.9"}
    hive.send_event({"kind": "app_write", "oid": "view.alex.a", "view": view}, 1000)
    hive.refuse_next_patch, hive.hold_internal = True, True
    hive.send_event({"kind": "app_write", "oid": "view.alex.b",
                     "view": dict(view, title="B")}, 2000)
    while hive.held and hive.ram()["phase"] != "repair":
        hive.send(hive.held.pop(0))           # the refused patch: the cell reads again
    failures = []
    if hive.ram()["phase"] != "repair":
        return ["the refused patch did not start a repair"]
    hive.held = [h for h in hive.held
                 if h["envelope"]["header"]["context"].get("display_origin") != "read"]
    hive.hold_internal = False
    mark = len(hive.hops)
    hive.send_event({"kind": "stroke"}, 2500)    # too soon: the read may still come
    if [h for h in hive.hops[mark:] if h["route"] == "read"]:
        failures.append("a pass right after the read asked again")
    hive.send_event({"kind": "app_write", "oid": "view.alex.c",
                     "view": dict(view, title="C")}, 9000)
    hive.flush()
    if hive.ram().get("phase") != "live":
        failures.append("the cell stands in %r, not live" % hive.ram().get("phase"))
    for oid in ("view.alex.b", "view.alex.c"):
        if oid not in hive.objects:
            failures.append("%s was never drawn" % oid)
    return failures


# --- main -------------------------------------------------------------------------

def stage(label, scs, only, skip, runner):
    total = passed = 0
    for sc in scs:
        if only and sc["id"] not in only:
            continue
        if sc["id"] in skip:
            print("SKIP %s %s -- colony only: %s" % (sc["id"], sc["title"], skip[sc["id"]]))
            continue
        failures = runner(sc)
        if failures is None:
            print("SKIP %s %s %s -- not applicable" % (label, sc["id"], sc["title"]))
            continue
        total += 1
        if failures:
            print("FAIL %s %s %s: %s" % (label, sc["id"], sc["title"], "; ".join(failures)))
        else:
            passed += 1
            print("PASS %s %s %s" % (label, sc["id"], sc["title"]))
    print("%s %d/%d" % (label, passed, total))
    return int(passed != total)


def single(label, cases):
    passed = 0
    for name, fn in cases:
        failures = fn()
        if failures:
            print("FAIL %s %s: %s" % (label, name, "; ".join(failures)))
        else:
            passed += 1
            print("PASS %s %s" % (label, name))
    print("%s %d/%d" % (label, passed, len(cases)))
    return int(passed != len(cases))


def main(argv):
    only = {a for a in argv if not a.startswith("--")}
    picked = [m for m in MODES if m in argv] or list(MODES)
    if "--cell-only" in argv:          # the old name of --curator-only
        picked = ["--curator-only"]
    with open(os.path.join(HERE, "scenarios.json")) as f:
        doc = json.load(f)
    fixtures, scs = doc.get("fixtures") or {}, doc["scenarios"]
    rc = 0
    if "--model-only" in picked:
        model = load(os.path.join(HERE, "run_model.py"), "run_model")
        n = sum(1 for _ in scs)
        rc |= model.main(["run_model.py"] + sorted(only))
        print("MODEL %s" % ("see above" if only else "%d/%d" % (n, n) if rc == 0 else "FAIL"))
    compose = load(COMPOSE, "compose")
    if "--pure-only" in picked:
        rc |= stage("PURE", scs, only, {}, lambda sc: run_pure(compose, fixtures, sc))
    if "--curator-only" in picked:
        rc |= stage("CURATOR", scs, only, COLONY_ONLY,
                    lambda sc: run_curator(compose, fixtures, sc))
    if "--rebuild-only" in picked:
        rc |= stage("REBUILD", scs, only, COLONY_ONLY,
                    lambda sc: run_rebuild(compose, fixtures, sc))
        for sid, first in AGE_ONLY:
            print("REBUILD %s equal after the second stroke only (OR-D19), first stroke: %s"
                  % (sid, ", ".join(first)))
    if "--rebuild-write-only" in picked:
        rc |= stage("REBUILD-WRITE", scs, only, COLONY_ONLY,
                    lambda sc: run_rebuild_write(compose, fixtures, sc))
    if "--boot-only" in picked:
        rc |= single("BOOT", [("two writes inside the boot window", run_boot),
                              ("a failed boot select asked again", run_boot_retry),
                              ("a row gone while the cell was down", run_boot_row_gone)])
    if "--repair-only" in picked:
        rc |= single("REPAIR", [("a refused patch", run_repair),
                                ("the clock through a repair", run_repair_clock),
                                ("a leg the display always refuses", run_repair_refused_leg),
                                ("a repair read that never answers", run_repair_lost_read)])
    if "--snapshot-only" in picked:
        rc |= stage("SNAPSHOT", scs, only, COLONY_ONLY,
                    lambda sc: run_snapshot(compose, fixtures, sc))
    if "--hops-only" in picked:
        rc |= stage("HOPS", scs, only, COLONY_ONLY,
                    lambda sc: run_hops(compose, fixtures, sc))
        size, where = largest_header(compose, fixtures, scs)
        print("HOPS largest internal header %d B (%s)" % (size, where))
    if "--idle-only" in picked:
        rc |= single("IDLE", [("20 strokes, judge off", lambda: run_idle(False)),
                              ("20 strokes after a judged app touch", lambda: run_idle(True))])
    return rc


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
