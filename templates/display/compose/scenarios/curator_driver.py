#!/usr/bin/env python3
"""The display hive's curator, driven exactly the way a `resident` code cell drives it.

`compose.py` keeps the screen's state in memory between messages (GH #809): the harness
of a `resident` cell compiles the script ONCE and executes it again for every message into
ONE globals dict (`crates/meclaw-cells/src/code/harness.py`, `_run`). A subprocess per
message cannot hold that memory and an `import` runs the module level only once, so this
driver does what the harness does -- `compile` once, `exec` into the same dict per
document, stdin/stdout/stderr redirected, `SystemExit` caught -- and plays the rest of
the hive in-process around it:

  * the `views` store (`table`): select / delete / insert / update, answered in the shapes
    of `crates/meclaw-cells/src/store/output.rs` -- one op: its metadata on the hop; a
    bundle of more: `results[]` in the body and `operation: bundle`, the summed
    `rows_affected` and `bundle_errors` on the hop;
  * the `web` cell (`objects`): `query` answers `{"route", "root", "objects": [...]}` over
    every object it holds (`web/ops.rs`, `query` by route), a patch applies its
    `object.*` / `page.set` / `component.define` calls and answers like
    `web/output.rs::build_bundle_result`. `refuse_next_patch` refuses the next patch
    whole: its first leg fails and NO leg is applied. The real `web` is no transaction and
    applies the legs it can take; either way the curator only learns "refused" and reads
    the tree again (OR-D7), so the proof holds for both. `glass` names components the
    display refuses to nest in one another on every try (`check_glass_on_glass`), the one
    refusal a re-read cannot cure;
  * the hop budget: one document's reply chain dies after `budget` hops (24, the ttl the
    display colony of the Rust locks gives a write -- `support/display_colony.rs`; the
    substrate's default is `MESSAGE_DEFAULT_TTL`, 64), the way a message whose ttl ran out is dead-lettered --
    `exhausted` counts the emissions that died so;
  * the hive's edges: what a reply carries in `context` is what `templates/display/
    config.json` stamps -- `display_origin` always, `display_request` on the store lane
    only (the `read` and `patch` edges delete it).

Everything the script emits on the three internal lanes (`views`, `read`, `patch`) is
served here and its reply goes back into the script, until nothing internal is left; the
emissions on every other lane (`event`, `receipt`, `due`, `judge`) are what `send`
returns. `kill` drops the globals dict -- a killed child -- and keeps the table and the
tree, which is exactly what a restart of the cell leaves behind.

Library:

    hive = Hive({"default_screen": "tv", "screens": {...}})
    hive.set_now(1000)
    out = hive.send_event({"kind": "app_write", "oid": "view.alex.card", "view": {...}}, 1000)
    hive.state(); hive.table; hive.objects; hive.hops

Server (line JSON; the interface the Rust locks speak):

    python3 curator_driver.py --serve --params '<json>'

    stdin, one per line:  {"op": "send", "doc": {...}}
                          {"op": "event", "event": {...}, "now": ms}
                          {"op": "kill"} | {"op": "web_reset"} | {"op": "now", "ms": ms}
                          {"op": "refuse_next_patch"} | {"op": "flush"}
                          {"op": "hold", "on": bool}
                          {"op": "table_put", "row": {...}}
                          {"op": "table_del", "owner": ..., "view_id": ...}
                          {"op": "params", "params": {...}}
                          {"op": "web_put", "object": {id, parent, ord, component, props}}
                          {"op": "web_update", "id": ..., "props": {...}}
    stdout, one per line: {"out": [...], "state": {...}|null, "said": [...], "objects": [...],
                           "table": [...], "hops": [...], "stderr": "..."}

`hops` in a server answer are the internal emissions of THAT line only; the library's
`hive.hops` is the whole history. Exit at EOF.
"""
import copy
import io
import json
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
COMPOSE = os.path.join(HERE, "..", "compose.py")
INTERNAL = ("views", "read", "patch")


class CellFailed(Exception):
    """The script exited non-zero: a traceback in a real colony, a finding here."""


