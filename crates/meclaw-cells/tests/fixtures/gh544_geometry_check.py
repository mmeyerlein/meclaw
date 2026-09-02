"""GH #544 -- the picture a viewer gets, measured against the five acceptance
criteria of the issue, over a colony that is nested five deep and arranged by
hand.

Driven by `crates/meclaw-cells/tests/gh544_the_flow_reaches_the_screen.rs`.
Run it by hand the same way:

    python3 crates/meclaw-cells/tests/fixtures/gh544_geometry_check.py \
        templates/colony-view/layout/layout.py templates/display/compose/compose.py

It loads the two SHIPPED sources rather than copies of them, and it measures
what the display's object store would hold -- the node positions the layout
writes plus the offsets a hand wrote beside them -- because that is the picture
a screenshot, an export or the showcase capture of GH #43 gets. The browser
re-derives the frames in `mounted()`; nothing else does.
"""
import importlib.util
import itertools
import json
import sys

FAIL = []
COUNT = [0]


def ok(name, cond, detail=""):
    COUNT[0] += 1
    if cond:
        print("  ok  " + name)
    else:
        print("  FAIL " + name + "  " + str(detail))
        FAIL.append(name)


def load(path, name):
    spec = importlib.util.spec_from_file_location(name, path)
    mod = importlib.util.module_from_spec(spec)
    sys.modules[name] = mod
    spec.loader.exec_module(mod)
    return mod


# --------------------------------------------------------------- the fixture
#
# Five levels of hive, several cells at every level, and a flow to project: a
# colony whose only nesting is at the leaves says nothing about a frame inside a
# frame inside a frame. `/os/orgs/<org>/members/<member>/assistants/<agent>/…`
# is the real shape this was measured on, so the fixture wears it.

def fixture():
    hives = [
        "os/gateway",
        "os/builder",
        "os/builder/librarian",
        "os/orgs/acme",
        "os/orgs/acme/members/lea",
        "os/orgs/acme/members/lea/memory-hive",
        "os/orgs/acme/members/lea/assistants/ada",
        "os/orgs/acme/members/lea/assistants/ada/cogny",
        "os/orgs/acme/members/lea/assistants/ada/cogny/collector",
        "os/orgs/acme/members/lea/assistants/ada/surface",
        "os/orgs/acme/members/lea/apps/colony-view",
    ]
    kinds = ["code", "llm", "store", "timer", "web"]
    nodes, edges = [], []
    for h, hive in enumerate(hives):
        n = 3 + (h % 4)
        names = ["c%d" % k for k in range(n)]
        for k, nm in enumerate(names):
            nodes.append({"path": "/" + hive + "/" + nm,
                          "cell_type": kinds[(h + k) % len(kinds)]})
        for k in range(n - 1):
            edges.append({"id": "e-%d-%d" % (h, k),
                          "from": "/" + hive + "/" + names[k],
                          "to": "/" + hive + "/" + names[k + 1]})
        if h:
            edges.append({"id": "x-%d" % h,
                          "from": "/" + hives[h - 1] + "/c0",
                          "to": "/" + hive + "/c0",
                          "condition": "has(hop.route)"})
    return {"scope": "/", "nodes": nodes, "edges": edges}


def depth_of(path):
    return path.strip("/").count("/")


# ------------------------------------------------------------- the arithmetic

def isect(a, b):
    x = max(a[0], b[0])
    y = max(a[1], b[1])
    r = min(a[0] + a[2], b[0] + b[2])
    bo = min(a[1] + a[3], b[1] + b[3])
    return max(0, r - x) * max(0, bo - y)


def inside(inner, outer):
    return (inner[0] >= outer[0] and inner[1] >= outer[1]
            and inner[0] + inner[2] <= outer[0] + outer[2]
            and inner[1] + inner[3] <= outer[1] + outer[3])


def under(a, b):
    return a == b or a.startswith(b + "/")


def picture_of(emission, layout):
    """`(cells, frames)` as the display's objects would hold them."""
    cells, frames = {}, {}
    for child in emission["content"]["children"]:
        p = child["props"]
        if child["component"] == "colony-view-node":
            dx, dy = [int(v) for v in str(p["hand"]).split(",")]
            cells[p["path"]] = (p["x"] + dx, p["y"] + dy,
                                layout.NODE_W, layout.NODE_H)
        elif child["component"] == "colony-view-hive":
            frames[p["path"]] = (p["x"], p["y"], p["w"], p["h"])
    return cells, frames


