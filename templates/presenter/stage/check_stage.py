#!/usr/bin/env python3
"""Table driver over the pure functions of the presenter's `stage` cell -- no colony.

    python3 templates/presenter/stage/check_stage.py          # run the tables
    python3 templates/presenter/stage/check_stage.py --sync   # write stage.py into config.json

It loads `stage.py` from beside itself, runs every table and prints one counter line per
table plus `STAGE <passed>/<total>`; it exits 1 on any red row, and 2 when the copy in
`config.json` (`params.script_inline`) is not byte-identical to `stage.py`. The tables
are: the questions of the one call, the threshold, the binding, the audience gate, the
fallbacks, and whole turns through the cell's own entry (`handle`) with a held clock.
"""

import copy
import importlib.util
import json
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
SOURCE = os.path.join(HERE, "stage.py")
CONFIG = os.path.join(HERE, "config.json")


def load():
    spec = importlib.util.spec_from_file_location("stage", SOURCE)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def sync():
    with open(CONFIG, encoding="utf-8") as f:
        cfg = json.load(f)
    with open(SOURCE, encoding="utf-8") as f:
        cfg["params"]["script_inline"] = f.read()
    with open(CONFIG, "w", encoding="utf-8") as f:
        f.write(json.dumps(cfg, indent=2, ensure_ascii=False) + "\n")


def in_sync():
    with open(CONFIG, encoding="utf-8") as f:
        cfg = json.load(f)
    with open(SOURCE, encoding="utf-8") as f:
        return cfg.get("params", {}).get("script_inline") == f.read()


# ---------------------------------------------------------------------------
# Fixtures: a small catalogue copy and the generic topic `sample`

CATALOG = [
    {"name": "display-card", "role": "content", "block": True, "slots": "none",
     "props": {"title": {"type": "text", "required": True},
               "body": {"type": "text", "required": False}}},
    {"name": "display-list", "role": "content", "block": True,
     "slots": ["display-item"], "props": {"title": {"type": "text"}}},
    {"name": "display-item", "role": "content", "block": False, "slots": "none",
     "props": {"k": {"type": "text", "required": True}, "v": {"type": "text"}}},
    {"name": "display-steps", "role": "content", "block": True,
     "slots": ["display-step"], "props": {"title": {"type": "text"}}},
    {"name": "display-step", "role": "content", "block": False, "slots": "none",
     "props": {"label": {"type": "text", "required": True}, "state": {"type": "text"}}},
    {"name": "display-value", "role": "content", "block": True, "slots": "none",
     "props": {"value": {"type": "number", "required": True},
               "unit": {"type": "text"}, "label": {"type": "text"}}},
    {"name": "display-chart", "role": "content", "block": True, "slots": "none",
     "props": {"figure": {"type": "html", "required": True},
               "caption": {"type": "text"}}},
]


def sample(**over):
    topic = {
        "topic": "sample", "title": "Sample", "describe": "a generic sample topic",
        "glyph": "S", "standard": "brief",
        "candidates": [
            {"key": "brief", "block": "display-card", "describe": "a short summary",
             "set": "brief", "bind": {"title": "brief.title", "body": "brief.body"}},
            {"key": "rows", "block": "display-list", "describe": "the items as a list",
             "set": "rows", "bind": {"title": "=Items"},
             "children": [{"each": "rows", "block": "display-item",
                           "bind": {"k": "$.name", "v": "$.note"}}]},
            {"key": "steps", "block": "display-steps", "describe": "the path as steps",
             "set": "steps", "bind": {"title": "=Path"},
             "children": [{"each": "steps", "block": "display-step",
                           "bind": {"label": "$.label"}}]},
            {"key": "slow", "block": "display-value", "describe": "a slow number",
             "set": "slow", "on_choice": True, "bind": {"value": "slow.n"}},
        ]}
    topic.update(over)
    return topic


class Table:
    def __init__(self, name):
        self.name, self.ok, self.n, self.red = name, 0, 0, []

    def check(self, label, got, want):
        self.n += 1
        if got == want:
            self.ok += 1
        else:
            self.red.append("%s: got %r, want %r" % (label, got, want))


# ---------------------------------------------------------------------------
# Tables


