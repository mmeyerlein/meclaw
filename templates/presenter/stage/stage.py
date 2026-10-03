"""The `stage` cell of the presenter: every turn asks the decider once, a sure verdict opens a window.

THIS FILE IS THE SOURCE. `config.json` beside it carries a byte-identical copy in
`params.script_inline`; `check_stage.py --sync` writes it and `check_stage.py` refuses a
tree in which the two have drifted apart.

# The rule (one sentence)

When the decider is sure that something on the screen helps with what was just said,
something is shown -- and only then. No trigger words: every turn with text goes to
`./decide` as ONE call (the topic choice over every known topic plus `none`, and per topic
a `lead` and an `also` choice over its candidates), and the state of that call is the
turn's text and nothing else -- no fetched data, no memory.

A sure verdict (`topic` is not `none` and its confidence reaches the topic's threshold)
leaves in ONE output: the window `show-<topic>` with a working hint, AND the data request
`in_show {op: "data"}` to the app that owns the topic. Blocks then appear as their data
arrives, each one fully bound and checked against the catalogue copy in `params.catalog`.
Unsure, `none`, an error or the deadline: nothing on the screen, the answer stays text, and
one journal row says why. A verdict that comes after the deadline changes nothing.

# State

The cell is `resident` (one child, strict FIFO): the order of events is the truth. RAM is a
cache of the store beside it and never its truth -- every change is written through as one
store bundle in the same output, and a cold child (first message, or a child that was
killed) reads the `shows` and `pending` tables back before it handles anything; the event
that woke it waits in RAM until the read is back.

# Pure functions

Everything that decides -- the questions, the threshold, the binding, the audience gate,
the fallback -- is a pure function of its arguments. `check_stage.py` drives them as
tables, without a colony.
"""

import json
import sys
import time
import uuid

# ---------------------------------------------------------------------------
# Names

T_SHOWS = "shows"
T_PENDING = "pending"
T_JOURNAL = "journal"
SHOWS_COLUMNS = ["topic", "owner_app", "show_at", "manifest", "at"]
PENDING_COLUMNS = ["turn_id", "value", "at"]

PHASE_BOOT = "boot"
PHASE_WRITE = "write"

FALLBACKS = ("none", "unsure", "no_topic", "timeout", "error", "no_selector",
             "invalid", "no_data")

# The decider's call carries 1 + 2*N questions; the decider takes at most 64 (E.2).
MAX_TOPICS = 31
MAX_CANDIDATES = 5
NONE = "none"
NONE_TOPIC = "nothing on the screen helps with this request"
NONE_ALSO = "no second block"

DEFAULTS = {
    "budget_ms": 1000,
    "threshold": 0.7,
    "data_wait_ms": 4000,
    "also_threshold": 0.5,
    "work_hint": "Working on it…",
}

PANE = "display-pane"
STACK = "display-stack"
STATUS = "display-status"
TILE = "display-tile"
LAYER_CANVAS = "canvas"
CONTEXT = "conversation"
RELEVANCE = "0.8"
TILE_CHARS = 24

# A done turn is kept this long, so a late verdict can still be told from a stray one.
KEEP_DONE_MS = 60000

DEADLINE = "deadline"
DUE_NAMESPACE = uuid.UUID("6f1c2a52-9f4e-4c1b-8d55-3c0b6e5a9d01")

TYPES = ("text", "int", "number", "boolean", "html")


# ---------------------------------------------------------------------------
# Wire helpers


def now_ms():
    held = globals().get("_TEST_NOW")
    if isinstance(held, int):
        return held
    return int(time.time() * 1000)


def emission(route, body, **header):
    head = {"route": route}
    head.update(header)
    out = {"header": head}
    out.update(body)
    if "messages" not in out:
        out["messages"] = []
    return out


def tool_call(args, tid):
    return {"origin": "assistant", "type": "tool_call", "id": tid,
            "text": json.dumps(args, sort_keys=True)}


def error(code, detail):
    return emission("error", {"messages": [{"origin": "assistant", "type": "text",
                                            "text": detail}], "detail": detail},
                    error_code=code, detail=detail)


def bundle(phase, legs):
    return emission("store", {"messages": legs}, phase=phase)


