"""GH #609 -- two regions on one screen, and an order a rewrite cannot move.

Driven by `crates/meclaw-cells/tests/gh609_a_standing_view_keeps_its_place.rs`.
Run it by hand the same way:

    python3 crates/meclaw-cells/tests/fixtures/gh609_region_order_check.py \
        templates/display/compose/compose.py

It drives the SHIPPED source the way the substrate drives it -- one JSON
document in on stdin, one out on stdout, once per pass -- and it plays both
of the cell's correspondents itself: a `views` store that holds rows, and a
display that holds objects and answers a `query` with them. Nothing here
imports the cell, because what is under test is the loop and not a function:
the order a person sees is settled by the SEATS the display already holds, so
a checker that called `build()` with a dictionary of its own would be asserting
its own fixture.

The measurement that matters is the second one. A view rewritten every twenty
seconds used to take the top slot on every tick, because the screen was sorted
newest-first -- right for a card, wrong for anything standing. Here one view is
rewritten five times and the assertion is that nothing moves.
"""
import json
import subprocess
import sys

FAIL = []
COUNT = [0]

ROOT = "display.root"
MAIN = "display.region.main"
ASIDE = "display.region.aside"
# The microphone hangs under the root beside the regions, behind both of them
# (GH #643). It is named here so the checks below can say what the root holds
# WITHOUT counting it as a column.
MIC = "display.mic"


def ok(name, cond, detail=""):
    COUNT[0] += 1
    if cond:
        print("  ok  " + name)
    else:
        print("  FAIL " + name + "  " + str(detail))
        FAIL.append(name)


# --------------------------------------------------------------- the cell


class Cell(object):
    """The compose cell, run the way a `code` cell is run."""

    def __init__(self, path):
        self.path = path

    def __call__(self, body, hop, context, reply_to=None):
        envelope = {"header": {"hop": hop, "context": context}}
        if reply_to is not None:
            envelope["reply_to"] = reply_to
        doc = {"body": body, "envelope": envelope}
        proc = subprocess.run(
            [sys.executable, self.path],
            input=json.dumps(doc),
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            universal_newlines=True,
        )
        if proc.returncode != 0:
            raise AssertionError("compose failed: " + proc.stderr)
        out = json.loads(proc.stdout)
        return out if isinstance(out, list) else [out]


# ------------------------------------------------------- the two neighbours


class Store(object):
    """The `views` table, and the one bundle this cell ever sends it."""

    def __init__(self):
        self.rows = []

    def bundle(self, calls):
        """`(messages, results)` -- leg 0's rows first, in call order."""
        messages, results = [], []
        for call in calls:
            args = json.loads(call["text"])
            op = args["operation"]
            text = '{"rows_affected":1}'
            if op == "select":
                text = json.dumps(self.rows)
            elif op == "delete":
                where = args["where"]
                self.rows = [
                    r for r in self.rows
                    if not all(r.get(k) == v for k, v in where.items())
                ]
            elif op == "insert":
                self.rows.append(dict(args["row"]))
            messages.append({"origin": "tool", "type": "tool_result",
                             "id": call["id"], "text": text})
            results.append({"tool_call_id": call["id"], "operation": op})
        return {"messages": messages, "results": results}


class Screen(object):
    """The display, as much of it as this loop can see: objects and a page."""

    def __init__(self):
        self.objects = {}
        self.components = set()
        self.page = None

    def apply(self, calls):
        """One patch bundle. Returns the ops that were refused, by name."""
        refused = []
        for call in calls:
            args = json.loads(call["text"])
            op = args["op"]
            if op == "component.define":
                self.components.add(args["name"])
            elif op == "page.set":
                self.page = args["root"]
            elif op == "object.create":
                if args.get("component") not in self.components:
                    refused.append("unknown_component:" + str(args.get("component")))
                    continue
                self.objects[args["id"]] = {
                    "props": dict(args.get("props") or {}),
                    "parent": args.get("parent"),
                    "ord": args.get("ord", 0),
                }
            elif op == "object.update":
                held = self.objects.get(args["id"])
                if held is None:
                    refused.append("unknown_object:" + args["id"])
                    continue
                held["props"].update(args.get("props") or {})
            elif op == "object.move":
                held = self.objects.get(args["id"])
                if held is None:
                    refused.append("unknown_object:" + args["id"])
                    continue
                held["parent"] = args.get("parent")
                held["ord"] = args.get("ord", 0)
            elif op == "object.delete":
                self.objects.pop(args["id"], None)
            else:
                refused.append("unknown_op:" + op)
        return refused

    def query(self):
        """What a `query` on `/` answers with, in the display's own shape."""
        if self.page is None:
            return None
        return {"objects": [
            dict(spec, id=oid) for oid, spec in sorted(self.objects.items())
        ]}

    def children(self, parent):
        """The ids under `parent`, in drawing order."""
        kids = [(spec["ord"], oid) for oid, spec in self.objects.items()
                if spec["parent"] == parent]
        return [oid for _, oid in sorted(kids)]