def t_manifest(s, t):
    cat = s.catalogue({"catalog": CATALOG})
    t.check("sample is well formed", s.check_topic(sample(), cat), None)
    # The shipped copy as `display_sync.py` writes it (blocks only, no `block` key).
    with open(CONFIG, encoding="utf-8") as f:
        shipped = s.catalogue(json.load(f)["params"])
    t.check("the shipped block copy is not empty", len(shipped) > 0, True)
    t.check("sample is well formed against the shipped copy",
            s.check_topic(sample(), shipped), None)
    bad = [
        ("no describe", sample(describe="  ")),
        ("dot in name", sample(topic="sa.mple")),
        ("threshold 0", sample(threshold=0)),
        ("threshold 1.5", sample(threshold=1.5)),
        ("standard missing", sample(standard="nope")),
        ("bind on a foreign set", sample(candidates=[dict(sample()["candidates"][0],
                                                          bind={"title": "rows.title"})]
                                        + sample()["candidates"][1:])),
        ("each over a foreign set", sample(candidates=sample()["candidates"][:1] + [
            dict(sample()["candidates"][1], children=[{"each": "slow", "block": "display-item",
                                                       "bind": {"k": "$.name"}}])]
            + sample()["candidates"][2:])),
        ("six candidates", sample(candidates=sample()["candidates"] + [
            dict(sample()["candidates"][0], key="b%d" % i) for i in range(2)])),
        ("no block", sample(candidates=[dict(sample()["candidates"][0],
                                             block="display-item")])),
        ("unknown prop", sample(candidates=[dict(sample()["candidates"][0],
                                                 bind={"nope": "brief.x"})])),
        ("html from a row", sample(candidates=[dict(sample()["candidates"][1],
                                                    children=[{"each": "rows",
                                                               "block": "display-chart",
                                                               "bind": {"figure": "$.svg"}}])])),
        ("html literal", sample(candidates=[{"key": "brief", "block": "display-chart",
                                             "describe": "x", "set": "brief",
                                             "bind": {"figure": "=<svg/>"}}])),
        ("child not in slot", sample(candidates=[dict(sample()["candidates"][1],
                                                      children=[{"each": "rows",
                                                                 "block": "display-step",
                                                                 "bind": {"label": "$.n"}}])])),
        ("children as an object", sample(candidates=[dict(sample()["candidates"][1],
                                                          children={"each": "rows",
                                                                    "block": "display-item",
                                                                    "bind": {"k": "$.n"}})])),
    ]
    for label, topic in bad:
        t.check(label + " is refused", s.check_topic(topic, cat) is not None, True)
    t.check("html from value is fine",
            s.check_topic(sample(candidates=[{"key": "brief", "block": "display-chart",
                                              "describe": "x", "set": "chart",
                                              "bind": {"figure": "chart.svg"}}]), cat),
            None)

    known, refused = s.accept_topics({}, "/a", [sample(), sample(describe="")], cat)
    t.check("one accepted", sorted(known), ["sample"])
    t.check("one refused invalid", [r[1].split(":")[0] for r in refused], ["invalid"])
    known2, refused2 = s.accept_topics(known, "/b", [sample()], cat)
    t.check("the older app keeps the topic", known2["sample"]["owner"], "/a")
    t.check("duplicate", refused2, [("sample", "duplicate")])
    many = [sample(topic="t%02d" % i) for i in range(s.MAX_TOPICS + 2)]
    known3, refused3 = s.accept_topics({}, "/a", many, cat)
    t.check("ceiling", len(known3), s.MAX_TOPICS)
    t.check("overflow alphabetically last", refused3,
            [("t%02d" % i, "overflow") for i in (s.MAX_TOPICS, s.MAX_TOPICS + 1)])
    known4, _ = s.accept_topics(known, "/a", [], cat)
    t.check("an app's empty list clears its rows", known4, {})


def t_questions(s, t):
    cat = s.catalogue({"catalog": CATALOG})
    known, _ = s.accept_topics({}, "/a", [sample(), sample(topic="other", title="Other")],
                               cat)
    qs = s.questions(known)
    t.check("1 + 2N questions", len(qs), 1 + 2 * 2)
    t.check("topic options", sorted(qs["topic"]["options"]), ["none", "other", "sample"])
    t.check("topic option says title and describe", qs["topic"]["options"]["sample"],
            "Sample: a generic sample topic")
    t.check("lead options", sorted(qs["sample.lead"]["options"]),
            ["brief", "rows", "slow", "steps"])
    t.check("also carries none", "none" in qs["sample.also"]["options"], True)
    t.check("kinds", sorted({q["kind"] for q in qs.values()}), ["choice"])
    call = s.decide_call("show me a sample", known, "t1")
    t.check("state is the turn text", call["decide"]["state"], "show me a sample")
    t.check("the call carries nothing else", sorted(call), ["decide", "header", "messages"])
    t.check("correlation on the hop", call["header"], {"route": "decide", "show_id": "t1"})
    full, _ = s.accept_topics({}, "/a", [sample(topic="t%02d" % i)
                                         for i in range(s.MAX_TOPICS)], cat)
    t.check("the ceiling fits the decider's 64", len(s.questions(full)) <= 64, True)


