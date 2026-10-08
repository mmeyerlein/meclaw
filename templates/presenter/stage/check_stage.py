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
        # OR-DP-77: the candidate's keys are closed -- a key nobody reads is refused.
        ("an unknown candidate key", sample(candidates=[dict(sample()["candidates"][0],
                                                             width=3)]
                                           + sample()["candidates"][1:])),
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
    # A one-candidate topic: a choice of one option would void the whole call (the wire
    # refuses fewer than two options); its lead is the standard, not a question.
    one = sample(standard="brief", candidates=sample()["candidates"][:1])
    t.check("a one-candidate topic is well formed", s.check_topic(one, cat), None)
    known1, _ = s.accept_topics({}, "/a", [one], cat)
    qs1 = s.questions(known1)
    t.check("no lead question for one candidate", "sample.lead" in qs1, False)
    t.check("every choice offers at least two options",
            min(len(q["options"]) for q in qs1.values()), 2)
    v = s.read_verdict({"answers": {"topic": {"choice": "sample", "p": 0.9}}}, known1)
    t.check("its standard leads", s.chosen(v, known1["sample"]["manifest"], {})[:1],
            ["brief"])


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
    # OR-HP-66: a threshold per topic of the presenter's own (`builtin_thresholds`); the
    # decider gave 0.57-0.62 for plain search and work requests against the 0.7 of all.
    own = dict(s.builtin_topics({"observed_topics": ["search", "work"], "catalog": CATALOG}))
    own.update(known)

    def o(topic, p, **params):
        body = {"answers": {"topic": {"choice": topic, "p": {topic: p}}}, "model": "m"}
        return s.judge(s.read_verdict(body, own), own, params)

    low = {"search": 0.55}
    t.check("own threshold: search at 0.6 is sure", o("search", 0.6, builtin_thresholds=low),
            None)
    t.check("own threshold: search at 0.5 is not", o("search", 0.5, builtin_thresholds=low),
            "unsure")
    t.check("own threshold: work keeps the default", o("work", 0.6, builtin_thresholds=low),
            "unsure")
    t.check("own threshold: as JSON text", o("search", 0.6, builtin_thresholds=json.dumps(low)),
            None)
    t.check("own threshold: out of (0, 1] keeps the default",
            o("search", 0.6, builtin_thresholds={"search": 1.5}), "unsure")
    t.check("own threshold: not for an app's topic",
            o("sample", 0.6, builtin_thresholds={"sample": 0.1}), "unsure")
    t.check("own threshold: default unchanged", o("search", 0.69), "unsure")
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
        self.store = {"shows": [], "pending": [], "work": []}

    def tick(self, ms):
        self.now += ms
        vars(self.s)["_TEST_NOW"] = self.now

    def run(self, hop, body=None, ctx=None, reply_to="/x"):
        screen = self.params.get("screen_audience")
        if ctx is None and hop.get("route") == "turn" and screen:
            # The member's observer edge stamps the member round on every turn, and a
            # screen with a round shows that round: a turn without one opens nothing
            # there (GH #1027). A table that wants a foreign or missing round says so.
            ctx = {"audience_set": screen if isinstance(screen, str) else json.dumps(screen)}
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
                    {"id": "b-pending", "text": json.dumps(self.store["pending"])},
                    {"id": "b-work", "text": json.dumps(self.store["work"])}]}
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
    c.tick(s.DEFAULTS["budget_ms"])
    out = c.run({"route": "in_tick", "schedule_id": orders[0]["schedule_id"]})
    t.check("timeout journal", [r["fallback"] for r in journal_of(out)], ["timeout"])
    out = c.run({"route": "in_decision", "show_id": "t6"}, verdict("sample", 0.9, "rows"),
                ctx=DEC_CTX)
    t.check("late: still nothing", routes(out), [])
    t.check("late journal", [(r["fallback"], r["late"]) for r in journal_of(out)],
            [("timeout", 1)])
    # P.7 invalid lead -> standard; no data -> withdraw. The lead's set came, visible, and
    # cannot fill its block (a `value`, no rows). A set left empty by the round gate is
    # no longer this case: it is a set that never came (N review I-1, FOLLOWUP).
    c2 = Cell(s, screen_audience=["a"])
    c2.run({"route": "show_topics", "show_app": "/app"}, {"topics": [sample()]})
    c2.run({"route": "turn", "turn_id": "u1"}, {"messages": [{"type": "text", "text": "x"}]})
    c2.run({"route": "in_decision", "show_id": "u1"}, verdict("sample", 0.9, "rows"), ctx=DEC_CTX)
    out = c2.run({"route": "show_data", "show_app": "/app"}, {"topic": "sample", "turn_id": "u1", "sets": {
        "rows": {"audience_set": ["*"], "value": {"n": 1}},
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


def partial(topic, p, lead=None, also=None, ap=0.0, missing=(), drop=()):
    """A verdict the decider answered in part (GH #977): `drop` leaves keys out of the
    answers, `missing` is what the cell says it did not get."""
    v = verdict(topic, p, lead, also, ap)
    for k in drop:
        v["decision"]["answers"].pop(k, None)
    if missing:
        v["decision"]["missing"] = sorted(missing)
    return v


def t_partial(s, t):
    """GH #977: one omitted answer no longer tips the whole verdict (OR-DP-84 b)."""
    other = sample(topic="other", title="Other", describe="another topic")
    c = Cell(s, screen_audience=["a"])
    c.run({"route": "show_topics", "show_app": "/app"}, {"topics": [sample(), other]})
    # another topic's lead is missing: the sure topic still opens its window
    c.run({"route": "turn", "turn_id": "m1"}, {"messages": [{"type": "text", "text": "x"}]})
    out = c.run({"route": "in_decision", "show_id": "m1"},
                partial("sample", 0.9, "rows", missing=["other.lead"]), ctx=DEC_CTX)
    t.check("missing other: window and data request", routes(out), ["view", "in_show"])
    out = data(c, "m1", ROWS)
    j = journal_of(out)
    t.check("missing other: the lead stands, journal none",
            [(r["fallback"], r["lead"]) for r in j], [("none", "rows")])
    t.check("missing other: journal names the key", [r.get("missing") for r in j],
            ['["other.lead"]'])
    # a whole verdict journals an empty list, never a missing column
    c.run({"route": "turn", "turn_id": "m0"}, {"messages": [{"type": "text", "text": "x"}]})
    c.run({"route": "in_decision", "show_id": "m0"}, verdict("sample", 0.9, "rows"),
          ctx=DEC_CTX)
    out = data(c, "m0", ROWS)
    t.check("whole verdict: missing is empty", [r.get("missing") for r in journal_of(out)],
            ["[]"])
    # the chosen topic's lead is missing: its standard leads, as if the lead could not stand
    c.run({"route": "turn", "turn_id": "m2"}, {"messages": [{"type": "text", "text": "x"}]})
    out = c.run({"route": "in_decision", "show_id": "m2"},
                partial("sample", 0.9, missing=["sample.lead"]), ctx=DEC_CTX)
    t.check("missing lead: the window opens", routes(out), ["view", "in_show"])
    t.check("missing lead: the standard is asked for", out[1]["lead"], "brief")
    out = data(c, "m2", {"brief": {"audience_set": ["*"], "value": {"title": "Brief"}}})
    blocks = out[0]["content"]["children"][1]["children"]
    t.check("missing lead: the standard stands", [b["key"] for b in blocks],
            ["show-sample-brief"])
    t.check("missing lead: journal invalid + key",
            [(r["fallback"], r["missing"]) for r in journal_of(out)],
            [("invalid", '["sample.lead"]')])
    # the chosen topic's also is missing: no also
    c.run({"route": "turn", "turn_id": "m3"}, {"messages": [{"type": "text", "text": "x"}]})
    out = c.run({"route": "in_decision", "show_id": "m3"},
                partial("sample", 0.9, "rows", missing=["sample.also"]), ctx=DEC_CTX)
    t.check("missing also: only the lead is asked", (out[1]["lead"], out[1]["also"]),
            ("rows", []))
    # without the topic question nothing shows; the journal says error and the key
    c.run({"route": "turn", "turn_id": "m4"}, {"messages": [{"type": "text", "text": "x"}]})
    out = c.run({"route": "in_decision", "show_id": "m4"},
                partial("sample", 0.9, "rows", missing=["topic"], drop=["topic"]),
                ctx=DEC_CTX)
    t.check("missing topic: nothing on the screen", routes(out), [])
    t.check("missing topic: journal error + key",
            [(r["fallback"], r["missing"]) for r in journal_of(out)],
            [("error", '["topic"]')])
    # a topic answer absent with no `missing` beside it reads the same (fail-closed)
    c.run({"route": "turn", "turn_id": "m5"}, {"messages": [{"type": "text", "text": "x"}]})
    out = c.run({"route": "in_decision", "show_id": "m5"},
                partial("sample", 0.9, "rows", drop=["topic"]), ctx=DEC_CTX)
    t.check("absent topic: error", ([r["fallback"] for r in journal_of(out)], routes(out)),
            (["error"], []))
    # a junk `missing` is read as keys only, sorted, never trusted further
    t.check("missing read", s.missing_of({"decision": {"missing": ["b", 3, "a", "a"]}}),
            ["a", "b"])
    t.check("missing absent", s.missing_of({"decision": {}}), [])
    # a late, partial verdict changes nothing on the screen
    c.run({"route": "turn", "turn_id": "m6"}, {"messages": [{"type": "text", "text": "x"}]})
    c.tick(s.DEFAULTS["budget_ms"])
    c.run({"route": "in_tick"})
    out = c.run({"route": "in_decision", "show_id": "m6"},
                partial("sample", 0.9, "rows", missing=["other.lead"]), ctx=DEC_CTX)
    t.check("late partial: nothing", routes(out), [])
    t.check("late partial journal", [(r["fallback"], r["late"], r["missing"])
                                     for r in journal_of(out)],
            [("timeout", 1, '["other.lead"]')])


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


# ---------------------------------------------------------------------------
# GH #963: tool results as data, the topics `search` and `work`, declared sources

ROUND = ["agent:g1", "member:p"]
OTHER = ["agent:g1", "member:q"]
SCREEN = ["member:p"]


def observing(s, **params):
    """A warm cell with both observed topics on and the generic app topic installed."""
    with open(CONFIG, encoding="utf-8") as f:
        shipped = json.load(f)["params"]["catalog"]
    base = {"observed_topics": ["search", "work"], "screen_audience": SCREEN,
            "catalog": shipped}
    base.update(params)
    c = Cell(s, **base)
    c.run({"route": "show_topics", "show_app": "/app"}, {"topics": [sample()]})
    return c


def call(c, tool, cid, args, ctx=None, hop=None):
    """One observed tool call, as the builder's tap delivers it (`in_tool_call`)."""
    h = {"route": "in_tool_call", "tool_name": tool, "tool_call_id": cid}
    h.update(hop or {})
    return c.run(h, {"messages": [{"origin": "assistant", "type": "tool_call", "id": cid,
                                   "text": json.dumps(args)}], "arguments": args},
                 ctx=dict({"audience_set": json.dumps(ROUND), "tool_caller": "talky",
                           "assistant": "g1", "session_id": "s1"}, **(ctx or {})))


def result(c, cid, text, ctx=None, hop=None):
    """One observed tool result, as the string-form observer delivers it (`tool_result`)."""
    h = {"route": "tool_result"}
    h.update(hop or {})
    return c.run(h, {"messages": [{"origin": "tool", "type": "tool_result", "id": cid,
                                   "text": text}]},
                 ctx=dict({"audience_set": json.dumps(ROUND), "tool_caller": "talky",
                           "session_id": "s1"}, **(ctx or {})))


HITS = json.dumps([
    {"title": "Alpha", "url": "https://a.example/x", "snippet": "first hit"},
    {"title": "Beta", "url": "javascript:alert(1)", "snippet": "a bad link"},
    {"title": "Gamma", "url": "http://a.example/y", "snippet": "third"}])


def screen_ids(out):
    """Every node key of every view the emissions carry."""
    ids = []

    def walk(n):
        if isinstance(n, dict):
            if isinstance(n.get("key"), str):
                ids.append(n["key"])
            for k in n.get("children") or []:
                walk(k)
    for e in out:
        if e["header"]["route"] == "view":
            walk(e["content"])
    return ids


def last_blocks(out):
    views = [e for e in out if e["header"]["route"] == "view"]
    return views[-1]["content"]["children"][1]["children"] if views else []


def work_rows(out):
    rows = []
    for e in out:
        if e["header"]["route"] != "store":
            continue
        for leg in e["messages"]:
            a = json.loads(leg["text"])
            if a.get("operation") == "insert" and a.get("table") == "work":
                rows.append(a["row"])
    return rows


def t_observe(s, t):
    """Q.1: an observation is data. It opens no window and asks the decider nothing."""
    c = observing(s)
    out = call(c, "web_search", "c1", {"query": "rust actors"})
    out += result(c, "c1", HITS, hop={"operation": "web_search", "result_count": 3})
    out += call(c, "bash", "c2", {"command": "cargo test -p x"})
    out += result(c, "c2", "ok", hop={"operation": "bash", "exit_code": 0})
    t.check("observations open no window and ask nothing",
            [r for r in routes(out) if r in ("view", "decide", "in_show", "withdraw")], [])
    rows = work_rows(out)
    t.check("one work row per call, then its state",
            [(r["tool"], r["state"]) for r in rows],
            [("web_search", "running"), ("web_search", "done"), ("bash", "running"),
             ("bash", "done")])
    t.check("a row carries the call's round", rows[0]["audience_set"], json.dumps(ROUND))
    t.check("a row carries no output", "first hit" in json.dumps(rows), False)
    t.check("a search row says the query", rows[0]["summary"], "search: rust actors")
    t.check("a test command is named a test run", rows[2]["summary"], "test run")
    t.check("exit code kept", rows[3]["exit_code"], 0)
    # A result whose call was not observed is not ours (the memory leg of the string-form
    # observer serves every generation of the member).
    t.check("a stray result is ignored", result(c, "zz", "x", hop={"operation": "bash"}), [])
    # A tool outside the closed list is not recorded.
    t.check("an unobserved tool leaves no row",
            work_rows(call(c, "memory_recall", "c3", {"q": "x"})), [])
    # Without observed topics the presenter keeps nothing.
    c0 = Cell(s)
    t.check("observed topics off: nothing kept", call(c0, "bash", "d1", {"command": "ls"}), [])

    # Turn association: by turn_id where the call carries one, else the newest open turn of
    # the same round, never a turn of another round, never a closed one. On a screen
    # without a round: a turn of another round on the member's screen is closed on arrival
    # since GH #1027 (`FOREIGN`), and this table is about the association alone.
    c = observing(s, screen_audience=[])
    c.run({"route": "turn", "turn_id": "t1"}, {"messages": [{"type": "text", "text": "a"}]},
          ctx={"audience_set": json.dumps(ROUND)})
    c.tick(5)
    c.run({"route": "turn", "turn_id": "t2"}, {"messages": [{"type": "text", "text": "b"}]},
          ctx={"audience_set": json.dumps(OTHER)})
    r1 = work_rows(call(c, "bash", "e1", {"command": "ls"}))
    t.check("the newest open turn of the same round", r1[0]["turn_id"], "t1")
    r2 = work_rows(call(c, "bash", "e2", {"command": "ls"}, ctx={"turn_id": "t2"}))
    t.check("a turn_id on the call wins", r2[0]["turn_id"], "t2")
    r3 = work_rows(call(c, "bash", "e3", {"command": "ls"}, ctx={"audience_set": None}))
    t.check("a call without a round belongs to no turn", (r3[0]["turn_id"],
                                                          r3[0]["audience_set"]), ("", ""))
    c.run({"route": "in_decision", "show_id": "t1"}, verdict("none", 0.9), ctx=DEC_CTX)
    r4 = work_rows(call(c, "bash", "e4", {"command": "ls"}))
    t.check("a closed turn takes no call", r4[0]["turn_id"], "")

    # The rolling view: at most WORK_ROWS rows per session, the oldest leaves.
    c = observing(s)
    out = []
    for i in range(s.WORK_ROWS + 3):
        out += call(c, "bash", "w%d" % i, {"command": "ls"})
    kept = [r for r in s.ram()["work"] if r["session"] == "s1"]
    t.check("the cap per session", len(kept), s.WORK_ROWS)
    t.check("the oldest left", min(r["seq"] for r in kept), 4)
    dels = [json.loads(leg["text"]) for e in out if e["header"]["route"] == "store"
            for leg in e["messages"]]
    t.check("the store follows the cap",
            len([a for a in dels if a.get("operation") == "delete" and a.get("table") == "work"
                 and "seq" in a.get("where", {})]), 3)
    # The pure helpers
    t.check("test pattern", s.is_test("python3 -m pytest -q", s.DEFAULT_TEST_PATTERNS), True)
    t.check("no test pattern", s.is_test("ls -la", s.DEFAULT_TEST_PATTERNS), False)
    t.check("a param list wins", s.is_test("ls -la", ["ls"]), True)
    t.check("file row names its path", s.summarise("file", {"op": "read", "path": "a/b.md"},
                                                   s.DEFAULT_TEST_PATTERNS),
            ("read a/b.md", "a/b.md"))
    t.check("a plain command is not shown", s.summarise("bash", {"command": "cat secret"},
                                                        s.DEFAULT_TEST_PATTERNS),
            ("shell command", ""))


def t_search(s, t):
    """Q.2: `search` -- a sure verdict opens the window, the observed result fills it."""
    hits = s.parse_hits(HITS, ROUND)
    t.check("links only http(s)", [h["url"] for h in hits],
            ["https://a.example/x", "", "http://a.example/y"])
    t.check("host from the link", [h["host"] for h in hits], ["a.example", "", "a.example"])
    t.check("the generic {results} shape reads too",
            len(s.parse_hits(json.dumps({"results": json.loads(HITS)}), ROUND)), 3)
    t.check("no JSON, no hits", s.parse_hits("not json", ROUND), [])
    sets = s.search_sets(hits, SCREEN)
    t.check("the three sets", sorted(sets), ["hits", "sources", "top"])
    t.check("top is the first hit", sets["top"]["value"]["title"], "Alpha")
    t.check("sources as one value", "<td>a.example</td><td>2</td>" in
            sets["sources"]["value"]["rows"], True)
    t.check("a foreign round's hits make no sets", s.search_sets(s.parse_hits(HITS, OTHER),
                                                                 SCREEN), {})
    t.check("the built-in manifests stand against the shipped catalogue",
            [s.check_topic(m, s.catalogue(json.load(open(CONFIG, encoding="utf-8"))["params"]),
                           own=True) for m in s.BUILTIN.values()], [None, None])

    # The flow: turn -> sure `search` -> hint, no data request; the result fills it.
    c = observing(s)
    c.run({"route": "turn", "turn_id": "t1"},
          {"messages": [{"type": "text", "text": "search rust actors"}]},
          ctx={"audience_set": json.dumps(ROUND)})
    out = c.run({"route": "in_decision", "show_id": "t1"}, verdict("search", 0.9, "results"),
                ctx=DEC_CTX)
    t.check("the window opens with its hint, and asks no app", routes(out), ["view"])
    t.check("the hint", screen_ids(out)[-1], "show-search-hint")
    rec = c.pending("t1")
    t.check("search waits long", rec["data_deadline"] - c.now, 15000)
    call(c, "web_search", "c1", {"query": "rust actors"})
    out = result(c, "c1", HITS, hop={"operation": "web_search"})
    t.check("the result fills the window", [b["key"] for b in last_blocks(out)],
            ["show-search-results"])
    t.check("one item per hit", len(last_blocks(out)[0]["children"]), 3)
    t.check("journal none", [r["fallback"] for r in journal_of(out)], ["none"])

    # A result of another round never reaches the window.
    c = observing(s)
    c.run({"route": "turn", "turn_id": "u1"}, {"messages": [{"type": "text", "text": "q"}]},
          ctx={"audience_set": json.dumps(ROUND)})
    c.run({"route": "in_decision", "show_id": "u1"}, verdict("search", 0.9, "results"),
          ctx=DEC_CTX)
    call(c, "web_search", "f1", {"query": "x"}, ctx={"turn_id": "u1"})
    out = result(c, "f1", HITS, ctx={"audience_set": json.dumps(OTHER)})
    t.check("a foreign round's hits draw nothing", routes(out), [])
    out = result(c, "f1", HITS, ctx={"audience_set": None})
    t.check("a result without a round draws nothing", routes(out), [])

    # Hits before the verdict: the verdict places them at once.
    c = observing(s)
    c.run({"route": "turn", "turn_id": "v1"}, {"messages": [{"type": "text", "text": "q"}]},
          ctx={"audience_set": json.dumps(ROUND)})
    call(c, "web_search", "g1", {"query": "x"})
    result(c, "g1", HITS)
    out = c.run({"route": "in_decision", "show_id": "v1"}, verdict("search", 0.9, "top"),
                ctx=DEC_CTX)
    t.check("hint, then the block, in one output", routes(out), ["view", "view"])
    t.check("the top card", [b["key"] for b in last_blocks(out)], ["show-search-top"])
    # An app may not hold a built-in topic.
    c = observing(s)
    out = c.run({"route": "show_topics", "show_app": "/app2"},
                {"topics": [sample(topic="search", title="Search")]})
    t.check("a built-in name is refused to an app",
            [(e.get("topic"), e.get("reason")) for e in out if e["header"]["route"] == "in_show"],
            [("search", "reserved")])
    t.check("the questions carry the built-ins", sorted(s.questions(s.all_topics(
        s.ram(), c.params))["topic"]["options"]), ["none", "sample", "search", "work"])


def t_work(s, t):
    """Q.3: `work` -- steps, files, tests; never a file's content or a command's output."""
    c = observing(s)
    call(c, "file", "a1", {"op": "read", "path": "src/lib.rs"})
    result(c, "a1", "fn secret_content() {}", hop={"operation": "read"})
    call(c, "bash", "a2", {"command": "cargo test -p x"})
    result(c, "a2", "test result: FAILED. boom output", hop={"operation": "bash", "exit_code": 101})
    call(c, "bash", "a3", {"command": "cargo test -p x"})
    result(c, "a3", "test result: ok. fine output", hop={"operation": "bash", "exit_code": 0})
    sets = s.work_sets(s.ram()["work"], SCREEN, c.params)
    t.check("three steps", [(r["label"], r["state"]) for r in sets["steps"]["rows"]],
            [("read src/lib.rs", "done"), ("test run", "failed"), ("test run", "done")])
    t.check("the touched file", [r["path"] for r in sets["files"]["rows"]], ["src/lib.rs"])
    t.check("tests: the last run decides", sets["tests"]["value"]["verdict"], "passed")
    t.check("a foreign round sees nothing", s.work_sets(s.ram()["work"], ["member:q"],
                                                        c.params), {})
    c.run({"route": "turn", "turn_id": "w1"},
          {"messages": [{"type": "text", "text": "what are you doing"}]},
          ctx={"audience_set": json.dumps(ROUND)})
    out = c.run({"route": "in_decision", "show_id": "w1"},
                verdict("work", 0.9, "steps", "files", 0.6), ctx=DEC_CTX)
    t.check("hint, then steps and files", [b["key"] for b in last_blocks(out)],
            ["show-work-steps", "show-work-files"])
    t.check("three steps on the screen", len(last_blocks(out)[0]["children"]), 3)
    blob = json.dumps(out)
    t.check("no content and no output on the screen",
            ("secret_content" in blob, "boom output" in blob, "fine output" in blob),
            (False, False, False))
    # Review Q M-4: the window of a turn shows the work of ITS conversation, not the
    # newest the screen may see. Two generations, both visible to the screen.
    c = observing(s)
    other = ["agent:g2", "member:p"]
    c.run({"route": "turn", "turn_id": "w5"}, {"messages": [{"type": "text", "text": "x"}]},
          ctx={"audience_set": json.dumps(ROUND)})
    c.run({"route": "turn", "turn_id": "w6"}, {"messages": [{"type": "text", "text": "y"}]},
          ctx={"audience_set": json.dumps(other)})
    call(c, "file", "m1", {"op": "read", "path": "mine.txt"})
    call(c, "file", "m2", {"op": "read", "path": "theirs.txt"},
         ctx={"audience_set": json.dumps(other), "session_id": "s2"})
    rows = s.ram()["work"]
    t.check("the turn's own session", [r["path"] for r in s.work_sets(
        rows, SCREEN, c.params, s.ram()["pending"]["w5"])["files"]["rows"]], ["mine.txt"])
    t.check("the other turn's own session", [r["path"] for r in s.work_sets(
        rows, SCREEN, c.params, s.ram()["pending"]["w6"])["files"]["rows"]], ["theirs.txt"])
    # No observed rows yet: the window waits, a later call fills it.
    c = observing(s)
    c.run({"route": "turn", "turn_id": "w2"}, {"messages": [{"type": "text", "text": "x"}]},
          ctx={"audience_set": json.dumps(ROUND)})
    out = c.run({"route": "in_decision", "show_id": "w2"}, verdict("work", 0.9, "steps"),
                ctx=DEC_CTX)
    t.check("nothing to show yet: the hint alone", routes(out), ["view"])
    out = call(c, "file", "b1", {"op": "list", "path": "docs"})
    t.check("the first observed call fills it", [b["key"] for b in last_blocks(out)],
            ["show-work-steps"])


SOURCE_TOPIC = {
    "topic": "notes", "title": "Notes", "describe": "the notes a resident keeps",
    "glyph": "N", "standard": "list",
    "candidates": [
        {"key": "list", "block": "display-list", "describe": "the notes as a list",
         "set": "notes",
         "source": {"read": "librarian", "hop": {"op": "find"},
                    "body": {"op": "find", "args": {"q": "$request"}}, "rows": "items"},
         "bind": {"title": "=Notes"},
         "children": [{"each": "notes", "block": "display-item", "bind": {"k": "$.name"}}]},
        {"key": "count", "block": "display-card", "describe": "how many notes",
         "set": "count",
         "source": {"read": "colony-view", "hop": {"op": "stats"}, "body": {}, "value": ""},
         "bind": {"title": "count.cells"}}]}


def t_source(s, t):
    """Q.4 against the seam OR-DP.M.1: a declared source is read after the verdict, its
    answer's round is `resident_round` alone."""
    cat = s.catalogue({"catalog": CATALOG})
    t.check("a source in the presenter's own topic", s.check_topic(SOURCE_TOPIC, cat, own=True),
            None)
    t.check("a source in an app's topic is refused",
            s.check_topic(SOURCE_TOPIC, cat) is not None, True)
    ok = {"read": "file-space", "hop": {"op": "list"}, "body": {}, "rows": "entries"}
    t.check("the seam's form", s.check_source(ok), None)
    for label, src in [("an unknown resident", dict(ok, read="vault")),
                       ("an extra key", dict(ok, sql="x")),
                       ("a hop key outside the lane's", dict(ok, hop={"route": "x"})),
                       ("a hop value that is no scalar", dict(ok, hop={"op": {"x": 1}})),
                       ("a path that is no string", dict(ok, rows=3)),
                       ("neither rows nor value", {"read": "file-space", "hop": {}, "body": {}}),
                       ("too big", dict(ok, body={"a": "x" * 5000}))]:
        t.check("source with %s is refused" % label, s.check_source(src) is not None, True)
    body = {"system": {"memory": {"bundle": {"text": json.dumps({"candidates": [{"t": 1}]})}}},
            "messages": [{"text": json.dumps({"items": [{"n": 2}]})}]}
    t.check("a string mid-path is read as JSON",
            s.at_path(body, "system.memory.bundle.text.candidates"), [{"t": 1}])
    t.check("digits index a list", s.at_path(body, "messages.0.text.items"), [{"n": 2}])
    t.check("the empty path is the whole body", s.at_path(body, ""), body)
    t.check("a missing step is nothing", s.at_path(body, "messages.5.text"), None)
    t.check("$request by equality only",
            s.fill({"a": "$request", "b": "x $request", "c": ["$request"]}, "hi"),
            {"a": "hi", "b": "x $request", "c": ["hi"]})

    c = observing(s, builtin_topics=[SOURCE_TOPIC], screen_audience=["member:p"])
    c.run({"route": "turn", "turn_id": "n1"},
          {"messages": [{"type": "text", "text": "my notes on rust"}]},
          ctx={"audience_set": json.dumps(ROUND)})
    out = c.run({"route": "in_decision", "show_id": "n1"},
                verdict("notes", 0.9, "list", "count", 0.6), ctx=DEC_CTX)
    t.check("hint and one read per wanted set", routes(out),
            ["view", "resident_read", "resident_read"])
    rd = [e for e in out if e["header"]["route"] == "resident_read"]
    t.check("the read's hop", rd[0]["header"],
            {"route": "resident_read", "resident": "librarian", "op_id": "n1/notes", "op": "find"})
    t.check("$request became the turn's text", rd[0]["args"], {"q": "my notes on rust"})
    t.check("a body without messages gets them", rd[1]["messages"], [])

    def answer(op_id, resident, body, rnd=json.dumps(["member:p"]), status="answer", **h):
        hop = {"route": "resident_answer", "op_id": op_id, "resident": resident,
               "resident_status": status}
        if rnd is not None:
            hop["resident_round"] = rnd
        hop.update(h)
        return c.run(hop, body)
    out = answer("n1/notes", "librarian",
                 {"items": [{"name": "mine", "audience_set": ["member:p"]},
                            {"name": "theirs", "audience_set": ["member:q"]}, {"name": "plain"}],
                  "audience_set": ["*"]})
    kids = [k["props"]["k"] for k in last_blocks(out)[0]["children"]]
    t.check("a foreign row falls, an unmarked row inherits resident_round", kids,
            ["mine", "plain"])
    out = answer("n1/count", "colony-view", {"ok": True, "cells": 7}, rnd=None)
    t.check("without resident_round the set shows nothing (the body's round never counts)",
            [b["key"] for b in last_blocks(out)] if routes(out) else [], [])
    t.check("a stray answer is ignored", answer("nope/notes", "librarian", {"items": []}), [])
    # A refusal is a set without data: the lead falls to the standard or the clock.
    c = observing(s, builtin_topics=[SOURCE_TOPIC], screen_audience=["member:p"])
    c.run({"route": "turn", "turn_id": "n2"}, {"messages": [{"type": "text", "text": "x"}]})
    c.run({"route": "in_decision", "show_id": "n2"}, verdict("notes", 0.9, "list"), ctx=DEC_CTX)
    # Review Q M-3: the builder's edge always stamps `hop.resident`; without it, or with
    # another resident, the answer is not the one asked for -- and the set stays open.
    t.check("an answer without hop.resident is ignored",
            answer("n2/notes", None, {"items": [{"name": "x"}]}), [])
    t.check("an answer of another resident is ignored",
            answer("n2/notes", "file-space", {"items": [{"name": "x"}]}), [])
    t.check("the set is still open", "notes" in c.pending("n2")["sets"], False)
    out = answer("n2/notes", "librarian", {"items": [{"name": "a"}]}, status="reject")
    t.check("a refused read places nothing", "view" in routes(out), False)
    t.check("the refused set came, empty", c.pending("n2")["sets"].get("notes"), {})
    # A malformed built-in topic is left out, never offered.
    c = observing(s, builtin_topics=[dict(SOURCE_TOPIC, standard="nope")])
    t.check("a malformed built-in topic is not offered",
            "notes" in s.all_topics(s.ram(), c.params), False)


def t_registry(s, t):
    """PE-DP-11, the presenter's half: the decider can be announced to the model registry,
    and the observations the app block declares reach `stage` through its contract."""
    root = os.path.dirname(HERE)
    with open(os.path.join(root, "decide", "config.json"), encoding="utf-8") as f:
        decide = json.load(f)
    with open(os.path.join(root, "config.json"), encoding="utf-8") as f:
        hive = json.load(f)
    with open(os.path.join(root, "template.json"), encoding="utf-8") as f:
        block = json.load(f)["app"]
    need = decide["params"]["requirement"]
    t.check("the decider speaks decisions", decide["params"]["provider"], "decisions")
    # OR-DP-77 (I-3): a push of the registry may move the endpoint only to an origin the
    # set lists; the template lists none and declares the knob.
    t.check("base_url_allow ships empty", decide["params"].get("base_url_allow"), [])
    t.check("base_url_allow is a declared setting",
            (decide["contract"]["settings"].get("base_url_allow") or {}).get("type"), "array")
    t.check("its need is one plain CEL literal",
            bool(need) and not any(ch in need for ch in "'\\\n\r"), True)
    t.check("its need fits the bound", len(need.encode("utf-8")) <= 2048, True)
    entry = s.registry_entry("/m/apps/presenter", need)
    t.check("the announcement entry", entry,
            {"cell_path": "/m/apps/presenter/decide", "start_model": "",
             "requirement": need, "protocol": "decisions"})
    accepts = hive["params"]["contract"]["accepts"]
    lanes = {(a["route"], tuple(a.get("at") or [])) for a in accepts}
    t.check("the push door", ("in_model", ()) in lanes, True)
    t.check("the call lane docks at stage", ("tool", ("./stage",)) in lanes, True)
    t.check("the result lane docks at stage", ("tool_result", ("./stage",)) in lanes, True)
    emits = {e["route"] for e in hive["params"]["contract"]["emits"]}
    t.check("a refused push leaves the rim", "model_refused" in emits, True)
    t.check("the app block observes the calls",
            block.get("observes_tool_calls"),
            {"at": "./stage", "tools": list(s.OBSERVED_TOOLS)})
    t.check("the app block observes the results (string form, every result)",
            block.get("observes_tool_results"), "./stage")
    t.check("the app block reads every resident a source may name",
            block.get("reads_residents"), list(s.RESIDENTS))
    with open(os.path.join(root, "store", "config.json"), encoding="utf-8") as f:
        schema = json.load(f)["params"]["schema"]
    t.check("the store has the work table", sorted(schema.get("work", {})),
            sorted(s.WORK_COLUMNS))
    # The recipe renders what the block declares (read, never written, here).
    recipes = os.path.join(os.path.dirname(root), "builder", "recipes", "config.json")
    if os.path.exists(recipes):
        with open(recipes, encoding="utf-8") as f:
            src = json.load(f)["params"]["script_inline"]
        # The recipe is a script that reads its stdin at module level: only the part
        # before that line is loaded, which defines every renderer.
        head = src.split("\ndoc = json.load(sys.stdin)", 1)[0]
        ns = {"__name__": "recipes_under_test"}
        exec(compile(head, "recipes", "exec"), ns)
        # Until the residents' road is merged (strand M), the recipe does not know the word
        # `reads_residents`; the observation is rendered without it then.
        if "reads_residents" not in ns.get("APP_KEYS", ()):
            block = {k: v for k, v in block.items() if k != "reads_residents"}
        decl = ns["install_app"]({"declaration": block, "ctx": {"member_person": "p"}, "scope": "/m", "app": "presenter",
                                  "template": "presenter", "screen": "display",
                                  "generation": "g1"})
        edges = decl[0]["diff"]["add_edges"]
        into = [(e.get("lane"), e["from"]) for e in edges
                if e["to"] == "./apps/presenter/stage"]
        t.check("calls from both surfaces, results from tools and memory", sorted(into),
                [("tool", "./assistants/g1/talky"), ("tool", "./assistants/g1/talky-chat"),
                 ("tool_result", "./assistants/g1/tools"), ("tool_result", "./memory-hive")])


def t_results(s, t):
    """GH #976 (PE-DP-9): the last result of the daily digest and of the research assistant
    is shown only to the round it stems from. Both residents answer every row with its own
    round; the card binds the newest row as its `value` and is gated by that row's round."""
    t.check("both are residents a source may name",
            [r for r in ("daily-digest", "research-assistant") if r in s.RESIDENTS],
            ["daily-digest", "research-assistant"])
    # A value with its own round is gated like a row; one without stays as it came.
    t.check("a value of another round falls",
            s.filter_set({"audience_set": ["*"], "value": {"a": 1, "audience_set": ["member:q"]}},
                         ["member:p"]), None)
    t.check("a value of the screen's round stands",
            s.filter_set({"audience_set": ["*"], "value": {"a": 1, "audience_set": ["member:p"]}},
                         ["member:p"]), {"value": {"a": 1, "audience_set": ["member:p"]}})
    t.check("a value with an empty round falls",
            s.filter_set({"audience_set": ["*"], "value": {"a": 1, "audience_set": ""}},
                         ["member:p"]), None)
    t.check("a value without a round of its own stays as it came (OR-DP-10)",
            s.filter_set({"audience_set": ["*"], "value": {"a": 1}}, ["member:p"]),
            {"value": {"a": 1}})
    with open(CONFIG, encoding="utf-8") as f:
        shipped = {x["topic"]: x for x in json.load(f)["params"]["builtin_topics"]}
    member = json.dumps(["agent:g1", "member:p"])
    mine = {"id": "d1", "at": 2, "when": "w2", "title": "Daily digest", "lead": "mine",
            "items": ["mine"], "audience_set": ["agent:g1", "member:p"]}
    theirs = {"id": "d0", "at": 3, "when": "w3", "title": "Daily digest", "lead": "theirs",
              "items": ["theirs"], "audience_set": ["agent:g1", "member:p", "member:q"]}
    group = {"id": "d2", "at": 1, "when": "w1", "title": "Daily digest", "lead": "kept",
             "items": ["kept"], "audience_set": None}

    def shown(topic, lead, resident, body, screen, tid):
        c = observing(s, builtin_topics=[shipped[topic]], screen_audience=screen)
        c.run({"route": "turn", "turn_id": tid},
              {"messages": [{"type": "text", "text": "what came"}]}, ctx={"audience_set": member})
        out = c.run({"route": "in_decision", "show_id": tid}, verdict(topic, 0.9, lead),
                    ctx=DEC_CTX)
        reads = [e["header"] for e in out if e["header"]["route"] == "resident_read"]
        out = c.run({"route": "resident_answer", "op_id": "%s/%s" % (tid, lead),
                     "resident": resident, "resident_status": "answer",
                     "resident_round": member}, body)
        return reads, last_blocks(out)

    reads, blocks = shown("digest", "last", "daily-digest",
                          {"ok": True, "op": "last", "digests": [mine, group]},
                          ["member:p"], "g1")
    t.check("the digest is read on its own lane, once per wanted set",
            sorted((r["resident"], r["op_id"], r["op"]) for r in reads),
            [("daily-digest", "g1/last", "last"), ("daily-digest", "g1/recent", "last")])
    t.check("the newest digest of the round shows as the card",
            [(b["component"], b["props"].get("body")) for b in blocks],
            [("display-card", "mine")])
    _, blocks = shown("digest", "last", "daily-digest",
                      {"ok": True, "op": "last", "digests": [mine]}, ["member:q"], "g2")
    t.check("a screen of another round sees no card", blocks, [])
    _, blocks = shown("digest", "last", "daily-digest",
                      {"ok": True, "op": "last", "digests": [theirs, mine]},
                      ["member:p", "member:q"], "g3")
    # R-HP-18: a screen wider than the member's round (a third party shares it) shows the
    # member's window with all of the member's data -- the newest digest the member holds
    t.check("a screen wider than the member's round shows the member's newest digest",
            [b["props"].get("body") for b in blocks], ["theirs"])
    _, blocks = shown("digest", "last", "daily-digest",
                      {"ok": True, "op": "last", "digests": [theirs, mine]}, ["member:p"], "g4")
    t.check("a digest of a wider round that holds the screen shows",
            [b["props"].get("body") for b in blocks], ["theirs"])
    _, blocks = shown("digest", "recent", "daily-digest",
                      {"ok": True, "op": "last",
                       "digests": [dict(theirs, audience_set=["member:q"]), mine, group]},
                      ["member:p"], "g5")
    t.check("the list keeps only the rows of the screen's round, a row without one falls",
            [k["props"]["v"] for b in blocks for k in b.get("children", [])], ["mine"])
    q1 = {"id": "a1", "question_id": "t1", "question": "mine?", "answer": "yes", "at": 2,
          "when": "w2", "audience_set": ["agent:g1", "member:p"]}
    q2 = {"id": "a2", "question_id": "t2", "question": "theirs?", "answer": "no", "at": 3,
          "when": "w3", "audience_set": ["agent:g1", "member:q"]}
    reads, blocks = shown("research", "answers", "research-assistant",
                          {"ok": True, "op": "last", "answers": [q2, q1]}, ["member:p"], "r1")
    t.check("the research answers are read on their own lane",
            sorted(r["resident"] for r in reads), ["research-assistant"] * 2)
    t.check("a research answer shows only to its round",
            [(k["props"]["k"], k["props"]["v"]) for b in blocks for k in b.get("children", [])],
            [("mine?", "yes")])
    _, blocks = shown("research", "latest", "research-assistant",
                      {"ok": True, "op": "last", "answers": [q2, q1]}, ["member:p"], "r2")
    t.check("the newest answer of another round is no card", blocks, [])
    _, blocks = shown("research", "answers", "research-assistant",
                      {"ok": False, "op": "last", "error": {"code": "no_round"}},
                      ["member:p"], "r3")
    t.check("a refused read places nothing", blocks, [])


SEEN = []


def t_data_round(s, t):
    """OR-DP-82: the data question carries the screen's round, canonical (sorted, once
    each), and the empty list where the screen has none."""
    for given, want in ((["member:p", "agent:a", "member:p"], ["agent:a", "member:p"]),
                        ('["peer:x","member:p"]', ["member:p", "peer:x"]),
                        (None, []), ("not a round", [])):
        c = Cell(s, screen_audience=given) if given is not None else Cell(s)
        c.run({"route": "show_topics", "show_app": "/app"}, {"topics": [sample()]})
        c.run({"route": "turn", "turn_id": "r1"}, {"messages": [{"type": "text", "text": "x"}]})
        out = c.run({"route": "in_decision", "show_id": "r1"}, verdict("sample", 0.9, "rows"),
                    ctx=DEC_CTX)
        asks = [e for e in out if e["header"]["route"] == "in_show" and e.get("op") == "data"]
        t.check("data question %r" % (given,), [e.get("screen_audience") for e in asks], [want])


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


def t_followup(s, t):
    """GH #966 N1.3: the review minors of the presenter, each a row."""
    solo = sample(topic="solo", title="Solo", describe="a topic with one block",
                  candidates=[sample()["candidates"][0]])
    other = sample(topic="other", title="Other", describe="another topic")
    # Q N-2: a topic with one candidate has no lead question -- its standard leads by
    # plan, and the journal says `none`, not `invalid` (nothing was invalid).
    c = Cell(s)
    c.run({"route": "show_topics", "show_app": "/app"}, {"topics": [solo]})
    c.run({"route": "turn", "turn_id": "s1"}, {"messages": [{"type": "text", "text": "x"}]})
    out = c.run({"route": "in_decision", "show_id": "s1"},
                {"decision": {"answers": {"topic": {"choice": "solo", "p": {"solo": 0.9}}},
                              "model": "mock-decider"}, "messages": []}, ctx=DEC_CTX)
    t.check("one candidate: window", routes(out), ["view", "in_show"])
    out = c.run({"route": "show_data", "show_app": "/app"},
                {"topic": "solo", "turn_id": "s1",
                 "sets": {"brief": {"audience_set": ["*"], "value": {"title": "Brief"}}}})
    t.check("one candidate: journal none",
            [(r["fallback"], r["lead"]) for r in journal_of(out)], [("none", "")])

    # E2 m-3: `missing` keeps only keys of questions that were asked
    c = Cell(s)
    c.run({"route": "show_topics", "show_app": "/app"}, {"topics": [sample(), other]})
    c.run({"route": "turn", "turn_id": "m1"}, {"messages": [{"type": "text", "text": "x"}]})
    c.run({"route": "in_decision", "show_id": "m1"},
          partial("sample", 0.9, "rows",
                  missing=["other.lead", "free text from a stranger", "sample.nope"]),
          ctx=DEC_CTX)
    out = data(c, "m1", ROWS)
    t.check("missing: only asked keys", [r["missing"] for r in journal_of(out)],
            ['["other.lead"]'])
    t.check("missing_of with the asked keys",
            s.missing_of({"decision": {"missing": ["topic", "zz", "other.lead"]}},
                         {"topic", "other.lead"}), ["other.lead", "topic"])

    # D1-P: a refusal of `./decide` that is no turn's verdict (no show context: a
    # params push the cell refused) reaches the stage and leaves as an error, never
    # a `no_route` in the hive, never a verdict
    c = Cell(s)
    c.run({"route": "show_topics", "show_app": "/app"}, {"topics": [sample()]})
    out = c.run({"route": "in_decision", "finish_reason": "error", "error_code": "invalid_input"},
                {"messages": [{"type": "text", "text": "a params push for another cell"}]},
                ctx={})
    t.check("a refusal without a turn: an error", routes(out), ["error"])
    t.check("named decide_refused",
            [e["header"].get("error_code") for e in out], ["decide_refused"])
    t.check("it says what was refused", "invalid_input" in json.dumps(out), True)
    out = c.run({"route": "in_decision"}, verdict("sample", 0.9, "rows"), ctx={})
    t.check("a verdict without a turn: nothing", out, [])

    # D1-P review: another topic's open turn keeps its window and its deadline when a
    # new window opens
    c = Cell(s)
    c.run({"route": "show_topics", "show_app": "/app"}, {"topics": [sample(), other]})
    opened(c, "a1")
    due = c.pending("a1")["data_deadline"]
    c.tick(1000)
    c.run({"route": "turn", "turn_id": "b1"}, {"messages": [{"type": "text", "text": "x"}]})
    out = c.run({"route": "in_decision", "show_id": "b1"}, verdict("other", 0.9, "rows"),
                ctx=DEC_CTX)
    t.check("the other topic opens", routes(out), ["view", "in_show"])
    t.check("the first topic stays open", c.pending("a1")["state"], "showing")
    t.check("and keeps its deadline", c.pending("a1")["data_deadline"], due)
    c.tick(due - c.now)
    out = c.run({"route": "in_tick"})
    t.check("its deadline still strikes", routes(out), ["withdraw"])
    t.check("for the first topic only",
            [(r["turn_id"], r["fallback"]) for r in journal_of(out)], [("a1", "no_data")])
    t.check("the other window stays", c.pending("b1")["state"], "showing")

    # R fix round 2 (Audience), N review I-1: a set whose own round covers the screen but
    # whose rows are ALL of other rounds, with no `value`, is no set -- an empty block, or
    # a standard that stands at once, would tell the screen that rows of other rounds
    # exist. It is EXACTLY a set that never came: not kept, the lead waits for the clock,
    # and at the deadline the window is withdrawn with `no_data`, in both cases alike.
    hidden = {"rows": {"audience_set": ["X", "Y", "Z", "member"],
                       "rows": [{"name": "a", "audience_set": ["member", "Y"]},
                                {"name": "b", "audience_set": ["X", "Z"]}]}}
    brief = {"brief": {"audience_set": ["*"], "value": {"title": "Brief"}}}
    seen = {}
    # Z2 (N review, side note): a set whose OWN round does not cover the screen is the
    # same -- stored as empty, its lead fell to the standard at once and told the screen
    # that a set of another round exists.
    foreign = {"rows": {"audience_set": ["Y"],
                        "rows": [{"name": "a", "audience_set": ["Y"]}]}}
    for case, first in (("hidden rows", hidden), ("foreign round", foreign),
                        ("never came", {})):
        c = Cell(s, screen_audience=["member", "X"])
        c.run({"route": "show_topics", "show_app": "/app"}, {"topics": [sample()]})
        opened(c, "f1")
        steps = []
        if first:
            out = data(c, "f1", first)
            steps.append(routes(out))
        out = data(c, "f1", brief)
        steps.append(routes(out))
        steps.append(sorted(c.pending("f1")["sets"]))
        c.tick(c.pending("f1")["data_deadline"] - c.now)
        out = c.run({"route": "in_tick"})
        steps.append(routes(out))
        steps.append([(r["turn_id"], r["fallback"]) for r in journal_of(out)])
        seen[case] = steps[-4:]
        if first:
            t.check("%s: no block" % case, steps[0], [])
    t.check("rows all of other rounds: not kept", "rows" in seen["hidden rows"][1], False)
    t.check("rows all of other rounds = a set that never came", seen["hidden rows"],
            seen["never came"])
    t.check("set of another round: not kept", "rows" in seen["foreign round"][1], False)
    t.check("set of another round = a set that never came", seen["foreign round"],
            seen["never came"])
    t.check("filter_set: no visible row, no value",
            s.filter_set(hidden["rows"], ["member", "X"]), None)
    t.check("filter_set: a value keeps the set",
            s.filter_set({"audience_set": ["*"], "value": {"n": 1}, "rows": []}, ["member"]),
            {"value": {"n": 1}, "rows": []})

    # D1-P review: two turns of one topic in the same millisecond -- the later turn is
    # the newer one, whichever verdict comes first
    for order in (("e1", "e2"), ("e2", "e1")):
        c = Cell(s)
        c.run({"route": "show_topics", "show_app": "/app"}, {"topics": [sample()]})
        for tid in ("e1", "e2"):
            c.run({"route": "turn", "turn_id": tid},
                  {"messages": [{"type": "text", "text": "x"}]})
        for tid in order:
            c.run({"route": "in_decision", "show_id": tid}, verdict("sample", 0.9, "rows"),
                  ctx=DEC_CTX)
        t.check("same ms, verdicts %s then %s: the later turn shows" % order,
                (c.pending("e1")["state"], c.pending("e2")["state"]), ("done", "showing"))


def t_foreign_round(s, t):
    """GH #1027: a turn whose round does not cover the screen's asks nothing and opens
    nothing; the journal says `foreign_round`. Measured before: a guest's turn opened the
    member's window with its working hint, and only the data were gated."""
    c = Cell(s, screen_audience=["member:a"])
    c.run({"route": "show_topics", "show_app": "/app"}, {"topics": [sample()]})
    text = {"messages": [{"type": "text", "text": "her sample"}]}
    out = c.run({"route": "turn", "turn_id": "g1"}, text,
                ctx={"audience_set": json.dumps(["agent:x", "person:guest"])})
    t.check("guest: no decider, no view", routes(out), [])
    rows = journal_of(out)
    t.check("guest journal", [r["fallback"] for r in rows], ["foreign_round"])
    t.check("guest journal carries no text", "her sample" in json.dumps(rows), False)
    t.check("guest turn is done", c.pending("g1")["state"], "done")
    out = c.run({"route": "in_decision", "show_id": "g1"}, verdict("sample", 0.9, "rows"),
                ctx=DEC_CTX)
    t.check("a verdict for it opens nothing", routes(out), [])
    out = c.run({"route": "turn", "turn_id": "n1"}, text, ctx={"audience_set": ""})
    t.check("no round: foreign", [r["fallback"] for r in journal_of(out)], ["foreign_round"])
    out = c.run({"route": "turn", "turn_id": "w1"}, text,
                ctx={"audience_set": json.dumps(["member:a", "person:guest"])})
    t.check("a wider round covers the screen", routes(out), ["decide"])
    out = c.run({"route": "turn", "turn_id": "m1"}, text,
                ctx={"audience_set": json.dumps(["agent:x", "member:a"])})
    t.check("member: the decider is asked", "decide" in routes(out), True)
    out = c.run({"route": "in_decision", "show_id": "m1"}, verdict("sample", 0.9, "rows"),
                ctx=DEC_CTX)
    t.check("member: the window opens", routes(out)[:2], ["view", "in_show"])
    # A screen without a round keeps the old way: it shows `*` data only anyway.
    c2 = Cell(s)
    c2.run({"route": "show_topics", "show_app": "/app"}, {"topics": [sample()]})
    out = c2.run({"route": "turn", "turn_id": "o1"}, text,
                 ctx={"audience_set": json.dumps(["person:guest"])})
    t.check("no screen round: asked as before", "decide" in routes(out), True)


def t_star_only(s, t):
    """R-HP-18 (replaces R-HP-9 c): a window shows all of the member's data, also on a
    screen that a third party shares (the turn's round does not cover the screen's, a
    member of the screen's round is in it). The data are gated by the members of the
    screen's round that are in the turn, not by the whole screen round: no `*` filter, no
    standard block for a private lead, the window on the verdict. Another member's rows
    stay out (identity, not company). `window_requires_star_data` on is the R-HP-9 (c)
    way back: a window only on `*` data left after the screen's gate."""
    shared = ["member:a", "member:b"]
    mine = json.dumps(["agent:x", "member:a"])
    text = {"messages": [{"type": "text", "text": "her sample"}]}

    def cell(**params):
        c = Cell(s, screen_audience=shared, **params)
        c.run({"route": "show_topics", "show_app": "/app"}, {"topics": [sample()]})
        return c

    def star():
        return cell(window_requires_star_data=True)

    def turn(c, tid, lead="rows"):
        out = c.run({"route": "turn", "turn_id": tid}, text, ctx={"audience_set": mine})
        t.check("%s: the decider is asked" % tid, "decide" in routes(out), True)
        return c.run({"route": "in_decision", "show_id": tid},
                     verdict("sample", 0.9, lead), ctx=DEC_CTX)

    # R-HP-18, shipped and switched off: the window shows all of the member's data
    for off in (None, False, "false"):
        c = cell() if off is None else cell(window_requires_star_data=off)
        out = turn(c, "a1")
        t.check("all %r: the window opens on the verdict" % (off,), routes(out)[:2],
                ["view", "in_show"])
        asks = [e for e in out if e["header"]["route"] == "in_show" and e.get("op") == "data"]
        t.check("all %r: the app picks among the member's rows" % (off,),
                [e.get("screen_audience") for e in asks], [["member:a"]])
        out = data(c, "a1", {"rows": {"audience_set": ["member:a"], "rows": [
            {"name": "hers"}, {"name": "his", "audience_set": ["member:b"]},
            {"name": "open", "audience_set": ["*"]}]}})
        t.check("all %r: the member's rows are drawn" % (off,), routes(out), ["view"])
        lst = out[0]["content"]["children"][1]["children"][0]
        t.check("all %r: hers and `*`, never another member's" % (off,),
                [r["props"].get("k") for r in lst["children"]], ["hers", "open"])
        t.check("all %r: journal none" % (off,), [r["fallback"] for r in journal_of(out)],
                ["none"])
    # switched on (R-HP-9 c), a private topic: the member's data never cover the third
    # party -> no window
    c = star()
    out = turn(c, "p1")
    t.check("private: the app is asked, no window", routes(out), ["in_show"])
    out = data(c, "p1", {"rows": {"audience_set": ["member:a"], "rows": [{"name": "x"}]}})
    t.check("private: member data draw nothing", routes(out), [])
    c.tick(4000)
    out = c.run({"route": "in_tick"})
    t.check("private: nothing to withdraw", routes(out), [])
    t.check("private: journal no_data", [r["fallback"] for r in journal_of(out)], ["no_data"])
    # on (c): a public topic: the window opens with the `*` data only
    c = star()
    turn(c, "q1")
    out = data(c, "q1", {"rows": {"audience_set": ["*"], "rows": [
        {"name": "open", "audience_set": ["*"]}, {"name": "hers", "audience_set": ["member:a"]}]}})
    t.check("public: the window opens now", routes(out), ["view"])
    t.check("public: it is a touch", "touched" in out[0]["content"]["props"], True)
    lst = out[0]["content"]["children"][1]["children"][0]
    t.check("public: only the `*` row", [r["props"].get("k") for r in lst["children"]], ["open"])
    t.check("public: journal none", [r["fallback"] for r in journal_of(out)], ["none"])
    # on (c): the lead cannot stand on its `*` set: no standard block instead
    c = star()
    turn(c, "v1")
    out = data(c, "v1", {"rows": {"audience_set": ["*"], "value": {"n": 1}},
                         "brief": {"audience_set": ["*"], "value": {"title": "Brief"}}})
    t.check("no standard block instead", routes(out), [])
    c.tick(4000)
    out = c.run({"route": "in_tick"})
    t.check("standard: nothing to withdraw", routes(out), [])
    # on (c): a member's turn never pulls the window of a covered turn of the same topic at
    # its verdict: the window would leave the moment the private turn was judged, the
    # third party's own window cut off and its leaving a side channel (XB-M review I-2).
    covered = json.dumps(["agent:x"] + shared)
    c = star()
    c.run({"route": "turn", "turn_id": "k1"}, text, ctx={"audience_set": covered})
    out = c.run({"route": "in_decision", "show_id": "k1"}, verdict("sample", 0.9, "rows"),
                ctx=DEC_CTX)
    t.check("covered: the window opens on the verdict", routes(out)[:1], ["view"])
    out = turn(c, "k2")
    t.check("covered, then private verdict: no withdraw", routes(out), ["in_show"])
    t.check("covered, then private verdict: the covered turn goes on", journal_of(out), [])
    out = data(c, "k2", {"rows": {"audience_set": ["member:a"], "rows": [{"name": "x"}]}})
    t.check("covered, then private data: nothing", routes(out), [])
    out = data(c, "k1", {"rows": {"audience_set": ["*"], "rows": [{"name": "k1"}]}})
    t.check("covered: its own data still fill its window", routes(out), ["view"])
    # ... and a member's `*` data replace it only when they open the window
    c = star()
    c.run({"route": "turn", "turn_id": "k3"}, text, ctx={"audience_set": covered})
    c.run({"route": "in_decision", "show_id": "k3"}, verdict("sample", 0.9, "rows"),
          ctx=DEC_CTX)
    turn(c, "k4")
    out = data(c, "k4", {"rows": {"audience_set": ["*"], "rows": [{"name": "open"}]}})
    t.check("covered, then `*` data: the window is replaced", routes(out), ["view"])
    t.check("covered, then `*` data: the covered turn leaves now",
            [r["fallback"] for r in journal_of(out)], ["no_data", "none"])
    # a lead the member's data cannot fill: the standard block stands in, as anywhere
    c = cell()
    turn(c, "b1")
    out = data(c, "b1", {"rows": {"audience_set": ["member:a"], "value": {"n": 1}},
                         "brief": {"audience_set": ["member:a"], "value": {"title": "Brief"}}})
    blocks = out[0]["content"]["children"][1]["children"]
    t.check("all: standard instead", [b["key"] for b in blocks], ["show-sample-brief"])
    c = cell()
    turn(c, "b2")
    c.tick(4000)
    out = c.run({"route": "in_tick"})
    t.check("all: no data withdraws", routes(out), ["withdraw"])
    # a turn of a foreign round (no member of the screen's round in it) opens nothing,
    # whatever the switch says (GH #1027)
    for sw in (True, False):
        c = cell(window_requires_star_data=sw)
        out = c.run({"route": "turn", "turn_id": "g1"}, text,
                    ctx={"audience_set": json.dumps(["agent:x", "person:guest"])})
        t.check("foreign (switch %s): asks nothing" % sw, routes(out), [])
        t.check("foreign (switch %s): journal" % sw,
                [r["fallback"] for r in journal_of(out)], ["foreign_round"])


def t_clock(s, t):
    """GH #1047: an order that fired is gone at the timer -- it is never removed, and the
    clock's `schedule_not_found` for a removed order is no fault. Measured downstream: 6
    dead letters `error` `clock_refused` `schedule_not_found` from `stage` in 5 of 109
    archived gate runs, more often under host load, when the strike of the standing
    order was still in the queue while `stage` handled the next turn."""
    def clock(out):
        return [(e["op"], e["schedule_id"]) for e in out if e["header"]["route"] == "clock"]

    c = Cell(s)
    c.run({"route": "show_topics", "show_app": "/app", "show_at": "./show"},
          {"topics": [sample()]})
    out = c.run({"route": "turn", "turn_id": "c1"},
                {"messages": [{"type": "text", "text": "x"}]})
    first = clock(out)
    t.check("the first turn orders one deadline", [op for op, _ in first], ["add"])
    # The deadline passes: the timer fires and the order is `completed` there; its strike
    # is still in the queue when the next turn comes.
    c.tick(s.DEFAULTS["budget_ms"] + 1000)
    out = c.run({"route": "turn", "turn_id": "c2"},
                {"messages": [{"type": "text", "text": "y"}]})
    ops = clock(out)
    t.check("no remove for an order whose moment has passed",
            [sid for op, sid in ops if op == "remove"], [])
    t.check("the next deadline is ordered", [op for op, _ in ops], ["add"])
    # The late strike of the fired order closes the expired turn and leaves the new
    # order standing.
    out = c.run({"route": "in_tick", "schedule_id": first[0][1]})
    t.check("the late strike times the first turn out",
            [r["fallback"] for r in journal_of(out)], ["timeout"])
    t.check("the late strike does not remove the order that struck",
            [sid for op, sid in clock(out) if op == "remove" and sid == first[0][1]], [])
    # A remove that met no order (it fired on the way): the order is gone, as wanted.
    out = c.run({"route": "in_tick_error", "msg_type": "timer_op_error",
                 "error_code": "schedule_not_found", "schedule_id": first[0][1]})
    t.check("schedule_not_found is no fault", out, [])
    # Every other refusal of the clock still leaves out loud.
    out = c.run({"route": "in_tick_error", "msg_type": "timer_op_error",
                 "error_code": "at_in_past", "schedule_id": ops[-1][1]})
    t.check("another refusal leaves as error",
            [(e["header"]["route"], e["header"].get("error_code")) for e in out],
            [("error", "clock_refused")])
    # An order whose moment has NOT passed is still removed when the deadline moves.
    c2 = Cell(s)
    c2.run({"route": "show_topics", "show_app": "/app", "show_at": "./show"},
           {"topics": [sample()]})
    first = clock(c2.run({"route": "turn", "turn_id": "d1"},
                         {"messages": [{"type": "text", "text": "x"}]}))
    c2.tick(10)
    out = c2.run({"route": "in_decision", "show_id": "d1"}, verdict("sample", 0.9, "rows"),
                 ctx=DEC_CTX)
    t.check("a standing order that moves is removed",
            [sid for op, sid in clock(out) if op == "remove"], [first[0][1]])


TABLES = [("MANIFEST", t_manifest), ("QUESTIONS", t_questions), ("THRESHOLD", t_threshold),
          ("BINDING", t_binding), ("AUDIENCE", t_audience), ("TURNS", t_turns),
          ("PARTIAL", t_partial),
          ("LIFECYCLE", t_lifecycle),
          ("OBSERVE", t_observe), ("SEARCH", t_search), ("WORK", t_work),
          ("SOURCE", t_source), ("REGISTRY", t_registry), ("DATAROUND", t_data_round),
          ("FOLLOWUP", t_followup), ("RESULTS", t_results),
          ("CONTRACT", t_contract), ("FOREIGN", t_foreign_round),
          ("STARONLY", t_star_only), ("CLOCK", t_clock)]


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