def iso_z(ms):
    ms = int(ms)
    t = time.gmtime(ms // 1000)
    return "%04d-%02d-%02dT%02d:%02d:%02d.%03dZ" % (
        t.tm_year, t.tm_mon, t.tm_mday, t.tm_hour, t.tm_min, t.tm_sec, ms % 1000)


def knob(params, key):
    v = params.get(key)
    d = DEFAULTS.get(key)
    if isinstance(d, float):
        try:
            f = float(v)
        except (TypeError, ValueError):
            return d
        return f if 0 < f <= 1 else d
    if isinstance(d, int):
        try:
            i = int(v)
        except (TypeError, ValueError):
            return d
        return i if i >= 1 else d
    return v if isinstance(v, str) and v else d


# ---------------------------------------------------------------------------
# Audience (R-DP-b, OR-DP-4): the substrate's `covers`, fail-closed


def as_round(value):
    """A round as a list of strings, or None where it is no round at all.

    Accepts a list or its canonical JSON text. Anything else -- None, a number, text that
    is no JSON list, a list with a non-string in it -- is None, and None covers nothing.
    """
    if isinstance(value, str):
        try:
            value = json.loads(value)
        except ValueError:
            return None
    if not isinstance(value, list):
        return None
    if not all(isinstance(x, str) for x in value):
        return None
    return value


def covers(audience_set, screen_round):
    """True exactly when `audience_set` may be seen by the screen's round.

    The rule of the substrate predicate `Covers` (store/query/mod.rs): the set is a
    non-empty list that holds `*` or every member of the round. A screen with no round
    (empty or missing `screen_audience`) sees `*` data only. Missing, empty or malformed
    sets are false -- never an error, never a pass.
    """
    have = as_round(audience_set)
    if not have:
        return False
    if "*" in have:
        return True
    want = as_round(screen_round) or []
    if not want:
        return False
    return all(m in have for m in want)


def filter_set(data_set, screen_round):
    """One data set after the audience gate, or None where nothing of it may be shown.

    The set's own `audience_set` must cover the round, or the whole set is gone, marked
    rows included (fail-closed: a set without a round shows nothing). A row with its OWN
    `audience_set` is gated by it alone; a row without one inherits the set's round,
    because the set's stamp is the app's statement about all of its rows (OR-DP-56: the
    weather app sends public sets `["*"]` with unmarked rows). `value` stays as it came,
    because an app derives a value from rows under the intersection of their rounds
    (OR-DP-10).
    """
    if not isinstance(data_set, dict):
        return None
    if not covers(data_set.get("audience_set"), screen_round):
        return None
    out = {}
    if isinstance(data_set.get("value"), dict):
        out["value"] = data_set["value"]
    rows = data_set.get("rows")
    if isinstance(rows, list):
        out["rows"] = [r for r in rows if isinstance(r, dict)
                       and ("audience_set" not in r
                            or covers(r.get("audience_set"), screen_round))]
    return out


# ---------------------------------------------------------------------------
# The catalogue copy


def catalogue(params):
    """`params.catalog` as `{name: entry}`. A list of entries or a map both read."""
    raw = params.get("catalog")
    if isinstance(raw, dict) and isinstance(raw.get("components"), list):
        raw = raw["components"]
    out = {}
    if isinstance(raw, list):
        for entry in raw:
            if isinstance(entry, dict) and isinstance(entry.get("name"), str):
                out[entry["name"]] = entry
    elif isinstance(raw, dict):
        for name, entry in raw.items():
            if isinstance(entry, dict):
                e = dict(entry)
                e.setdefault("name", name)
                out[name] = e
    return out


def props_of(entry):
    props = entry.get("props") if isinstance(entry, dict) else None
    out = {}
    if isinstance(props, dict):
        for name, spec in props.items():
            if isinstance(spec, dict):
                out[name] = spec
            elif isinstance(spec, str):
                out[name] = {"type": spec}
    return out


def slot_allows(entry, child):
    slots = entry.get("slots") if isinstance(entry, dict) else None
    if slots == "any":
        return True
    if slots in (None, "none"):
        return False
    return isinstance(slots, list) and child in slots


def is_block(entry):
    """A block of the copy. `scripts/display_sync.py` writes ONLY the blocks, each as
    `name, props, slots, describe` -- no `block` key -- so presence in the copy is the
    mark; an entry that says `block: false` (a hand-written copy) is still refused."""
    return isinstance(entry, dict) and entry.get("block", True) is True


# ---------------------------------------------------------------------------
# The manifest (README § 2): checked once, when an app says its topics


def word_ok(text):
    return isinstance(text, str) and 0 < len(text) <= 64 and \
        all(c in "abcdefghijklmnopqrstuvwxyz0123456789_-" for c in text)


def children_of(candidate):
    """The `each` specs of a candidate: the list under `children` (OR-DP-53), else
    `[]`; anything that is not a list is None, which the manifest check refuses."""
    kids = candidate.get("children")
    if kids is None:
        return []
    return kids if isinstance(kids, list) else None


def scalar_bind(bind):
    if not isinstance(bind, dict):
        return {}
    return dict(bind)


def check_bind_spec(bind, entry, row_scope, own_set):
    """None when every bound prop is a catalogue prop of the block and every source is of
    the closed vocabulary; else the reason. `html` props only from a set's `value`
    (OR-DP-10): never from a row, never as a literal. A `<set>.<field>` source names the
    candidate's OWN set: a foreign set may be one nobody asked for (an `on_choice` set of
    a candidate not chosen), and an optional prop would then drop silently (review P,
    minor 1)."""
    props = props_of(entry)
    for prop, src in scalar_bind(bind).items():
        if prop not in props:
            return "prop %s is not in the catalogue of %s" % (prop, entry.get("name"))
        if not isinstance(src, str) or not src:
            return "prop %s has no source" % prop
        raw = props[prop].get("type") == "html"
        if src.startswith("="):
            if raw:
                return "raw prop %s cannot be a literal" % prop
            continue
        if src.startswith("$."):
            if not row_scope:
                return "prop %s reads a row outside of each" % prop
            if raw:
                return "raw prop %s cannot come from a row" % prop
            if len(src) <= 2:
                return "prop %s names no row field" % prop
            continue
        head, dot, field = src.partition(".")
        if not dot or not head or not field:
            return "prop %s source %r is not <set>.<field>" % (prop, src)
        if head != own_set:
            return "prop %s reads set %s, not its own set %s" % (prop, head, own_set)
    return None


def check_candidate(cand, cat):
    if not isinstance(cand, dict):
        return "a candidate is not an object"
    if not word_ok(cand.get("key")) or "." in cand.get("key", ""):
        return "candidate key %r" % cand.get("key")
    if not isinstance(cand.get("describe"), str) or not cand["describe"].strip():
        return "candidate %s has no describe" % cand.get("key")
    block = cand.get("block")
    entry = cat.get(block) if isinstance(block, str) else None
    if not is_block(entry):
        return "candidate %s: %r is no block of the catalogue" % (cand.get("key"), block)
    if not isinstance(cand.get("set"), str) or not cand["set"]:
        return "candidate %s names no set" % cand.get("key")
    if "on_choice" in cand and not isinstance(cand["on_choice"], (str, bool, list)):
        return "candidate %s: on_choice" % cand.get("key")
    why = check_bind_spec(cand.get("bind") or {}, entry, False, cand["set"])
    if why:
        return "candidate %s: %s" % (cand.get("key"), why)
    kids = children_of(cand)
    if kids is None:
        return "candidate %s: children" % cand.get("key")
    for kid in kids:
        if not isinstance(kid, dict) or not isinstance(kid.get("each"), str):
            return "candidate %s: a child is not {each, block, bind}" % cand.get("key")
        if kid["each"] != cand["set"]:
            return "candidate %s: each runs over %s, not its own set" % (
                cand.get("key"), kid["each"])
        kentry = cat.get(kid.get("block")) if isinstance(kid.get("block"), str) else None
        if kentry is None:
            return "candidate %s: child %r is not in the catalogue" % (
                cand.get("key"), kid.get("block"))
        if not slot_allows(entry, kid["block"]):
            return "candidate %s: %s takes no %s" % (cand.get("key"), block, kid["block"])
        why = check_bind_spec(kid.get("bind") or {}, kentry, True, cand["set"])
        if why:
            return "candidate %s: %s" % (cand.get("key"), why)
    return None


def check_topic(topic, cat):
    """None when the manifest entry of one topic is well formed; else the reason."""
    if not isinstance(topic, dict):
        return "a topic is not an object"
    name = topic.get("topic")
    if not word_ok(name) or "." in name:
        return "topic name %r" % name
    if not isinstance(topic.get("describe"), str) or not topic["describe"].strip():
        return "topic %s has no describe" % name
    if not isinstance(topic.get("title"), str) or not topic["title"].strip():
        return "topic %s has no title" % name
    if "threshold" in topic:
        t = topic["threshold"]
        if isinstance(t, bool) or not isinstance(t, (int, float)) or not 0 < t <= 1:
            return "topic %s: threshold must lie in (0, 1]" % name
    if "data_wait_ms" in topic:
        w = topic["data_wait_ms"]
        if isinstance(w, bool) or not isinstance(w, int) or w < 1:
            return "topic %s: data_wait_ms" % name
    cands = topic.get("candidates")
    if not isinstance(cands, list) or not cands:
        return "topic %s has no candidates" % name
    if len(cands) > MAX_CANDIDATES:
        return "topic %s has more than %d candidates" % (name, MAX_CANDIDATES)
    keys = []
    for cand in cands:
        why = check_candidate(cand, cat)
        if why:
            return "topic %s: %s" % (name, why)
        if cand["key"] == NONE or cand["key"] in keys:
            return "topic %s: candidate key %s twice or reserved" % (name, cand["key"])
        keys.append(cand["key"])
    if topic.get("standard") not in keys:
        return "topic %s: the standard is not one of its candidates" % name
    return None


def accept_topics(known, owner, topics, cat):
    """The `shows` table after one app said its topics.

    `known` is `{topic: {owner, manifest, at}}`. The app's own earlier rows are replaced
    as a whole; a topic another app already holds stays with that app (`duplicate`); a
    malformed one gets no row; topics past the call's ceiling, alphabetically last,
    `overflow`. Returns `(new_known, refusals)`; a refusal is `(topic, reason)`.
    """
    out = {t: dict(v) for t, v in known.items() if v.get("owner") != owner}
    refused = []
    fresh = []
    seen = set()
    for topic in topics if isinstance(topics, list) else []:
        name = topic.get("topic") if isinstance(topic, dict) else None
        why = check_topic(topic, cat)
        if why:
            refused.append((str(name or "?"), "invalid: " + why))
            continue
        if name in seen:
            refused.append((name, "duplicate"))
            continue
        seen.add(name)
        if name in out:
            refused.append((name, "duplicate"))
            continue
        fresh.append(topic)
    room = MAX_TOPICS - len(out)
    fresh.sort(key=lambda t: t["topic"])
    for i, topic in enumerate(fresh):
        if i >= room:
            refused.append((topic["topic"], "overflow"))
            continue
        out[topic["topic"]] = {"owner": owner, "manifest": topic, "at": now_ms()}
    return out, refused


# ---------------------------------------------------------------------------
# The one call (R-29-6, OR-DP-3)


def questions(known):
    """`topic` over every topic plus `none`; per topic `<t>.lead` and `<t>.also`."""
    topic_opts = {}
    qs = {}
    for name in sorted(known):
        m = known[name]["manifest"]
        topic_opts[name] = "%s: %s" % (m["title"], m["describe"])
        cands = {c["key"]: c["describe"] for c in m["candidates"]}
        qs[name + ".lead"] = {
            "kind": "choice",
            "instructions": "Which block shows best what the request asks about %s?" % m["title"],
            "options": dict(cands)}
        also = dict(cands)
        also[NONE] = NONE_ALSO
        qs[name + ".also"] = {
            "kind": "choice",
            "instructions": "Which second block helps beside the first one, if any?",
            "options": also}
    topic_opts[NONE] = NONE_TOPIC
    qs["topic"] = {"kind": "choice",
                   "instructions": "Which of these would help on a screen beside the answer to this request?",
                   "options": topic_opts}
    return qs


def decide_call(text, known, show_id):
    """The emission to `./decide`: the turn's text as the state, nothing else (R-DP-a)."""
    return emission("decide", {"messages": [],
                               "decide": {"state": text, "questions": questions(known)}},
                    show_id=show_id)


def answer_of(answers, key):
    """`(choice, p)` of one choice answer; `(None, 0.0)` where it is missing or malformed.

    `p` is read the forgiving way, because the decider's answer is the one thing in here
    that comes from outside: a map of choice -> probability, a bare number, or a
    `confidence` beside the choice.
    """
    a = answers.get(key) if isinstance(answers, dict) else None
    if not isinstance(a, dict):
        return None, 0.0
    choice = a.get("choice")
    if not isinstance(choice, str):
        return None, 0.0
    p = a.get("p")
    if isinstance(p, dict):
        p = p.get(choice)
    if p is None:
        p = a.get("confidence")
    if isinstance(p, bool) or not isinstance(p, (int, float)):
        return choice, 0.0
    return choice, float(p)


def read_verdict(body, known):
    """The decision, as `{topic, p, lead, lead_p, also, also_p, model}`.

    A topic the decider made up, or a candidate key the topic does not carry, reads as no
    choice: the decider chooses among what it was offered or it chose nothing.
    """
    d = body.get("decision") if isinstance(body.get("decision"), dict) else body
    answers = d.get("answers") if isinstance(d, dict) else None
    model = str((d or {}).get("model") or body.get("model") or "")
    topic, p = answer_of(answers, "topic")
    v = {"topic": topic, "p": p, "lead": None, "lead_p": 0.0, "also": None,
         "also_p": 0.0, "model": model}
    if topic == NONE or topic is None:
        v["topic"] = NONE if topic == NONE else None
        return v
    if topic not in known:
        v["topic"] = None
        return v
    keys = [c["key"] for c in known[topic]["manifest"]["candidates"]]
    lead, lp = answer_of(answers, topic + ".lead")
    also, ap = answer_of(answers, topic + ".also")
    v["lead"] = lead if lead in keys else None
    v["lead_p"] = lp
    v["also"] = also if also in keys else None
    v["also_p"] = ap
    return v


def threshold_of(manifest, params):
    t = manifest.get("threshold")
    if isinstance(t, (int, float)) and not isinstance(t, bool) and 0 < t <= 1:
        return float(t)
    return knob(params, "threshold")


def judge(v, known, params):
    """`None` for a sure verdict, else the journal's fallback word."""
    if v["topic"] == NONE:
        return "no_topic"
    if v["topic"] is None:
        return "unsure"
    if v["p"] < threshold_of(known[v["topic"]]["manifest"], params):
        return "unsure"
    return None


def chosen(v, manifest, params):
    """The keys to place, in order: the lead (or, without one, the standard), then the
    also where it is a different block and sure enough."""
    first = v["lead"] or manifest["standard"]
    out = [first]
    if v["also"] and v["also"] != first and v["also_p"] >= knob(params, "also_threshold"):
        out.append(v["also"])
    return out


def wanted_sets(manifest, keys):
    """Every set without `on_choice`, plus the sets of the chosen candidates (R-29-2)."""
    want = []
    cands = {c["key"]: c for c in manifest["candidates"]}
    for c in manifest["candidates"]:
        if not c.get("on_choice") and c["set"] not in want:
            want.append(c["set"])
    for k in keys + [manifest["standard"]]:
        c = cands.get(k)
        if c and c["set"] not in want:
            want.append(c["set"])
    return want


# ---------------------------------------------------------------------------
# Binding (README § 2 bind) and the catalogue check


def lookup(obj, path):
    cur = obj
    for part in path.split("."):
        if not isinstance(cur, dict) or part not in cur:
            return None
        cur = cur[part]
    return cur


def coerce(value, ty):
    """A bound value in the catalogue's type, or None where it is not one."""
    if value is None:
        return None
    if ty in ("text", "html"):
        if isinstance(value, (dict, list)):
            return None
        if isinstance(value, bool):
            return "true" if value else "false"
        if isinstance(value, float) and value.is_integer():
            value = int(value)
        return str(value)
    if ty == "int":
        if isinstance(value, bool):
            return None
        if isinstance(value, int):
            return value
        if isinstance(value, float) and value.is_integer():
            return int(value)
        if isinstance(value, str):
            try:
                return int(value.strip())
            except ValueError:
                return None
        return None
    if ty == "number":
        if isinstance(value, bool):
            return None
        if isinstance(value, (int, float)):
            return value
        if isinstance(value, str):
            try:
                return float(value.strip())
            except ValueError:
                return None
        return None
    if ty == "boolean":
        if isinstance(value, bool):
            return value
        if value in ("true", "1", 1):
            return True
        if value in ("false", "0", 0, ""):
            return False
        return None
    return None


def bind_props(bind, entry, sets, row):
    """The props of one block, or None where a required prop stays empty or a value does
    not fit its type."""
    props = props_of(entry)
    out = {}
    for prop, src in scalar_bind(bind).items():
        if src.startswith("="):
            raw = src[1:]
        elif src.startswith("$."):
            raw = lookup(row, src[2:]) if isinstance(row, dict) else None
        else:
            head, _, field = src.partition(".")
            data = sets.get(head)
            raw = lookup(data.get("value"), field) if isinstance(data, dict) else None
        value = coerce(raw, props[prop].get("type", "text"))
        if value is None:
            if raw is not None:
                return None
            continue
        out[prop] = value
    for prop, spec in props.items():
        if spec.get("required") and prop not in out:
            return None
    return out


def place(topic, cand, sets, cat):
    """One fully bound block, or None where it cannot be placed (P.6 (ii)-(iv)).

    `sets` holds the sets that arrived, already through the audience gate. A candidate
    whose own set is missing, whose `each` has no row left, or whose tree does not stand
    against the catalogue is not placeable -- and is never placed half.
    """
    data = sets.get(cand["set"])
    if not isinstance(data, dict):
        return None
    entry = cat.get(cand["block"])
    if not is_block(entry):
        return None
    if not data.get("value") and not data.get("rows"):
        return None
    props = bind_props(cand.get("bind") or {}, entry, sets, None)
    if props is None:
        return None
    key = "show-%s-%s" % (topic, cand["key"])
    node = {"component": cand["block"], "props": props, "key": key}
    kids = []
    for spec in children_of(cand) or []:
        src = sets.get(spec["each"])
        rows = src.get("rows") if isinstance(src, dict) else None
        if not rows:
            return None
        kentry = cat.get(spec["block"])
        for i, row in enumerate(rows):
            kp = bind_props(spec.get("bind") or {}, kentry, sets, row)
            if kp is None:
                return None
            kids.append({"component": spec["block"], "props": kp,
                         "key": "%s-%d" % (key, len(kids))})
    if children_of(cand) and not kids:
        return None
    if kids:
        node["children"] = kids
    return node


# ---------------------------------------------------------------------------
# The window


def shorten(text):
    text = " ".join(str(text or "").split())
    return text if len(text) <= TILE_CHARS else text[:TILE_CHARS - 1] + "…"


def window(topic, manifest, turn_id, blocks, hint, touched):
    """The view `show-<topic>`: a pane on the canvas, its tile, and either the working
    hint or the blocks placed so far. `touched` is said once, by the write that opens
    the window; a write that only adds a block leaves it out, and a key left out stands
    (§ 4.8): a block is no new touch."""
    vid = "show-" + topic
    props = {"pane_id": vid, "kicker": manifest["title"], "title": "",
             "context": CONTEXT, "relevance": RELEVANCE, "topic": "show:" + topic,
             "layer": LAYER_CANVAS}
    if turn_id:
        props["turn_id"] = turn_id
    if touched:
        props["touched"] = str(int(touched))
    tile = {"component": TILE, "key": "tile",
            "props": {"glyph": str(manifest.get("glyph") or ""),
                      "line": shorten(manifest["title"]), "value": ""}}
    if blocks:
        inner = list(blocks)
    else:
        inner = [{"component": STATUS, "key": vid + "-hint",
                  "props": {"kind": "working", "text": hint}}]
    stack = {"component": STACK, "props": {"gap": "s"}, "key": vid + "-stack",
             "children": inner}
    return {"component": PANE, "props": props, "key": "show." + topic,
            "children": [tile, stack]}


def a_view(topic, tree):
    return emission("view", {"messages": [], "view_id": "show-" + topic,
                             "kind": "component", "content": tree, "components": []})


def withdraw(topic):
    return emission("withdraw", {"messages": [], "view_id": "show-" + topic})


# ---------------------------------------------------------------------------
# One turn's record, and the decisions over it (pure)


def fresh_turn(turn_id, arrived, budget, audience_set):
    return {"turn_id": turn_id, "arrived": arrived, "deadline": arrived + budget,
            "state": "asking", "topic": None, "p": 0.0, "lead": None, "also": None,
            "keys": [], "model": "", "t_verdict": None, "t_window": None,
            "t_content": None, "data_deadline": None, "sets": {}, "placed": [],
            "first": "open", "fallback": None, "late": False,
            "audience_set": audience_set, "owner": None, "done_at": None}


def resolve(rec, manifest, cat):
    """The blocks a turn can place now, in order, and the fallback word they mean.

    Order is the lead, then the also. The first slot resolves when the lead is placeable
    (`none`) or, its set being there and the lead not placeable, when the standard is
    (`invalid`). An also waits for the first slot; a block once placed stays.
    Returns `(new_blocks, fallback_or_None)`.
    """
    cands = {c["key"]: c for c in manifest["candidates"]}
    topic = rec["topic"]
    new = []
    fallback = None
    if rec["first"] == "open":
        lead = cands.get(rec["keys"][0])
        node = place(topic, lead, rec["sets"], cat) if lead else None
        if node is not None:
            rec["first"] = "lead"
            fallback = "none" if rec["lead"] else "invalid"
            new.append((lead["key"], node))
        elif lead and lead["set"] in rec["sets"]:
            std = cands.get(manifest["standard"])
            if std is not None and std["key"] != lead["key"]:
                snode = place(topic, std, rec["sets"], cat)
                if snode is not None:
                    rec["first"] = "standard"
                    fallback = "invalid"
                    new.append((std["key"], snode))
                elif std["set"] in rec["sets"]:
                    rec["first"] = "failed"
            else:
                rec["first"] = "failed"
    if rec["first"] != "open":
        for k in rec["keys"][1:]:
            if k in rec["placed"] or any(k == n for n, _ in new):
                continue
            c = cands.get(k)
            node = place(topic, c, rec["sets"], cat) if c else None
            if node is not None:
                new.append((k, node))
                if fallback is None and not rec["placed"]:
                    fallback = "invalid"
    return new, fallback


def journal_row(rec, at):
    def ms(v):
        return None if v is None else int(v)
    return {"turn_id": rec["turn_id"], "topic": rec["topic"] or "",
            "p": float(rec["p"] or 0.0), "lead": rec["lead"] or "",
            "also": rec["also"] or "", "fallback": rec["fallback"] or "",
            "late": 1 if rec["late"] else 0, "t_verdict_ms": ms(rec["t_verdict"]),
            "t_window_ms": ms(rec["t_window"]), "t_content_ms": ms(rec["t_content"]),
            "model": rec["model"] or "", "at": int(at),
            "audience_set": json.dumps(rec["audience_set"]) if rec["audience_set"] is not None else ""}


# ---------------------------------------------------------------------------
# RAM, written through


RAM_V = 1


def ram():
    held = globals().get("_RAM")
    if isinstance(held, dict) and held.get("v") == RAM_V:
        return held
    held = {"v": RAM_V, "phase": "cold", "queue": [], "shows": {}, "pending": {},
            "due": None}
    globals()["_RAM"] = held
    return held


def w_pending(rec):
    tid = rec["turn_id"]
    return [tool_call({"operation": "delete", "table": T_PENDING,
                       "where": {"turn_id": tid}}, "p-del-" + tid),
            tool_call({"operation": "insert", "table": T_PENDING,
                       "row": {"turn_id": tid, "value": rec, "at": now_ms()}},
                      "p-ins-" + tid)]


def w_pending_gone(tid):
    return [tool_call({"operation": "delete", "table": T_PENDING,
                       "where": {"turn_id": tid}}, "p-gone-" + tid)]


def w_journal(rec):
    tid = rec["turn_id"]
    return [tool_call({"operation": "delete", "table": T_JOURNAL,
                       "where": {"turn_id": tid}}, "j-del-" + tid),
            tool_call({"operation": "insert", "table": T_JOURNAL,
                       "row": journal_row(rec, now_ms())}, "j-ins-" + tid)]


def w_shows(owner, known):
    legs = [tool_call({"operation": "delete", "table": T_SHOWS,
                       "where": {"owner_app": owner}}, "s-del")]
    for name in sorted(known):
        v = known[name]
        if v["owner"] != owner:
            continue
        legs.append(tool_call({"operation": "insert", "table": T_SHOWS,
                               "row": {"topic": name, "owner_app": owner,
                                       "show_at": v.get("show_at") or "",
                                       "manifest": v["manifest"], "at": v["at"]}},
                              "s-ins-" + name))
    return legs


def boot_read():
    return bundle(PHASE_BOOT, [
        tool_call({"operation": "select", "table": T_SHOWS, "columns": SHOWS_COLUMNS},
                  "b-shows"),
        tool_call({"operation": "select", "table": T_PENDING, "columns": PENDING_COLUMNS},
                  "b-pending")])


def rows_of(body, tid):
    for m in body.get("messages") or []:
        if isinstance(m, dict) and str(m.get("id") or "") == tid:
            try:
                rows = json.loads(str(m.get("text") or ""))
            except (TypeError, ValueError):
                return []
            return [r for r in rows if isinstance(r, dict)] if isinstance(rows, list) else []
    return []


def as_json(value):
    if isinstance(value, str):
        try:
            return json.loads(value)
        except ValueError:
            return None
    return value


def store_failed(body, hop):
    if hop.get("error_code"):
        return str(hop["error_code"])
    for entry in body.get("results") or []:
        if isinstance(entry, dict) and entry.get("error_code"):
            return "%s on %s" % (entry["error_code"], entry.get("operation") or "?")
    return None


# ---------------------------------------------------------------------------
# The clock: one one-shot for the earliest moment anything is due


def next_due(r):
    at = None
    for rec in r["pending"].values():
        if rec["state"] == "asking":
            t = rec["deadline"]
        elif rec["state"] == "showing":
            # A shown turn waits for its data deadline too: it closes there (review I1),
            # with or without a block.
            t = rec["data_deadline"]
        else:
            continue
        if t is not None and (at is None or t < at):
            at = t
    return at


def clock_ops(r, now, struck=""):
    """Zero, one or two orders: remove the standing one, add the next (GH #553: no poll)."""
    at = next_due(r)
    old = r.get("due") or {}
    if old.get("sid") == struck:
        old = {}
    ops = []
    if at is None:
        if old:
            ops.append(emission("clock", {"op": "remove", "schedule_id": old["sid"]}))
        r["due"] = None
        return ops
    at = max(int(at), now + 50)
    if old.get("at") == at:
        return ops
    sid = str(uuid.uuid5(DUE_NAMESPACE, "deadline:%d" % at))
    if old:
        ops.append(emission("clock", {"op": "remove", "schedule_id": old["sid"]}))
    ops.append(emission("clock", {"op": "add", "schedule_id": sid,
                                  "schedule_name": DEADLINE, "at": iso_z(at),
                                  "emit_to": ".", "emit_body": {"messages": []}}))
    r["due"] = {"sid": sid, "at": at}
    return ops


# ---------------------------------------------------------------------------
# Lanes


def text_of(body):
    said = []
    for m in body.get("messages") or []:
        if isinstance(m, dict) and m.get("type", "text") == "text" \
                and isinstance(m.get("text"), str):
            said.append(m["text"])
    return " ".join(s for s in said if s.strip()).strip()


def lane_topics():
    """Every committed mutation asks every app with `shows` for its topics (P.3)."""
    return [emission("in_show", {"messages": [], "op": "topics"})]


def lane_show_topics(body, hop, ctx, params):
    """An app said its topics. The owner is `hop.show_app`, stamped by the edge that
    carried the answer (OR-DP-54) -- never the body. An answer with an EMPTY `show_app` is
    the default edge of a presenter with no app at all: a sentinel, no row, no refusal."""
    r = ram()
    owner = str(hop.get("show_app") or ctx.get("show_app") or "")
    if not owner:
        return []
    at = str(hop.get("show_at") or "")
    known, refused = accept_topics(r["shows"], owner, body.get("topics"),
                                   catalogue(params))
    for v in known.values():
        if v["owner"] == owner and at:
            v["show_at"] = at
    r["shows"] = known
    out = [bundle(PHASE_WRITE, w_shows(owner, known))]
    for topic, reason in refused:
        out.append(emission("in_show", {"messages": [], "op": "refused", "topic": topic,
                                        "reason": reason}, **app_head(owner, at)))
    return out


def app_head(owner, at):
    """The hop keys that address one app: `show_app`, and `show_at` where the edge said
    it (OR-DP-54)."""
    head = {"show_app": owner}
    if at:
        head["show_at"] = at
    return head


def prune(r, now):
    legs = []
    for tid in list(r["pending"]):
        rec = r["pending"][tid]
        if rec["state"] == "done" and now - int(rec.get("done_at") or now) > KEEP_DONE_MS:
            del r["pending"][tid]
            legs += w_pending_gone(tid)
    return legs


def lane_turn(body, hop, ctx, params, now):
    r = ram()
    text = text_of(body)
    if not text or not r["shows"]:
        return []
    turn_id = str(hop.get("turn_id") or ctx.get("turn_id") or "")
    if not turn_id:
        turn_id = "t-%d" % now
    if turn_id in r["pending"]:
        return []
    rec = fresh_turn(turn_id, now, knob(params, "budget_ms"),
                     as_round(ctx.get("audience_set")))
    # The request travels to the owning app after a sure verdict (it needs the words:
    # "the weather in <place>"); it never reaches the journal and leaves the pending row
    # when the turn is done.
    rec["text"] = text
    r["pending"][turn_id] = rec
    legs = prune(r, now) + w_pending(rec)
    return [decide_call(text, r["shows"], turn_id), bundle(PHASE_WRITE, legs)] + \
        clock_ops(r, now)


def close(rec, now):
    """The turn is done: the request text, the data sets and the bound blocks leave the
    pending row (OR-DP.P.4 -- the words are transient, review I1). The journal row is
    the one written before; nothing on the screen changes."""
    rec["state"] = "done"
    rec["done_at"] = now
    rec["sets"] = {}
    rec["_nodes"] = []
    rec["text"] = ""
    return w_pending(rec)


def finish(rec, fallback, now):
    rec["fallback"] = fallback
    close(rec, now)
    return w_pending(rec) + w_journal(rec)


def settled(rec, manifest):
    """True when nothing more can be placed: the first slot is decided and every chosen
    also is placed or its set already came (a set never changes once there, and a block
    binds only its own set)."""
    if rec["first"] == "open":
        return False
    cands = {c["key"]: c for c in manifest["candidates"]}
    for k in rec["keys"][1:]:
        c = cands.get(k)
        if k not in rec["placed"] and c is not None and c["set"] not in rec["sets"]:
            return False
    return True


def supersede(r, rec, now):
    """One window per topic (P § 5): a newer turn of the topic replaces the older one.
    Returns `(legs, stale)` -- the writes that close every older open turn of the topic,
    and whether `rec` itself is older than a turn that already opened the window (then
    it opens nothing). A closed turn's deadline and late data never reach the newer
    window (review I2); an older turn without a block journals `no_data`."""
    legs = []
    stale = False
    for other in r["pending"].values():
        if other is rec or other.get("topic") != rec["topic"]:
            continue
        if other["arrived"] > rec["arrived"] and other.get("t_window") is not None:
            stale = True
        elif other["arrived"] <= rec["arrived"] and other["state"] == "showing":
            legs += close(other, now) if other["placed"] else finish(other, "no_data", now)
    return legs, stale


def lane_decision(body, hop, params, now):
    r = ram()
    tid = str(hop.get("show_id") or "")
    rec = r["pending"].get(tid)
    if rec is None:
        return []
    failed = hop.get("finish_reason") == "error" or bool(hop.get("error_code"))
    if rec["state"] != "asking":
        if rec["state"] == "done" and rec["fallback"] == "timeout" and not rec["late"]:
            # A late verdict changes nothing on the screen; the journal says it came.
            rec["late"] = True
            if not failed:
                v = read_verdict(body, r["shows"])
                rec["topic"], rec["p"], rec["model"] = v["topic"], v["p"], v["model"]
            return [bundle(PHASE_WRITE, w_pending(rec) + w_journal(rec))]
        return []
    rec["t_verdict"] = now - rec["arrived"]
    if failed:
        code = str(hop.get("error_code") or "")
        fb = "no_selector" if code == "decisions_unconfigured" else "error"
        return [bundle(PHASE_WRITE, finish(rec, fb, now))] + clock_ops(r, now)
    v = read_verdict(body, r["shows"])
    rec["topic"], rec["p"], rec["model"] = v["topic"], v["p"], v["model"]
    rec["lead"], rec["also"] = v["lead"], v["also"] if v["also"] != NONE else None
    why = judge(v, r["shows"], params)
    if why:
        return [bundle(PHASE_WRITE, finish(rec, why, now))] + clock_ops(r, now)
    older, stale = supersede(r, rec, now)
    if stale:
        return [bundle(PHASE_WRITE, older + finish(rec, "no_data", now))] + clock_ops(r, now)
    show = r["shows"][v["topic"]]
    manifest = show["manifest"]
    rec["keys"] = chosen(v, manifest, params)
    rec["state"] = "showing"
    rec["owner"] = show["owner"]
    wait = manifest.get("data_wait_ms") or knob(params, "data_wait_ms")
    rec["data_deadline"] = now + int(wait)
    rec["t_window"] = now - rec["arrived"]
    tree = window(v["topic"], manifest, tid, [], knob(params, "work_hint"), now)
    ask = emission("in_show", {"messages": [], "op": "data", "topic": v["topic"],
                               "request": rec.get("text") or "",
                               "lead": rec["keys"][0], "also": rec["keys"][1:],
                               "turn_id": tid,
                               "sets": wanted_sets(manifest, rec["keys"])},
                   **app_head(show["owner"], show.get("show_at") or ""))
    return [a_view(v["topic"], tree), ask, bundle(PHASE_WRITE, older + w_pending(rec))] + \
        clock_ops(r, now)


def lane_show_data(body, hop, ctx, params, now):
    r = ram()
    tid = str(body.get("turn_id") or hop.get("turn_id") or "")
    rec = r["pending"].get(tid)
    if rec is None or rec["state"] != "showing" or rec["topic"] != body.get("topic"):
        return []
    show = r["shows"].get(rec["topic"])
    if show is None:
        return []
    sender = str(hop.get("show_app") or ctx.get("show_app") or "")
    if sender != rec["owner"]:
        # The edge stamps `show_app` (OR-DP-54); only the topic's owner fills its window
        # (review I3). Said out loud, never placed.
        return [error("foreign_data", "show_data for topic %s from %r; its owner is %s"
                      % (rec["topic"], sender, rec["owner"]))]
    manifest = show["manifest"]
    screen = params.get("screen_audience")
    sets = body.get("sets") if isinstance(body.get("sets"), dict) else {}
    for name, data in sets.items():
        kept = filter_set(data, screen)
        if kept is not None and name not in rec["sets"]:
            rec["sets"][name] = kept
        elif kept is None and name not in rec["sets"]:
            # A set the screen may not see arrived: it counts as there and empty, so a
            # lead waiting on it falls to the standard rather than to the clock.
            rec["sets"][name] = {}
    new, fallback = resolve(rec, manifest, catalogue(params))
    if not new:
        if rec["placed"] and settled(rec, manifest):
            return [bundle(PHASE_WRITE, close(rec, now))] + clock_ops(r, now)
        return [bundle(PHASE_WRITE, w_pending(rec))]
    first = not rec["placed"]
    rec["placed"] += [k for k, _ in new]
    blocks = [node for _, node in rec.get("_nodes", [])] + [n for _, n in new]
    rec["_nodes"] = rec.get("_nodes", []) + new
    tree = window(rec["topic"], manifest, tid, blocks, "", None)
    out = [a_view(rec["topic"], tree)]
    legs = []
    if first:
        rec["t_content"] = now - rec["arrived"]
        rec["fallback"] = fallback or "none"
        legs += w_journal(rec)
    # Every chosen block placed or final: the turn closes now, not at data_wait_ms.
    legs += close(rec, now) if settled(rec, manifest) else w_pending(rec)
    out.append(bundle(PHASE_WRITE, legs))
    return out + clock_ops(r, now)


def lane_tick(hop, now):
    r = ram()
    struck = str(hop.get("schedule_id") or "")
    out = []
    legs = []
    for tid in sorted(r["pending"]):
        rec = r["pending"][tid]
        if rec["state"] == "asking" and now >= rec["deadline"]:
            legs += finish(rec, "timeout", now)
        elif rec["state"] == "showing" and \
                rec["data_deadline"] is not None and now >= rec["data_deadline"]:
            if rec["placed"]:
                # The blocks stay; the turn closes and its words leave (review I1).
                legs += close(rec, now)
            else:
                out.append(withdraw(rec["topic"]))
                legs += finish(rec, "no_data", now)
    if legs:
        out.append(bundle(PHASE_WRITE, legs))
    return out + clock_ops(r, now, struck)


# ---------------------------------------------------------------------------
# The store's answers


def pass_store(body, hop, ctx):
    phase = str(ctx.get("show_phase") or "?")
    failed = store_failed(body, hop)
    if phase != PHASE_BOOT:
        if failed:
            return [error("store_failed", "the %s bundle failed: %s" % (phase, failed))]
        return []
    r = ram()
    out = []
    if failed:
        out.append(error("store_failed", "the boot read failed: %s" % failed))
    else:
        for row in rows_of(body, "b-shows"):
            m = as_json(row.get("manifest"))
            if isinstance(m, dict) and isinstance(row.get("topic"), str):
                r["shows"][row["topic"]] = {"owner": str(row.get("owner_app") or ""),
                                            "show_at": str(row.get("show_at") or ""),
                                            "manifest": m, "at": int(row.get("at") or 0)}
        for row in rows_of(body, "b-pending"):
            rec = as_json(row.get("value"))
            if isinstance(rec, dict) and rec.get("turn_id"):
                r["pending"][rec["turn_id"]] = rec
    r["phase"] = "live"
    queue, r["queue"] = r["queue"], []
    for doc in queue:
        out += handle(doc)
    return out


# ---------------------------------------------------------------------------
# The dispatcher


def handle(doc):
    body = doc.get("body") or {}
    params = doc.get("params") or {}
    env = doc.get("envelope") or {}
    header = env.get("header") or {}
    hop = header.get("hop") or {}
    ctx = header.get("context") or {}
    origin = str(ctx.get("show_origin") or "")
    route = str(hop.get("route") or "")
    now = now_ms()
    r = ram()

    if origin == "store":
        return pass_store(body, hop, ctx)
    if route == "mutation_committed":
        return lane_topics()
    if route in ("event", "receipt"):
        return []
    if r["phase"] != "live":
        # Cold: this event waits for the read of the two tables (RAM is a cache).
        doc = dict(doc)
        doc["_at"] = now
        r["queue"].append(doc)
        if r["phase"] == "cold":
            r["phase"] = "booting"
            return [boot_read()]
        return []
    if "_at" in doc:
        now = int(doc["_at"])
    # The decider's answer: the edge back from `./decide` deletes the inner context
    # (no marker leaves the hive, gh494) and restamps the hop `in_decision` with the
    # turn's `show_id` -- a colony run read `context {}` on that hop and dropped every
    # verdict as routeless. Only that edge sets the route: the door from `.` passes a
    # fixed list of routes that does not include it.
    if origin == "decide" or route == "in_decision":
        return lane_decision(body, hop, params, now)
    if route == "in_tick":
        return lane_tick(hop, now)
    if route == "in_tick_error":
        return [error("clock_refused", str(body.get("detail") or hop.get("error_code")
                                             or "the clock refused an order"))]
    if route == "show_topics":
        return lane_show_topics(body, hop, ctx, params)
    if route == "show_data":
        return lane_show_data(body, hop, ctx, params, now)
    if route == "turn":
        return lane_turn(body, hop, ctx, params, now)
    return []


def main():
    return handle(json.load(sys.stdin))


if __name__ == "__main__":
    out = main()
    sys.stdout.write(json.dumps(out[0] if len(out) == 1 else out))