def t_threshold(s, t):
    cat = s.catalogue({"catalog": CATALOG})
    known, _ = s.accept_topics({}, "/a", [sample(), sample(topic="strict", threshold=0.95)],
                               cat)
    params = {}

    def v(topic, p):
        body = {"answers": {"topic": {"choice": topic, "p": {topic: p}}}, "model": "m"}
        return s.judge(s.read_verdict(body, known), known, params)

    t.check("0.7 is sure", v("sample", 0.7), None)
    t.check("0.69 is unsure", v("sample", 0.69), "unsure")
    t.check("the topic's threshold wins", v("strict", 0.9), "unsure")
    t.check("the topic's threshold reached", v("strict", 0.95), None)
    t.check("none", v("none", 0.99), "no_topic")
    t.check("a made-up topic", v("ghost", 0.99), "unsure")
    t.check("param default", s.judge(s.read_verdict(
        {"answers": {"topic": {"choice": "sample", "p": 0.6}}}, known), known,
        {"threshold": 0.5}), None)
    t.check("confidence spelling", s.answer_of(
        {"topic": {"choice": "sample", "confidence": 0.8}}, "topic"), ("sample", 0.8))
    t.check("missing answer", s.answer_of({}, "topic"), (None, 0.0))
    body = {"decision": {"answers": {
        "topic": {"choice": "sample", "p": {"sample": 0.9}},
        "sample.lead": {"choice": "ghost", "p": {"ghost": 0.9}},
        "sample.also": {"choice": "steps", "p": {"steps": 0.4}}}, "model": "m"}}
    vd = s.read_verdict(body, known)
    t.check("an unknown lead reads as none", vd["lead"], None)
    t.check("chosen falls to the standard", s.chosen(vd, known["sample"]["manifest"], {}),
            ["brief"])
    vd["also_p"] = 0.5
    t.check("also at its threshold", s.chosen(vd, known["sample"]["manifest"], {}),
            ["brief", "steps"])
    t.check("on_choice only when chosen",
            s.wanted_sets(known["sample"]["manifest"], ["rows"]), ["brief", "rows", "steps"])
    t.check("on_choice set when chosen",
            s.wanted_sets(known["sample"]["manifest"], ["slow"]),
            ["brief", "rows", "steps", "slow"])


def t_binding(s, t):
    cat = s.catalogue({"catalog": CATALOG})
    m = sample()
    c = {x["key"]: x for x in m["candidates"]}
    sets = {"brief": {"value": {"title": "Hello", "body": 3}},
            "rows": {"rows": [{"name": "a", "note": "x"}, {"name": "b"}]},
            "slow": {"value": {"n": "2.5"}}}
    t.check("scalar and number to text", s.place("sample", c["brief"], sets, cat),
            {"component": "display-card", "props": {"title": "Hello", "body": "3"},
             "key": "show-sample-brief"})
    rows = s.place("sample", c["rows"], sets, cat)
    t.check("each", [k["props"] for k in rows["children"]],
            [{"k": "a", "v": "x"}, {"k": "b"}])
    t.check("literal", rows["props"], {"title": "Items"})
    t.check("number type", s.place("sample", c["slow"], sets, cat)["props"], {"value": 2.5})
    t.check("missing required -> not placeable",
            s.place("sample", c["brief"], {"brief": {"value": {"body": "x"}}}, cat), None)
    t.check("wrong type -> not placeable",
            s.place("sample", c["slow"], {"slow": {"value": {"n": "many"}}}, cat), None)
    t.check("set missing -> not placeable", s.place("sample", c["steps"], sets, cat), None)
    t.check("each over no rows -> not placeable",
            s.place("sample", c["rows"], {"rows": {"rows": []}}, cat), None)
    t.check("a required child prop missing -> not placeable",
            s.place("sample", c["rows"], {"rows": {"rows": [{"note": "x"}]}}, cat), None)