def criteria(label, cells, frames, layout, k_outer, k_leaf):
    hs = sorted(frames)
    pairs = [(a, b) for a, b in itertools.combinations(hs, 2)
             if not under(a, b) and not under(b, a)]
    bad = [(a, b) for a, b in pairs if isect(frames[a], frames[b]) > 0]
    ok(label + ": no two unrelated frames overlap (%d pairs)" % len(pairs),
       not bad, bad[:3])

    worst_leaf, worst_outer = (0, None), (0, None)
    for h in hs:
        mine = [c for c in cells if under(c, h)]
        if not mine:
            continue
        area = len(mine) * layout.NODE_W * layout.NODE_H
        ratio = frames[h][2] * frames[h][3] / float(area)
        leaf = not any(under(o, h) and o != h for o in hs)
        if leaf and ratio > worst_leaf[0]:
            worst_leaf = (ratio, h)
        if not leaf and ratio > worst_outer[0]:
            worst_outer = (ratio, h)
    ok(label + ": a leaf frame is at most %gx the cells it holds" % k_leaf,
       worst_leaf[0] <= k_leaf, "%.2fx at %s" % worst_leaf)
    ok(label + ": a nesting frame is at most %gx the cells under it" % k_outer,
       worst_outer[0] <= k_outer, "%.2fx at %s" % worst_outer)

    foreign = [(h, c) for h in hs for c in cells
               if not under(c, h) and isect(frames[h], cells[c]) > 0]
    ok(label + ": no frame covers a cell that is not under it",
       not foreign, foreign[:3])

    out = [c for c in cells
           if layout.hive_of(c) in frames and not inside(cells[c], frames[layout.hive_of(c)])]
    ok(label + ": every cell is inside its own hive's frame", not out, out[:3])

    nest = [h for h in hs
            if layout.hive_of(h) in frames
            and not inside(frames[h], frames[layout.hive_of(h)])]
    ok(label + ": every child frame is inside its parent's", not nest, nest[:3])