# ------------------------------------------------------------- the whole loop


class Loop(object):
    """One screen: the cell, its store and its display, wired to each other."""

    def __init__(self, cell):
        self.cell = cell
        self.store = Store()
        self.screen = Screen()
        self.refused = []

    def send(self, owner, body, route="in_view"):
        """One `in_view` or `in_withdraw`, all the way to the screen.

        Returns the receipt when the door refused it, else None.
        """
        out = self.cell(body, {"route": route}, {}, owner)
        if not out:
            return None
        first = out[0]
        if first["header"]["route"] == "receipt":
            return first["receipt"]

        # Pass 2: the store answers the bundle it was sent.
        reply = self.store.bundle(first["messages"])
        out = self.cell(
            reply,
            {"operation": "bundle", "bundle_errors": 0},
            {"display_origin": "views",
             "display_request": first["header"]["display_request"]},
        )
        if not out:
            return None
        read = out[0]

        # Pass 3: the display answers the query.
        answer = self.screen.query()
        if answer is None:
            body_in = {"messages": [{"origin": "tool", "type": "tool_result",
                                     "id": "d-query", "text": "{}"}]}
            hop = {"operation": "query", "error_code": "invalid_input"}
        else:
            body_in = {"messages": [{"origin": "tool", "type": "tool_result",
                                     "id": "d-query",
                                     "text": json.dumps(answer)}]}
            hop = {"operation": "query"}
        out = self.cell(
            body_in, hop,
            {"display_origin": "read",
             "display_views": read["header"]["display_views"]},
        )
        if out:
            self.refused += self.screen.apply(out[0]["messages"])
        return None

    def view(self, owner, view_id, title, **extra):
        body = {"messages": [], "view_id": view_id, "kind": "prose",
                "content": {"title": title, "body": title + " body"}}
        body.update(extra)
        return self.send(owner, body)

    def withdraw(self, owner, view_id):
        return self.send(owner, {"messages": [], "view_id": view_id},
                         route="in_withdraw")

    def order(self, region):
        """The `view_id` of every view standing in `region`, top to bottom."""
        parent = "display.region." + region
        return [self.screen.objects[oid]["props"]["view_id"]
                for oid in self.screen.children(parent)]


ALICE = "/os/orgs/x/members/one/assistants/alice"
AMBIENT = "/os/orgs/x/members/one/apps/ambient"