def t_audience(s, t):
    cases = [
        (["*"], ["a"], True), (["a"], ["a"], True), (["a", "b"], ["a"], True),
        (["b"], ["a"], False), (None, ["a"], False), ([], ["a"], False),
        ("not json", ["a"], False), ('["a"]', ["a"], True), (["a"], [], False),
        (["*"], [], True), (["a"], None, False), (["a"], ["a", "b"], False),
        ([1], ["a"], False),
    ]
    for have, want, ok in cases:
        t.check("covers %r %r" % (have, want), s.covers(have, want), ok)
    data = {"audience_set": ["*"], "rows": [
        {"n": 1, "audience_set": ["a"]}, {"n": 2, "audience_set": ["a", "b"]},
        {"n": 3, "audience_set": ["b"]}, {"n": 4}, {"n": 5, "audience_set": ["*"]}]}
    # OR-DP-56: a row without its own round inherits the set's; a row with one is gated
    # alone.
    t.check("rows one by one", [r["n"] for r in s.filter_set(data, ["a"])["rows"]],
            [1, 2, 4, 5])
    t.check("an unmarked row inherits a covering set's round",
            [r["n"] for r in s.filter_set({"audience_set": ["a"], "rows": [{"n": 1}]},
                                          ["a"])["rows"]], [1])
    t.check("a set without a round takes its marked rows with it",
            s.filter_set({"rows": [{"n": 1, "audience_set": ["*"]}]}, ["a"]), None)
    t.check("a foreign set takes its marked rows with it",
            s.filter_set({"audience_set": ["b"], "rows": [{"n": 1, "audience_set": ["a"]}]},
                         ["a"]), None)
    t.check("a set of a foreign round is gone",
            s.filter_set({"audience_set": ["b"], "value": {"x": 1}}, ["a"]), None)
    t.check("a set without a round is gone", s.filter_set({"value": {"x": 1}}, ["a"]), None)
    t.check("an empty screen round sees * only",
            s.filter_set({"audience_set": ["a"], "value": {}}, []), None)


# ---------------------------------------------------------------------------
# Whole turns through `handle`, with a held clock


class Cell:
    """The cell as the substrate drives it: one doc at a time, the RAM kept."""

    def __init__(self, s, **params):
        self.s = s
        s.globals_reset = None
        for k in ("_RAM", "_TEST_NOW"):
            vars(s).pop(k, None)
        self.params = {"catalog": CATALOG}
        self.params.update(params)
        self.now = 1000000
        vars(s)["_TEST_NOW"] = self.now
        # A warm cell: the boot read answered with empty tables.
        self.store = {"shows": [], "pending": []}

    def tick(self, ms):
        self.now += ms
        vars(self.s)["_TEST_NOW"] = self.now

    def run(self, hop, body=None, ctx=None, reply_to="/x"):
        doc = {"envelope": {"header": {"hop": hop, "context": ctx or {}},
                            "reply_to": reply_to},
               "body": dict({"messages": []}, **(body or {})), "params": self.params}
        out = self.s.handle(doc)
        SEEN.extend(out)
        done = []
        for e in out:
            if e["header"]["route"] == "store" and e["header"].get("phase") == "boot":
                ans = {"messages": [
                    {"id": "b-shows", "text": json.dumps(self.store["shows"])},
                    {"id": "b-pending", "text": json.dumps(self.store["pending"])}]}
                done += self.s.handle({"envelope": {"header": {
                    "hop": {}, "context": {"show_origin": "store", "show_phase": "boot"}}},
                    "body": ans, "params": self.params})
                SEEN.extend(done)
            else:
                done.append(e)
        return done

    def pending(self, tid):
        """The pending record as the cell holds it -- no journal view on top, so a turn
        that never closes shows as open (review I1)."""
        return self.s.ram()["pending"].get(tid)


def routes(out):
    return [e["header"]["route"] for e in out if e["header"]["route"] not in ("store", "clock")]


def journal_of(out):
    """The journal rows a set of emissions writes."""
    rows = []
    for e in out:
        if e["header"]["route"] != "store":
            continue
        for leg in e["messages"]:
            a = json.loads(leg["text"])
            if a.get("operation") == "insert" and a.get("table") == "journal":
                rows.append(a["row"])
    return rows