def main():
    layout = load(sys.argv[1], "cv_layout")
    compose = load(sys.argv[2], "display_compose")
    graph = fixture()
    owner = "/os/orgs/acme/members/lea/apps/colony-view/layout"

    print("fixture: %d cells, %d edges, deepest hive %d levels"
          % (len(graph["nodes"]), len(graph["edges"]),
             max(depth_of(n["path"]) for n in graph["nodes"])))
    ok("the fixture nests at least four levels of hive",
       max(depth_of(n["path"]) for n in graph["nodes"]) >= 4)

    emission = layout.main.__globals__["content"](graph, owner)
    emission = {"content": emission}
    cells, frames = picture_of(emission, layout)

    # ---- 1. the picture nobody has touched IS the flow's picture
    criteria("untouched", cells, frames, layout, 40, 8)
    ok("an untouched box carries no offset and no pin",
       all(c["props"]["hand"] == "0,0" and c["props"]["pinned"] == ""
           for c in emission["content"]["children"]
           if c["component"] == "colony-view-node"))

    # ---- 2. the same picture after a hand has arranged it
    #
    # The BOUND is gone, and that is a decision rather than an omission. 1.0.1
    # trimmed a hand's offset to its own hive's frame, which made all five
    # counts hold for any arrangement -- and was measured on a real screen as
    # 85 x 15 pixels of travel before a wall, which reads as a broken gesture.
    # A constraint a person experiences as a broken gesture is a bug.
    #
    # So what is asserted here is what a picture still promises once a hand has
    # been in it: nothing moved that a hand did not move, every box that moved
    # says so, and the frames a viewer sees hold their own cells. Two frames CAN
    # now cover each other -- because somebody asked for that -- and the durable
    # answer is the app remembering the arrangement so the flow can pack around
    # it, which is a build and has its own issue.
    arranged = {}
    for i, child in enumerate(emission["content"]["children"]):
        if child["component"] != "colony-view-node":
            continue
        p = child["props"]
        want = [(-900, -900), (900, -900), (900, 900), (-900, 900),
                (0, 900), (-900, 0)][i % 6]
        p["hand"], p["pinned"] = "%d,%d" % want, "1"
        arranged[p["path"]] = want
    cells2, _ = picture_of(emission, layout)
    # The frames a viewer gets: re-derived from where the boxes ended up, which
    # is what the browser does on every drag frame and writes back to the store.
    placed = {k: (v[0], v[1]) for k, v in cells2.items()}
    frames2 = layout.hive_frames(placed)
    ok("arranged: every box that moved carries the marker",
       all(c["props"]["pinned"] == "1"
           for c in emission["content"]["children"]
           if c["component"] == "colony-view-node"))
    out = [c for c in cells2
           if layout.hive_of(c) in frames2
           and not inside(cells2[c], frames2[layout.hive_of(c)])]
    ok("arranged: the re-derived frame of a hive holds its own cells", not out, out[:3])
    nest = [h for h in frames2
            if layout.hive_of(h) in frames2
            and not inside(frames2[h], frames2[layout.hive_of(h)])]
    ok("arranged: a re-derived child frame is inside its parent's", not nest, nest[:3])
    ok("the hand's offset is ONE prop, so one drag is one picture",
       all(isinstance(c["props"]["hand"], str) and "," in c["props"]["hand"]
           for c in emission["content"]["children"]
           if c["component"] == "colony-view-node"))
    ok("no bound travels with the picture any more",
       not any(k in c["props"] for c in emission["content"]["children"]
               if c["component"] == "colony-view-node" for k in ("cx", "cw")))

    # ---- 3. identity: the id a drag writes to is the id the display mints
    #
    # The defect that made every measurement above possible. `data-oid` is the
    # only channel from this cell to the browser, and until 1.0.1 it named an
    # object one tree level away from the one `add_tree` had created -- so no
    # drag ever landed, and every "hand placed" box in the measurement was the
    # layout's own stale output.
    fresh = layout.main.__globals__["content"](graph, owner)
    wrapper = layout.wrapper_id(owner)
    want = {}
    compose.add_tree(want, wrapper, fresh, 0)
    claimed = {c["props"]["oid"] for c in fresh["children"]
               if c["component"] == "colony-view-node"}
    ok("every oid the picture claims is an object the display mints",
       claimed and claimed <= set(want), sorted(claimed - set(want))[:3])

    # ---- 4. identity survives a graph that grew an edge
    grown = fixture()
    grown["edges"].append({"id": "late", "from": "/os/gateway/c0",
                           "to": "/os/builder/c0"})
    after = layout.main.__globals__["content"](grown, owner)
    want2 = {}
    compose.add_tree(want2, wrapper, after, 0)
    keyed_before = {oid for oid, spec in want.items()
                    if spec["component"] == "colony-view-node"}
    keyed_after = {oid for oid, spec in want2.items()
                   if spec["component"] == "colony-view-node"}
    ok("one more edge does not move a single box's object id",
       keyed_before == keyed_after,
       sorted(keyed_before ^ keyed_after)[:3])
    ok("a box's object id names the cell, not its slot",
       all("/n." in oid for oid in keyed_before))
    ok("`x` and `y` are no longer kept -- the flow owns them",
       all(sorted(spec["keep"]) == ["hand", "pinned"]
           for spec in want2.values()
           if spec["component"] == "colony-view-node"))
    ok("a hive rectangle is an object the browser can correct",
       all("/h." in oid for oid, spec in want2.items()
           if spec["component"] == "colony-view-hive")
       and [c for c in layout.components() if c["name"] == "colony-view-hive"
            ][0]["editable"] == ["x", "y", "w", "h"])
    ok("the picture says which browser half wrote it",
       len(fresh["props"].get("client") or "") >= 8)

    # ---- 5. a frame says WHICH LEVEL it is, not only its directory name
    #
    # GH #549. A member is a person and an assistant is a generation of that
    # person's agent, so the obvious name for the first generation is the
    # person's own name: `members/alex` holding `assistants/alex` draws two
    # nested frames carrying one word, and the only way to tell the person from
    # the agent is to count rectangles. The composition levels address their
    # children through fixed containers (`orgs/`, `members/`, `assistants/`,
    # `channels/`, `apps/`), so the parent segment IS the level -- readable off
    # today's `/colony/graph`, with no new field on the wire and no second
    # consumer waiting for one. A hive that is not addressed through one of the
    # five is a plain `hive`, which is the honest answer rather than a guess.
    collision = {"scope": "/", "edges": [], "nodes": [
        {"path": "/" + h + "/c0", "cell_type": "code"} for h in (
            "os/builder",
            "os/orgs/acme",
            "os/orgs/acme/members/alex",
            "os/orgs/acme/members/alex/assistants/alex",
            "os/orgs/acme/members/alex/channels/telegram",
            "os/orgs/acme/members/alex/apps/colony-view",
        )]}
    labelled = layout.main.__globals__["content"](collision, owner)
    seen = {c["props"]["path"]: (c["props"]["name"], c["props"].get("level"))
            for c in labelled["children"]
            if c["component"] == "colony-view-hive"}
    want_level = {
        "os/builder": ("builder", "hive"),
        "os/orgs/acme": ("acme", "org"),
        "os/orgs/acme/members/alex": ("alex", "member"),
        "os/orgs/acme/members/alex/assistants/alex": ("alex", "assistant"),
        "os/orgs/acme/members/alex/channels/telegram": ("telegram", "channel"),
        "os/orgs/acme/members/alex/apps/colony-view": ("colony-view", "app"),
    }
    ok("every frame carries its level beside its directory name",
       all(seen.get(h) == w for h, w in want_level.items()),
       {h: (seen.get(h), w) for h, w in want_level.items() if seen.get(h) != w})
    ok("the two frames that share the word `alex` no longer read alike",
       (seen.get("os/orgs/acme/members/alex")
        != seen.get("os/orgs/acme/members/alex/assistants/alex")))
    hive_component = [c for c in layout.components()
                      if c["name"] == "colony-view-hive"][0]
    ok("the hive component declares `level` as a prop",
       hive_component["prop_schema"].get("level") == "text")
    # ONE `<text>`: the browser's `hiveParts` grabs the first one it finds to
    # place the label, so a second element would be positioned by nothing.
    ok("the frame renders `<name> · <level>` out of a single text element",
       "{{name}} \u00b7 {{level}}" in layout.HIVE_TEMPLATE
       and layout.HIVE_TEMPLATE.count("<text") == 1,
       layout.HIVE_TEMPLATE)

    print("\n%d checks, %d failed" % (COUNT[0], len(FAIL)))
    if FAIL:
        return 1
    print("all green")
    return 0


if __name__ == "__main__":
    sys.exit(main())