class Hive:
    """One display hive: compose resident, the store and the display in-process."""

    def __init__(self, params, compose_path=COMPOSE, hold_internal=False):
        with open(compose_path, "r", encoding="utf-8") as fh:
            self.code = compile(fh.read(), os.path.abspath(compose_path), "exec")
        self.params = dict(params or {})
        self.hold_internal = hold_internal
        self.table = []          # the `views` store, one dict per row
        self.objects = {}        # what `web` holds: id -> {id, parent, component, ord, props}
        self.pages = {}          # route -> root (`page.set`)
        self.components = {}     # name -> definition (`component.define`)
        self.hops = []           # every internal emission, in order: {route, request, header, ops, calls}
        self.stderr = []         # what the script wrote on stderr, per exec
        self.held = []           # replies kept back while `hold_internal` (see `flush`)
        self.refuse_next_patch = False
        # Component names the display refuses to nest in one another ("glass never sits
        # on glass", `web/ops.rs::check_glass_on_glass`): a create of one of them under
        # a parent of one of them is refused on EVERY try -- a refusal a re-read cannot
        # cure. Empty unless a case sets it, so no other case sees the rule.
        self.glass = set()
        # The hop budget of one document: a reply chain deeper than this dies the way a
        # message whose `ttl` ran out dies in a colony -- dead-lettered, never delivered
        # (measured: the repair loop of wave Display's integration pass, ttl 24 -> 0).
        self.budget = 24
        self.exhausted = 0
        self.now = None
        self.glb = None
        self.kill()

    # --- the cell ------------------------------------------------------------------

    def kill(self):
        """A killed child: the resident dict is gone, the store and the tree stay."""
        # `script_inline`, as the template ships it: no `__file__` (harness.py seeds one
        # only for a `script_path`).
        self.glb = {"__name__": "__main__"}
        if self.now is not None:
            self.glb["_TEST_NOW"] = self.now

    def set_now(self, ms):
        """The clock the script reads (`now_ms`, OR-D10) until the next call."""
        self.now = int(ms)
        self.glb["_TEST_NOW"] = self.now

    def web_reset(self):
        """A fresh `web`: no objects, no pages, no vocabulary."""
        self.objects = {}
        self.pages = {}
        self.components = {}

    def ram(self):
        held = self.glb.get("_RAM")
        return held if isinstance(held, dict) else {}

    def state(self):
        return self.ram().get("state")

    def fn(self, name):
        """A function of the running script (the current exec's definition)."""
        if name not in self.glb:
            self._exec({"params": self.params, "body": {}, "envelope": {"header": {}}})
        return self.glb[name]

    def _exec(self, doc):
        """One message, exactly as `harness.py::_run` frames it."""
        out, err = io.StringIO(), io.StringIO()
        real = sys.stdin, sys.stdout, sys.stderr
        code = 0
        sys.stdin, sys.stdout, sys.stderr = io.StringIO(json.dumps(doc)), out, err
        try:
            exec(self.code, self.glb)
        except SystemExit as exc:
            code = exc.code if isinstance(exc.code, int) else (0 if exc.code is None else 1)
        except BaseException:  # noqa: BLE001 -- the harness frames every failure the same way
            import traceback
            code = 1
            traceback.print_exc(file=err)
        finally:
            sys.stdin, sys.stdout, sys.stderr = real
        if err.getvalue():
            self.stderr.append(err.getvalue())
        if code != 0:
            raise CellFailed(err.getvalue())
        text = out.getvalue().strip()
        if not text:
            return []
        answer = json.loads(text)
        if isinstance(answer, list):
            return [a for a in answer if isinstance(a, dict)]
        return [answer] if isinstance(answer, dict) else []

    # --- one document in, the outer emissions out -------------------------------------

    def send(self, doc):
        """Hand the cell ONE document; serve every internal lane until nothing is left."""
        doc = dict(doc)
        doc.setdefault("params", self.params)
        outer = []
        queue = [(doc, self.budget)]
        while queue:
            doc, ttl = queue.pop(0)
            ems = self._exec(doc)
            for em in ems:
                head = em.get("header") or {}
                route = str(head.get("route") or "")
                if route not in INTERNAL:
                    outer.append(em)
                    continue
                if ttl <= 2:
                    # the emission and its reply are two more hops than the chain has left
                    self.exhausted += 1
                    continue
                reply = self._serve(route, em)
                if self.hold_internal:
                    self.held.append(reply)
                else:
                    queue.append((reply, ttl - 2))
        return outer

    def flush(self):
        """Deliver the replies held back under `hold_internal`, then serve on as usual."""
        outer = []
        keep, self.hold_internal = self.hold_internal, False
        try:
            while self.held:
                outer += self.send(self.held.pop(0))
        finally:
            self.hold_internal = keep
        return outer

    def send_event(self, event, now):
        """A model event (`display-hive.md` § 4.1) as the document that carries it."""
        self.set_now(now)
        return self.send(self.doc_of(event))

    def doc_of(self, event):
        kind = str(event.get("kind") or "")
        if kind in ("app_write", "app_withdraw"):
            _, owner, view_id = str(event["oid"]).split(".", 2)
            if kind == "app_withdraw":
                return self._lane("in_withdraw", {"messages": [], "view_id": view_id}, owner)
            sent = dict(event.get("view") or {})
            ttl = sent.pop("ttl_ms", 0)
            children = sent.pop("children", None)
            sent.pop("owner", None)
            body = {"messages": [], "view_id": view_id, "region": "main", "ord": 0,
                    "kind": "component",
                    "content": self.fn("window_node")(sent, children),
                    "components": [], "ttl_ms": ttl}
            return self._lane("in_view", body, owner)
        if kind not in ("tap", "hold", "verdict", "stroke"):
            # Not a trigger of § 4.1 (a finger's input on a page, a press): what reaches the
            # cell is a browser event it hands on, and no pass runs.
            value = {"for": str(event.get("for") or "")}
            return {"body": {"messages": [], "event": {"name": kind, "value": value}},
                    "envelope": {"header": {"hop": {"route": "event"}, "context": {}}}}
        if kind in ("tap", "hold"):
            value = {"for": str(event.get("for") or "")} if kind == "tap" else {}
            return {"body": {"messages": [], "event": {"name": kind, "value": value}},
                    "envelope": {"header": {"hop": {"route": "event"}, "context": {}}}}
        if kind == "verdict":
            windows = [dict({"id": oid}, **(v or {}))
                       for oid, v in sorted((event.get("windows") or {}).items())]
            answer = {"weights": event.get("weights") or {}, "windows": windows}
            if "bar" in event and event["bar"] is not None:
                answer["bar"] = event["bar"]
            return {"body": {"messages": [{"origin": "assistant", "type": "text",
                                           "text": json.dumps(answer)}]},
                    "envelope": {"header": {"hop": {"route": "in_verdict",
                                                    "finish_reason": "stop"},
                                            "context": {}}}}
        # a stroke: the clock's strike, as the hive's edge re-stamps it
        return {"body": {"messages": []},
                "envelope": {"header": {"hop": {"route": "in_tick", "schedule_name": "due",
                                                "schedule_id": str(event.get("struck") or "")},
                                        "context": {}}}}

    @staticmethod
    def _lane(route, body, owner):
        return {"body": body,
                "envelope": {"reply_to": owner,
                             "header": {"hop": {"route": route}, "context": {}}}}

    # --- the store by hand ------------------------------------------------------------

    def table_put(self, row):
        """A row in the `views` store WITHOUT a pass: what stood there before the cell woke.

        The curator reads the store once, at its boot; a row put here after that is only
        seen by the next boot (a `kill`). This is how a test sets up a prior state -- the
        rows the apps wrote while the cell was down -- not how an app writes.
        """
        row = copy.deepcopy(row)
        self.table = [r for r in self.table
                      if not (r.get("owner") == row.get("owner")
                              and r.get("view_id") == row.get("view_id"))]
        self.table.append(row)

    def web_put(self, obj):
        """An object in `web`'s tree WITHOUT a patch: what a display held before the cell
        woke (an object an older version drew, say). Seen by the next boot's `read`."""
        obj = copy.deepcopy(obj)
        self.objects[str(obj.get("id") or "")] = {
            "id": str(obj.get("id") or ""), "parent": obj.get("parent"),
            "ord": obj.get("ord") or 0, "component": str(obj.get("component") or ""),
            "props": obj.get("props") or {}}

    def table_del(self, owner, view_id):
        """Take one row out of the store WITHOUT a pass (see `table_put`)."""
        self.table = [r for r in self.table
                      if not (r.get("owner") == owner and r.get("view_id") == view_id)]

    # --- the internal lanes ------------------------------------------------------------

    def _serve(self, route, em):
        head = em.get("header") or {}
        request = head.get("display_request")
        try:
            parsed = json.loads(request) if isinstance(request, str) and request else None
        except ValueError:
            parsed = None
        calls = []
        for leg in em.get("messages") or []:
            try:
                call = json.loads(str(leg.get("text") or ""))
            except (TypeError, ValueError, AttributeError):
                call = None
            calls.append((str(leg.get("id") or ""), call if isinstance(call, dict) else {}))
        self.hops.append({"route": route, "request": parsed, "header": head,
                          "ops": [c.get("operation") or c.get("op") for _, c in calls],
                          "calls": [copy.deepcopy(c) for _, c in calls]})
        if route == "views":
            legs = [self._store(c) + (tid,) for tid, c in calls]
            context = {"display_origin": "views",
                       "display_request": request if isinstance(request, str) else ""}
        elif route == "read":
            legs = [self._query(c) + (tid,) for tid, c in calls]
            context = {"display_origin": "read"}
        else:
            if self.refuse_next_patch:
                self.refuse_next_patch = False
                legs = [("patch refused by the driver", 0, "invalid_input",
                         str(c.get("op") or ""), tid) for tid, c in calls[:1]]
                legs += [("not applied", 0, None, str(c.get("op") or ""), tid)
                         for tid, c in calls[1:]]
            else:
                legs = [self._apply(c) + (tid,) for tid, c in calls]
            context = {"display_origin": "patch"}
        body, hop = self._reply(legs)
        return {"params": self.params, "body": body,
                "envelope": {"header": {"hop": hop, "context": context}}}

    @staticmethod
    def _reply(legs):
        """`build_tool_result` for one leg, `build_bundle_result` for more."""
        turns = [{"origin": "tool", "type": "tool_result", "text": text, "id": tid}
                 for text, _, _, _, tid in legs]
        if len(legs) == 1:
            text, moved, code, op, tid = legs[0]
            hop = {"operation": op, "rows_affected": moved, "duration_ms": 0}
            if code:
                hop["error_code"] = code
            return {"messages": turns}, hop
        results = []
        for text, moved, code, op, tid in legs:
            entry = {"tool_call_id": tid, "operation": op, "rows_affected": moved,
                     "duration_ms": 0}
            if code:
                entry["error_code"] = code
            results.append(entry)
        hop = {"operation": "bundle", "rows_affected": sum(l[1] for l in legs),
               "duration_ms": 0, "bundle_errors": sum(1 for l in legs if l[2])}
        return {"messages": turns, "results": results}, hop

    @staticmethod
    def _match(row, where):
        return all(row.get(k) == v for k, v in (where or {}).items())

    def _store(self, call):
        """One store op on `table`: (text, rows_affected, error_code, operation)."""
        op = str(call.get("operation") or "")
        if op == "select":
            cols = call.get("columns")
            rows = [r for r in self.table if self._match(r, call.get("where"))]
            if isinstance(cols, list):
                rows = [{c: r.get(c) for c in cols} for r in rows]
            return json.dumps(copy.deepcopy(rows)), len(rows), None, op
        if op == "delete":
            keep = [r for r in self.table if not self._match(r, call.get("where"))]
            moved = len(self.table) - len(keep)
            self.table = keep
            return json.dumps({"rows_affected": moved}), moved, None, op
        if op == "insert":
            row = call.get("row")
            if not isinstance(row, dict):
                return "insert without a row", 0, "invalid_input", op
            self.table.append(copy.deepcopy(row))
            return json.dumps({"rows_affected": 1}), 1, None, op
        if op == "update":
            moved = 0
            for r in self.table:
                if self._match(r, call.get("where")):
                    r.update(copy.deepcopy(call.get("set") or {}))
                    moved += 1
            return json.dumps({"rows_affected": moved}), moved, None, op
        return "unknown operation %r" % op, 0, "invalid_input", op

    def _query(self, call):
        route = call.get("route")
        if call.get("op") != "query" or route not in self.pages:
            return ("no page declares the route %r" % (route,), 0, "invalid_input", "query")
        objs = sorted(self.objects.values(),
                      key=lambda o: (str(o.get("parent") or ""), o.get("ord") or 0))
        doc = {"route": route, "root": self.pages[route], "objects": copy.deepcopy(objs)}
        return json.dumps(doc), 0, None, "query"

    def _apply(self, call):
        """One patch call on `objects`, refused the way `web` refuses it."""
        op = str(call.get("op") or "")
        oid = str(call.get("id") or "")
        if op == "component.define":
            self.components[str(call.get("name") or "")] = copy.deepcopy(call)
            return "{}", 1, None, op
        if op == "page.set":
            if call.get("root") not in self.objects:
                return "no object %r" % (call.get("root"),), 0, "unknown_object", op
            self.pages[str(call.get("route") or "")] = call["root"]
            return "{}", 1, None, op
        if op == "object.create":
            parent_of = self.objects.get(str(call.get("parent") or ""))
            if (str(call.get("component") or "") in self.glass and parent_of
                    and parent_of.get("component") in self.glass):
                return "glass never sits on glass", 0, "invalid_input", op
            if oid in self.objects:
                return "object %r exists" % oid, 0, "invalid_input", op
            parent = call.get("parent")
            if parent is not None and parent not in self.objects:
                return "no parent %r" % parent, 0, "unknown_object", op
            self.objects[oid] = {"id": oid, "parent": parent, "ord": call.get("ord") or 0,
                                 "component": str(call.get("component") or ""),
                                 "props": copy.deepcopy(call.get("props") or {})}
            return "{}", 1, None, op
        if oid not in self.objects:
            return "no object %r" % oid, 0, "unknown_object", op
        held = self.objects[oid]
        if op == "object.update":
            held["props"].update(copy.deepcopy(call.get("props") or {}))
            return "{}", 1, None, op
        if op == "object.move":
            held["parent"] = call.get("parent")
            held["ord"] = call.get("ord") or 0
            return "{}", 1, None, op
        if op == "object.delete":
            if any(o.get("parent") == oid for o in self.objects.values()):
                return "object %r has children" % oid, 0, "invalid_input", op
            del self.objects[oid]
            return "{}", 1, None, op
        return "unknown op %r" % op, 0, "invalid_input", op