def verdict(topic, p, lead=None, also=None, ap=0.0):
    a = {"topic": {"choice": topic, "p": {topic: p}}}
    if lead:
        a[topic + ".lead"] = {"choice": lead, "p": {lead: 0.9}}
    if also:
        a[topic + ".also"] = {"choice": also, "p": {also: ap}}
    return {"decision": {"answers": a, "model": "mock-decider", "ms": 5}, "messages": []}


# What the edge back from `./decide` really delivers: the inner context deleted,
# the hop restamped `in_decision` (config.json)
DEC_CTX = {}


def t_turns(s, t):
    # (d) a presenter without topics asks nothing
    c = Cell(s)
    t.check("no topics, no call", c.run({"route": "turn", "turn_id": "t0"},
                                        {"messages": [{"type": "text", "text": "hi"}]}), [])
    t.check("mutation asks for topics", routes(c.run({"route": "mutation_committed"})),
            ["in_show"])
    t.check("an empty show_app is the no-app sentinel",
            c.run({"route": "show_topics", "show_app": ""}, {"topics": [sample()]}), [])
    t.check("the sentinel wrote no row", s.ram()["shows"], {})
    out = c.run({"route": "show_topics", "show_app": "/app", "show_at": "./show"},
                {"topics": [sample()]})
    t.check("topics accepted, no refusal", routes(out), [])

    # a sure verdict: window + data in ONE output, nothing before it
    out = c.run({"route": "turn", "turn_id": "t1"},
                {"messages": [{"type": "text", "text": "show me a sample"}]},
                ctx={"audience_set": '["a"]'})
    t.check("the turn's output is the call alone", routes(out), ["decide"])
    c.tick(300)
    out = c.run({"route": "in_decision", "show_id": "t1"},
                verdict("sample", 0.9, "rows", "steps", 0.6), ctx=DEC_CTX)
    t.check("view and data request in one output", routes(out), ["view", "in_show"])
    view = out[0]
    stack = view["content"]["children"][1]["children"]
    t.check("the working hint", [(n["component"], n["props"]["kind"]) for n in stack],
            [("display-status", "working")])
    t.check("pane props", {k: view["content"]["props"][k] for k in
                           ("layer", "topic", "context", "relevance", "turn_id")},
            {"layer": "canvas", "topic": "show:sample", "context": "conversation",
             "relevance": "0.8", "turn_id": "t1"})
    ask = out[1]
    t.check("asks the owner", ask["header"].get("show_app"), "/app")
    t.check("at its connect point", ask["header"].get("show_at"), "./show")
    t.check("asks the sets", ask["sets"], ["brief", "rows", "steps"])
    t.check("the request is the turn", ask["request"], "show me a sample")
    c.tick(200)
    out = c.run({"route": "show_data", "show_app": "/app"}, {"topic": "sample", "turn_id": "t1", "sets": {
        "rows": {"audience_set": ["*"], "rows": [{"name": "a", "audience_set": ["*"]}]}}})
    blocks = out[0]["content"]["children"][1]["children"]
    t.check("the lead stands", [b["key"] for b in blocks], ["show-sample-rows"])
    t.check("no new touch", "touched" in out[0]["content"]["props"], False)
    j = journal_of(out)
    t.check("journal none", [(r["fallback"], r["lead"], r["also"]) for r in j],
            [("none", "rows", "steps")])
    t.check("times ordered", j[0]["t_verdict_ms"] <= j[0]["t_window_ms"] <= j[0]["t_content_ms"],
            True)
    t.check("journal carries no text", "show me a sample" in json.dumps(j), False)
    t.check("journal carries the turn's round", j[0]["audience_set"], '["a"]')
    out = c.run({"route": "show_data", "show_app": "/app"}, {"topic": "sample", "turn_id": "t1", "sets": {
        "steps": {"audience_set": ["*"], "rows": [{"label": "one", "audience_set": ["*"]}]}}})
    blocks = out[0]["content"]["children"][1]["children"]
    t.check("then the also", [b["key"] for b in blocks],
            ["show-sample-rows", "show-sample-steps"])
    t.check("no second journal row", journal_of(out), [])

    # (a) unsure
    c.run({"route": "turn", "turn_id": "t2"}, {"messages": [{"type": "text", "text": "x"}]})
    out = c.run({"route": "in_decision", "show_id": "t2"}, verdict("sample", 0.4), ctx=DEC_CTX)
    t.check("unsure: nothing on the screen", routes(out), [])
    t.check("unsure journal", [r["fallback"] for r in journal_of(out)], ["unsure"])
    # (b) none
    c.run({"route": "turn", "turn_id": "t3"}, {"messages": [{"type": "text", "text": "x"}]})
    out = c.run({"route": "in_decision", "show_id": "t3"}, verdict("none", 0.9), ctx=DEC_CTX)
    t.check("none journal", [r["fallback"] for r in journal_of(out)], ["no_topic"])
    # error and unconfigured
    c.run({"route": "turn", "turn_id": "t4"}, {"messages": [{"type": "text", "text": "x"}]})
    out = c.run({"route": "in_decision", "show_id": "t4", "finish_reason": "error",
                 "error_code": "decisions_unconfigured"}, {}, ctx=DEC_CTX)
    t.check("no selector", ([r["fallback"] for r in journal_of(out)], routes(out)),
            (["no_selector"], []))
    c.run({"route": "turn", "turn_id": "t5"}, {"messages": [{"type": "text", "text": "x"}]})
    out = c.run({"route": "in_decision", "show_id": "t5", "finish_reason": "error",
                 "error_code": "upstream_error"}, {}, ctx=DEC_CTX)
    t.check("error", [r["fallback"] for r in journal_of(out)], ["error"])
    # (c) timeout, then late
    out = c.run({"route": "turn", "turn_id": "t6"},
                {"messages": [{"type": "text", "text": "x"}]})
    orders = [e for e in out if e["header"]["route"] == "clock" and e.get("op") == "add"]
    t.check("one order for the deadline", len(orders), 1)
    c.tick(1000)
    out = c.run({"route": "in_tick", "schedule_id": orders[0]["schedule_id"]})
    t.check("timeout journal", [r["fallback"] for r in journal_of(out)], ["timeout"])
    out = c.run({"route": "in_decision", "show_id": "t6"}, verdict("sample", 0.9, "rows"),
                ctx=DEC_CTX)
    t.check("late: still nothing", routes(out), [])
    t.check("late journal", [(r["fallback"], r["late"]) for r in journal_of(out)],
            [("timeout", 1)])
    # P.7 invalid lead -> standard; no data -> withdraw
    c2 = Cell(s, screen_audience=["a"])
    c2.run({"route": "show_topics", "show_app": "/app"}, {"topics": [sample()]})
    c2.run({"route": "turn", "turn_id": "u1"}, {"messages": [{"type": "text", "text": "x"}]})
    c2.run({"route": "in_decision", "show_id": "u1"}, verdict("sample", 0.9, "rows"), ctx=DEC_CTX)
    out = c2.run({"route": "show_data", "show_app": "/app"}, {"topic": "sample", "turn_id": "u1", "sets": {
        "rows": {"audience_set": ["*"], "rows": [{"name": "b", "audience_set": ["b"]}]},
        "brief": {"audience_set": ["*"], "value": {"title": "Brief"}}}})
    blocks = out[0]["content"]["children"][1]["children"]
    t.check("standard instead", [b["key"] for b in blocks], ["show-sample-brief"])
    t.check("invalid journal", [r["fallback"] for r in journal_of(out)], ["invalid"])
    out = c2.run({"route": "turn", "turn_id": "u2"},
                 {"messages": [{"type": "text", "text": "x"}]})
    c2.run({"route": "in_decision", "show_id": "u2"}, verdict("sample", 0.9, "rows"), ctx=DEC_CTX)
    c2.tick(4000)
    out = c2.run({"route": "in_tick"})
    t.check("no data: withdraw", routes(out), ["withdraw"])
    t.check("no data journal", [r["fallback"] for r in journal_of(out)], ["no_data"])
    # a killed child rebuilds from the store and answers the same
    c3 = Cell(s)
    c3.store["shows"] = [{"topic": "sample", "owner_app": "/app", "manifest": json.dumps(sample()),
                          "at": 1}]
    out = c3.run({"route": "turn", "turn_id": "k1"},
                 {"messages": [{"type": "text", "text": "x"}]})
    t.check("cold child: boot then the call", routes(out), ["decide"])
    rec = copy.deepcopy(s.ram()["pending"]["k1"])
    c4 = Cell(s)
    c4.store["shows"] = c3.store["shows"]
    c4.store["pending"] = [{"turn_id": "k1", "value": json.dumps(rec), "at": 1}]
    out = c4.run({"route": "in_decision", "show_id": "k1"}, verdict("sample", 0.9, "rows"),
                 ctx=DEC_CTX)
    t.check("killed child: the verdict still opens the window", routes(out),
            ["view", "in_show"])