def main():
    if len(sys.argv) < 2:
        print("usage: gh609_region_order_check.py <compose.py>")
        return 2
    cell = Cell(sys.argv[1])

    # ------------------------------------------------ 1. two regions, at once
    loop = Loop(cell)
    loop.view(ALICE, "note", "Note")
    loop.view(AMBIENT, "ambient", "Ambient", region="aside")

    ok("both regions hang under the root",
       sorted(loop.screen.children(ROOT)) == sorted([MAIN, ASIDE, MIC]),
       loop.screen.children(ROOT))
    ok("the root holds them in declaration order, main first",
       [c for c in loop.screen.children(ROOT) if c != MIC] == [MAIN, ASIDE],
       loop.screen.children(ROOT))
    ok("and the microphone stands behind both columns",
       loop.screen.children(ROOT)[-1] == MIC,
       loop.screen.children(ROOT))
    ok("the two regions do not share an `ord`",
       loop.screen.objects[MAIN]["ord"] != loop.screen.objects[ASIDE]["ord"],
       (loop.screen.objects[MAIN]["ord"], loop.screen.objects[ASIDE]["ord"]))
    ok("nothing in the bundle was refused", loop.refused == [], loop.refused)
    ok("a view that named `aside` stands in the aside",
       loop.order("aside") == ["ambient"], loop.order("aside"))
    ok("a view that named no region stands in main",
       loop.order("main") == ["note"], loop.order("main"))

    # -------------------------------- 2. THE measurement: a rewrite moves noth
    loop = Loop(cell)
    loop.view(ALICE, "note", "Note")
    loop.view(AMBIENT, "ambient", "Ambient")
    ok("first appearance is the order two views arrive in",
       loop.order("main") == ["note", "ambient"], loop.order("main"))

    for i in range(5):
        loop.view(AMBIENT, "ambient", "Ambient %d" % i)
    ok("a view rewritten five times keeps its place (GH #609)",
       loop.order("main") == ["note", "ambient"], loop.order("main"))
    ok("...and the rewrite did land: the screen shows the last one",
       any(o["props"].get("title") == "Ambient 4"
           for o in loop.screen.objects.values()),
       [o["props"].get("title") for o in loop.screen.objects.values()])
    ok("...and nothing was refused on the way", loop.refused == [], loop.refused)

    # The other half of the same promise: rewriting the TOP view leaves it on
    # top. An order that only holds in one direction is a coincidence.
    for i in range(5):
        loop.view(ALICE, "note", "Note %d" % i)
    ok("rewriting the top view does not push it down either",
       loop.order("main") == ["note", "ambient"], loop.order("main"))

    # ------------------------------------------------ 3. a declared `ord` wins
    loop = Loop(cell)
    loop.view(ALICE, "note", "Note")
    loop.view(AMBIENT, "ambient", "Ambient", ord=-10)
    ok("a lower `ord` stands above an earlier arrival",
       loop.order("main") == ["ambient", "note"], loop.order("main"))
    for i in range(5):
        loop.view(AMBIENT, "ambient", "Ambient %d" % i, ord=-10)
    ok("and it stays there across five rewrites",
       loop.order("main") == ["ambient", "note"], loop.order("main"))

    # ------------------------------------- 4. a seat is vacated, not inherited
    loop = Loop(cell)
    for name in ("one", "two", "three"):
        loop.view(ALICE, name, name)
    ok("three views arrive in three seats",
       loop.order("main") == ["one", "two", "three"], loop.order("main"))
    loop.withdraw(ALICE, "one")
    ok("withdrawing the top view moves the rest up",
       loop.order("main") == ["two", "three"], loop.order("main"))
    loop.view(ALICE, "one", "one again")
    ok("a view that comes back is a NEW appearance and stands last",
       loop.order("main") == ["two", "three", "one"], loop.order("main"))

    # ------------------------------------------- 5. a region change is a move
    loop = Loop(cell)
    loop.view(ALICE, "note", "Note")
    loop.view(AMBIENT, "ambient", "Ambient")
    loop.view(AMBIENT, "ambient", "Ambient", region="aside")
    ok("a view that changes region leaves the one it was in",
       loop.order("main") == ["note"], loop.order("main"))
    ok("...and arrives in the other one",
       loop.order("aside") == ["ambient"], loop.order("aside"))
    ok("...and the object moved rather than being replaced",
       len([o for o in loop.screen.objects
            if o.startswith("view.")]) == 2,
       sorted(o for o in loop.screen.objects if o.startswith("view.")))

    # ------------------------------------------------- 6. the door still says
    loop = Loop(cell)
    receipt = loop.view(ALICE, "note", "Note", region="left")
    ok("an unknown region is still an `invalid_view` receipt",
       receipt is not None and receipt.get("error_code") == "invalid_view",
       receipt)
    ok("...naming the region it refused",
       receipt is not None and "left" in str(receipt.get("detail")), receipt)
    ok("...and nothing was written", loop.store.rows == [], loop.store.rows)

    receipt = loop.view(ALICE, "note", "Note", ord="3")
    ok("an `ord` that is not an integer is refused by name",
       receipt is not None and receipt.get("error_code") == "invalid_view"
       and "ord" in str(receipt.get("detail")), receipt)
    receipt = loop.view(ALICE, "note", "Note", ord=True)
    ok("a boolean `ord` is refused with it",
       receipt is not None and receipt.get("error_code") == "invalid_view",
       receipt)

    # ----------------------------------------------- 7. the layout is shipped
    src = open(sys.argv[1]).read()
    ok("the shell carries this scope's own two-column rule",
       ".display-columns" in src and 'data-region="aside"' in src)
    ok("an empty aside takes no width",
       'data-region="aside"]:empty' in src)
    ok("a narrow screen stacks the two columns",
       "@media (max-width: 60rem)" in src and "flex-direction: column" in src)
    style = src[src.find("LAYOUT_CSS"):src.find("SHELL_TEMPLATE")]
    ok("the stylesheet carries no `{{`, which the component language would eat",
       "{{" not in style)

    print("\n%d checks, %d failed" % (COUNT[0], len(FAIL)))
    if FAIL:
        return 1
    print("all green")
    return 0


if __name__ == "__main__":
    sys.exit(main())