# --- server mode -------------------------------------------------------------------------

def jsonable(value):
    return json.loads(json.dumps(value, default=lambda o: sorted(o) if isinstance(o, set)
                                 else str(o)))


def serve(params, stdin=None, stdout=None):
    stdin = stdin or sys.stdin
    stdout = stdout or sys.stdout
    hive = Hive(params)
    for line in stdin:
        if not line.strip():
            continue
        req = json.loads(line)
        op = req.get("op")
        mark = len(hive.hops)
        hive.stderr = []
        out, error = [], None
        try:
            if op == "send":
                out = hive.send(req.get("doc") or {})
            elif op == "event":
                out = hive.send_event(req.get("event") or {}, int(req.get("now") or 0))
            elif op == "kill":
                hive.kill()
            elif op == "web_reset":
                hive.web_reset()
            elif op == "now":
                hive.set_now(int(req.get("ms") or 0))
            elif op == "refuse_next_patch":
                hive.refuse_next_patch = True
            elif op == "hold":
                hive.hold_internal = bool(req.get("on", True))
            elif op == "flush":
                out = hive.flush()
            elif op == "table_put":
                hive.table_put(req.get("row") or {})
            elif op == "table_del":
                hive.table_del(str(req.get("owner") or ""), str(req.get("view_id") or ""))
            elif op == "params":
                hive.params = dict(req.get("params") or {})
            elif op == "web_put":
                hive.web_put(req.get("object") or {})
            elif op == "web_update":
                held = hive.objects.get(str(req.get("id") or ""))
                if held is None:
                    error = "web holds no object %r" % (req.get("id"),)
                else:
                    held["props"].update(copy.deepcopy(req.get("props") or {}))
            else:
                error = "unknown op %r" % (op,)
        except CellFailed as exc:
            error = "compose failed: %s" % (exc,)
        answer = {"out": out, "state": hive.state(), "said": hive.ram().get("said") or [],
                  "objects": sorted(hive.objects.values(), key=lambda o: o["id"]),
                  "table": hive.table, "hops": hive.hops[mark:],
                  "stderr": "".join(hive.stderr)}
        if error:
            answer["error"] = error
        stdout.write(json.dumps(jsonable(answer), sort_keys=True) + "\n")
        stdout.flush()
    return 0


def main(argv):
    if "--serve" not in argv:
        sys.stderr.write(__doc__)
        return 2
    params = {}
    if "--params" in argv:
        params = json.loads(argv[argv.index("--params") + 1])
    return serve(params)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