def opened(c, tid, text="the sample please", lead="rows", also=None, ap=0.0):
    """A turn and its sure verdict: the window stands, the data request is out."""
    c.run({"route": "turn", "turn_id": tid}, {"messages": [{"type": "text", "text": text}]})
    return c.run({"route": "in_decision", "show_id": tid},
                 verdict("sample", 0.9, lead, also, ap), ctx=DEC_CTX)


def data(c, tid, sets, app="/app"):
    return c.run({"route": "show_data", "show_app": app},
                 {"topic": "sample", "turn_id": tid, "sets": sets})


ROWS = {"rows": {"audience_set": ["*"], "rows": [{"name": "a"}]}}
STEPS = {"steps": {"audience_set": ["*"], "rows": [{"label": "one"}]}}


def pending_rows(out):
    """The pending values a set of emissions writes."""
    rows = []
    for e in out:
        if e["header"]["route"] != "store":
            continue
        for leg in e["messages"]:
            a = json.loads(leg["text"])
            if a.get("operation") == "insert" and a.get("table") == "pending":
                rows.append(a["row"]["value"])
    return rows


def t_lifecycle(s, t):
    """Review I1-I3: a shown turn closes, a newer turn of the same topic replaces the
    older one, and data counts only from the topic's owner."""
    # I1 (a): every chosen block placed -> the turn is done at once, text and data gone
    c = Cell(s)
    c.run({"route": "show_topics", "show_app": "/app"}, {"topics": [sample()]})
    opened(c, "l1", lead="rows", also="steps", ap=0.6)
    data(c, "l1", ROWS)
    t.check("one of two blocks: still open", c.pending("l1")["state"], "showing")
    out = data(c, "l1", STEPS)
    rec = c.pending("l1")
    t.check("all blocks placed: done", rec["state"], "done")
    t.check("all blocks placed: no text kept", rec["text"], "")
    t.check("all blocks placed: no data kept", (rec["sets"], rec.get("_nodes")), ({}, []))
    t.check("the closing write carries no text",
            "the sample please" in json.dumps(pending_rows(out)), False)
    t.check("closing keeps the journal word", rec["fallback"], "none")
    # I1 (b): the also never comes -> data_wait_ms closes it, the blocks stay
    c.tick(10)
    opened(c, "l2", lead="rows", also="steps", ap=0.6)
    out = data(c, "l2", ROWS)
    t.check("an open also keeps an order on the clock",
            [e.get("op") for e in out if e["header"]["route"] == "clock"].count("add") +
            (1 if s.ram()["due"] else 0) > 0, True)
    c.tick(4000)
    out = c.run({"route": "in_tick"})
    rec = c.pending("l2")
    t.check("data_wait_ms closes a shown turn", rec["state"], "done")
    t.check("closing a shown turn withdraws nothing", routes(out), [])
    t.check("closing a shown turn: no text", (rec["text"], rec["sets"]), ("", {}))
    t.check("closing a shown turn: journal word stays", rec["fallback"], "none")
    t.check("closing a shown turn writes no text",
            "the sample please" in json.dumps(pending_rows(out)), False)
    # I1 (c): pending does not grow -- a closed turn leaves after KEEP_DONE_MS
    c.tick(s.KEEP_DONE_MS + 1)
    c.run({"route": "turn", "turn_id": "l3"}, {"messages": [{"type": "text", "text": "x"}]})
    t.check("closed turns are pruned", sorted(s.ram()["pending"]), ["l3"])

    # I2 (a): a newer turn of the same topic replaces an older open one
    c = Cell(s)
    c.run({"route": "show_topics", "show_app": "/app"}, {"topics": [sample()]})
    opened(c, "a1")
    c.tick(1000)
    out = opened(c, "b1")
    t.check("the newer turn opens the window", routes(out), ["view", "in_show"])
    t.check("the older turn is done", c.pending("a1")["state"], "done")
    t.check("the older turn keeps no text", c.pending("a1")["text"], "")
    t.check("the older turn journals no_data",
            [(r["turn_id"], r["fallback"]) for r in journal_of(out)], [("a1", "no_data")])
    c.tick(3100)  # past a1's data deadline, before b1's
    out = c.run({"route": "in_tick"})
    t.check("the older deadline withdraws nothing", routes(out), [])
    out = data(c, "a1", ROWS)
    t.check("late data of the older turn draws nothing", routes(out), [])
    out = data(c, "b1", ROWS)
    t.check("the newer turn still fills", routes(out), ["view"])
    t.check("with its own turn id", out[0]["content"]["props"]["turn_id"], "b1")
    # I2 (b): an older verdict after the newer window opens nothing
    c.tick(10)
    c.run({"route": "turn", "turn_id": "a2"}, {"messages": [{"type": "text", "text": "x"}]})
    c.tick(10)
    opened(c, "b2")
    out = c.run({"route": "in_decision", "show_id": "a2"}, verdict("sample", 0.9, "rows"),
                ctx=DEC_CTX)
    t.check("an older verdict leaves the newer window", routes(out), [])
    t.check("the older verdict is done", c.pending("a2")["state"], "done")

    # I3: data only from the topic's owner
    c = Cell(s)
    c.run({"route": "show_topics", "show_app": "/app"}, {"topics": [sample()]})
    opened(c, "o1")
    # Marked rows: the stranger's set would place on its own (no audience gate between).
    marked = {"rows": {"audience_set": ["*"], "rows": [{"name": "a", "audience_set": ["*"]}]}}
    out = data(c, "o1", marked, app="/stranger")
    t.check("a stranger's data draws nothing", [r for r in routes(out) if r != "error"], [])
    t.check("a stranger's data is said out loud",
            [e["header"].get("error_code") for e in out if e["header"]["route"] == "error"],
            ["foreign_data"])
    t.check("a stranger's set did not land", c.pending("o1")["sets"], {})
    out = c.run({"route": "show_data"}, {"topic": "sample", "turn_id": "o1", "sets": ROWS})
    t.check("data without an owner stamp draws nothing",
            [r for r in routes(out) if r != "error"], [])
    out = data(c, "o1", ROWS)
    t.check("the owner's data draws", routes(out), ["view"])


SEEN = []


def t_contract(s, t):
    """Every emission the turn tables saw stands in `contract.emits` of config.json
    (lesson B2: the substrate refuses an undeclared route value, and a lock that never
    compares the two finds out on a colony)."""
    with open(CONFIG, encoding="utf-8") as f:
        emits = json.load(f)["contract"]["emits"]
    routes = set(emits["hop"]["route"]["values"])
    t.check("the turn tables emitted", len(SEEN) > 20, True)
    shows = [e for e in SEEN if e["header"]["route"] == "in_show"]
    t.check("data requests were seen", any(e.get("op") == "data" for e in shows), True)
    for e in shows:
        # A-review M-2: an `in_show` without `show_app` is the topics question to EVERY
        # app; a data request or a refusal must name its one app.
        addressed = bool(e["header"].get("show_app"))
        t.check("in_show %s addressed" % e.get("op"), addressed, e.get("op") != "topics")
    for e in SEEN:
        head = e["header"]
        t.check("route %s declared" % head["route"], head["route"] in routes, True)
        for k in head:
            t.check("hop %s declared" % k, k in emits["hop"], True)
        for k, v in e.items():
            if k == "header":
                continue
            t.check("body %s declared" % k, k in emits["body"], True)
            want = emits["body"].get(k, {}).get("type")
            kind = {"array": list, "object": dict, "string": str}.get(want)
            if kind is not None:
                t.check("body %s is %s" % (k, want), isinstance(v, kind), True)


TABLES = [("MANIFEST", t_manifest), ("QUESTIONS", t_questions), ("THRESHOLD", t_threshold),
          ("BINDING", t_binding), ("AUDIENCE", t_audience), ("TURNS", t_turns),
          ("LIFECYCLE", t_lifecycle),
          ("CONTRACT", t_contract)]


def main(argv):
    if "--sync" in argv:
        sync()
        print("synced %s" % os.path.relpath(CONFIG))
        return 0
    s = load()
    ok = n = 0
    red = []
    for name, fn in TABLES:
        t = Table(name)
        try:
            fn(s, t)
        except Exception as e:  # a crash is a red row, never a traceback instead of a count
            t.n += 1
            t.red.append("crashed: %r" % e)
        print("%s %d/%d" % (name, t.ok, t.n))
        ok += t.ok
        n += t.n
        red += ["%s: %s" % (name, r) for r in t.red]
    print("STAGE %d/%d" % (ok, n))
    for r in red:
        print("  RED " + r)
    if not in_sync():
        print("DRIFT config.json params.script_inline differs from stage.py "
              "(run with --sync)")
        return 2
    return 1 if red else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
