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

import html
import json
import sys
import time
import uuid
from urllib.parse import urlsplit

# ---------------------------------------------------------------------------
# Names

T_SHOWS = "shows"
T_PENDING = "pending"
T_JOURNAL = "journal"
T_WORK = "work"
SHOWS_COLUMNS = ["topic", "owner_app", "show_at", "manifest", "at"]
PENDING_COLUMNS = ["turn_id", "value", "at"]
WORK_COLUMNS = ["seq", "session", "turn_id", "tool", "call_id", "state", "summary", "path",
                "exit_code", "audience_set", "at"]

PHASE_BOOT = "boot"
PHASE_WRITE = "write"

FALLBACKS = ("none", "unsure", "no_topic", "timeout", "error", "no_selector",
             "invalid", "no_data", "foreign_round", "form_none")

# The decider's call carries 1 + 2*N questions (`topic`, then `<t>.lead` and
# `<t>.also` per topic); the decider takes at most DECIDER_QUESTIONS (E.2, the
# llm cell's `MAX_QUESTIONS` in `translate_decisions.rs`, a mirror), so
# MAX_TOPICS is derived from it: 31 topics are 63 questions. A topic past it is
# refused as `overflow` (an app's) or named on stderr (a built-in one), never
# left out unsaid (GH #1098).
DECIDER_QUESTIONS = 64
MAX_TOPICS = (DECIDER_QUESTIONS - 1) // 2
MAX_CANDIDATES = 5
NONE = "none"
NONE_TOPIC = "nothing on the screen helps with this request"
NONE_ALSO = "no second block"

# GH #1097 -- the verdict's deadline comes from the timeout chain (docs/cell-types.md
# § llm, the decisions road): the slowest measured decision took 1007 ms (the decisions road
# measurement, eight in parallel, 2026-10-02; 41 questions on the live wire 495 ms), the decider's own
# operation timeout is twice that rounded up to the second (`decide`
# `external_timeout_ms` 3000), and the stage waits that plus 1000 ms for the verdict's
# way back (measured below 1 ms: t_verdict equals t_window). So a verdict the decider
# may still deliver is never `late`; a template test holds the three numbers together.
DEFAULTS = {
    "budget_ms": 4000,
    "threshold": 0.7,
    "data_wait_ms": 4000,
    "also_threshold": 0.5,
    "work_hint": "Working on it…",
    # R-HP-18: off, a window shows all of the member's data (`window_round`); on is the
    # way back to R-HP-9 (c), see `gate_round`.
    "window_requires_star_data": False,
    # live-2 GR (display-hive.md § 9b.14): how long the frame of a window that is done
    # takes to come to rest, a duration the sheet spends once (R-FU-1).
    "settle_ms": 600,
    # live-2 GR: off, the lane `screen_topics` stays silent -- a presenter whose parent
    # draws no edge for it says nothing that would dead-letter. On, the topic table leaves
    # on boot and on every change of it (`topic_table`).
    "screen_topics": False,
}

PANE = "display-pane"
STACK = "display-stack"
STATUS = "display-status"
TILE = "display-tile"
LAYER_CANVAS = "canvas"
CONTEXT = "conversation"
RELEVANCE = "0.8"
TILE_CHARS = 24

# live-2 GR (display-hive.md § 9b.12-9b.14, R-FU-1, R-FU-4): where the verdict comes from.
# `selector` (shipped) is the one call of § 9b.1; `stamp` takes it from the talky's stamp
# on `in_verdict` and asks nobody -- the turn's verdict is judged once, by the talky.
SOURCE_SELECTOR = "selector"
SOURCE_STAMP = "stamp"
SKELETON = "display-skeleton"
BUILDING = "building"
# The screen forms of the stamp and the fixed map to a rung (R-FU-4). `urgent` is not in
# it and cannot be put in it: urgent comes from apps and guards, never from a verdict.
FORMS = ("active", "accompanying", "referring", "none")
FORM_RUNGS = {"active": "focus", "accompanying": "relevant", "referring": "relevant",
              "none": "none"}
# #1417: the form of a stamp-way turn that ends without one (live-2 T5 round 2b: two judge
# timeouts at 6 s left `form` empty, the window `relevant` but unnamed). A structural
# default, never read from words: its rung is `relevant`, the window's standing weight, so
# naming it moves nothing -- never `focus` without a verdict (R-FU-4).
FORM_FALLBACK = "referring"
# The presenter writes no rung -- the curator does (§ 4.17-4.23). It writes the hint the
# rung follows: `focus` leads its ladder by score, `relevant` keeps today's weight
# (OR-LV.GR-1, display-hive.md § 9b.13).
RUNG_RELEVANCE = {"focus": "1", "relevant": RELEVANCE}
SKELETON_LINES = 3

# A done turn is kept this long, so a late verdict can still be told from a stray one.
KEEP_DONE_MS = 60000

DEADLINE = "deadline"
DUE_NAMESPACE = uuid.UUID("6f1c2a52-9f4e-4c1b-8d55-3c0b6e5a9d01")

TYPES = ("text", "int", "number", "boolean", "html")

# ---------------------------------------------------------------------------
# GH #963: what the presenter observes, and its own topics

# The tools whose CALLS the app block observes (`observes_tool_calls`), exactly the
# occupants of a generation's own `./tools` hive (the `tools` template): their results are what the
# string-form result observer hears from that hive. The file tool is `file` (op read,
# write, list, stat), not `file_*`; the member's file space answers on its own road, which
# no observer leg reaches (OR-DP.Q.1). A longer list is a pin change of the presenter.
OBSERVED_TOOLS = ("web_search", "web_fetch", "bash", "file", "edit")

# The rolling work view: at most this many rows per session in the store (a view with a
# ceiling; the colony log stays the record), and this many steps on the screen.
WORK_ROWS = 50
WORK_SHOWN = 12
SEARCH_HITS = 6
SNIPPET_CHARS = 200
SUMMARY_CHARS = 64

# A bash command is a test run when one of these is in it (lower case, plain substring:
# deterministic, no model). `params.test_patterns` replaces the list.
DEFAULT_TEST_PATTERNS = ["cargo test", "cargo nextest", "pytest", "npm test", "npm run test",
                         "yarn test", "pnpm test", "go test", "make test", "make check",
                         "unittest", "test.sh", "test-tier.sh", "gate.sh"]

# A declared source (Q.4, the seam with the residents' road, OR-DP.M.1):
# `{read, hop, body, rows?, value?}`, closed. `read` is one of the residents the builder's
# `reads_residents` reaches; `hop` may set only the keys a resident's read lane takes
# (closed, so `contract.emits` can name them); a string value that is exactly `$request`
# becomes the turn's text.
SOURCE_KEYS = ("read", "hop", "body", "rows", "value")
RESIDENTS = ("memory-hive", "file-space", "graph-space", "objects", "librarian", "affinity",
             "colony-view", "daily-digest", "research-assistant")
SOURCE_HOP_KEYS = ("op", "recall_query", "memory_tier")
SOURCE_MAX_BYTES = 4096
REQUEST = "$request"
# The keys a candidate may carry, closed (OR-DP-68, OR-DP-77): a key the presenter does not
# read is a manifest it would misread, so it is refused, never ignored. Measured 03.10.:
# every manifest of the library, the residents' topics and the apps beside it uses only
# these eight.
CANDIDATE_KEYS = ("key", "block", "describe", "set", "bind", "children", "on_choice", "source")

# The presenter's own topics, fed by what it observes rather than by an app. Each is
# switched on by name in `params.observed_topics`; an app may not hold their names.
BUILTIN = {
    "search": {
        "topic": "search", "title": "Web search", "glyph": "\u2315", "standard": "results",
        "describe": "results of a web search the assistant ran for this request",
        "data_wait_ms": 15000,
        "candidates": [
            {"key": "results", "block": "display-list", "set": "hits",
             "describe": "the hits as a list: title and site",
             "bind": {"title": "=Results"},
             "children": [{"each": "hits", "block": "display-item",
                           "bind": {"k": "$.title", "v": "$.host"}}]},
            {"key": "top", "block": "display-card", "set": "top",
             "describe": "the first hit as a card: site, title and its snippet",
             "bind": {"kicker": "top.host", "title": "top.title", "body": "top.snippet"}},
            {"key": "sources", "block": "display-table", "set": "sources",
             "describe": "which sites the hits came from, with a count each",
             "bind": {"caption": "sources.caption", "head": "sources.head",
                      "rows": "sources.rows"}}]},
    "work": {
        "topic": "work", "title": "Work report", "glyph": "\u2699", "standard": "steps",
        "describe": "what the assistant is working on right now: its steps, the files it "
                    "touched, whether tests passed",
        "data_wait_ms": 15000,
        "candidates": [
            {"key": "steps", "block": "display-steps", "set": "steps",
             "describe": "the steps taken so far, each with its state",
             "bind": {"title": "=Steps"},
             "children": [{"each": "steps", "block": "display-step",
                           "bind": {"label": "$.label", "state": "$.state",
                                    "detail": "$.detail"}}]},
            {"key": "files", "block": "display-list", "set": "files",
             "describe": "the files touched so far",
             "bind": {"title": "=Files"},
             "children": [{"each": "files", "block": "display-item",
                           "bind": {"k": "$.path"}}]},
            {"key": "tests", "block": "display-card", "set": "tests",
             "describe": "whether the last test run passed",
             "bind": {"kicker": "=Tests", "title": "tests.verdict", "body": "tests.detail"}}]},
}


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
    if isinstance(d, bool):
        # A switch: a JSON boolean or its word; anything else keeps the default.
        if isinstance(v, bool):
            return v
        w = str(v).strip().lower() if v is not None else ""
        return {"true": True, "on": True, "1": True,
                "false": False, "off": False, "0": False}.get(w, d)
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


def gate_round(turn_round, screen_round, params):
    """The audience gate before the window: what a turn of `turn_round` may open on a
    screen of `screen_round`. One of

    - "open": the round covers the screen's (or the screen has none) -- the window opens
      on the verdict, as always;
    - "foreign": no member of the screen's round is in the turn, or the turn has no
      round -- nothing is asked, nothing opens (GH #1027);
    - "star": a member of the screen's round is in the turn, but the round does not
      cover the screen (a third party shares it), and `window_requires_star_data` is on
      (R-HP-9 c, the way back): the window opens only on `*` data left after the screen's
      gate, never with the working hint and never with the standard block instead. Off
      (shipped, R-HP-18), "open": the window shows all of the member's data, gated by
      `window_round`. The owner, 06.10.: a window is almost always the member's phone or
      private browser, the shared TV the rare case; the chat keeps its audience.
    """
    screen = as_round(screen_round) or []
    if not screen or covers(turn_round, screen):
        return "open"
    have = as_round(turn_round) or []
    if not any(m in have for m in screen if m.startswith("member:")):
        return "foreign"
    return "star" if knob(params, "window_requires_star_data") else "open"


def window_round(rec, params):
    """The round a turn's window data are gated by (R-HP-18): the members of the screen's
    round that are in the turn. A turn that covers the screen gets the screen's round, as
    always; a member's turn on a screen a third party shares gets the member's part of it,
    so the window shows all of the member's data and never another member's rows (the
    third party's presence does not filter, identity does -- a foreign round opened nothing
    before this is asked, GH #1027). A screen without a round stays at `*` (empty), and
    with `window_requires_star_data` on (`star_only`) the screen's whole round gates."""
    screen = as_round(params.get("screen_audience")) or []
    if not screen or rec.get("star_only"):
        return screen
    have = as_round(rec.get("audience_set")) or []
    return [m for m in screen if m in have]


def filter_set(data_set, screen_round):
    """One data set after the audience gate, or None where nothing of it may be shown.

    The set's own `audience_set` must cover the round, or the whole set is gone, marked
    rows included (fail-closed: a set without a round shows nothing). A row with its OWN
    `audience_set` is gated by it alone; a row without one inherits the set's round,
    because the set's stamp is the app's statement about all of its rows (OR-DP-56: the
    weather app sends public sets `["*"]` with unmarked rows). `value` stays as it came,
    because an app derives a value from rows under the intersection of their rounds
    (OR-DP-10) -- unless it carries its OWN `audience_set`: then it is gated by that like
    a row (GH #976, PE-DP-9: a source whose `value` is one row of the answer, the newest
    digest or research answer, brings that row's round along, and the set's round is only
    the member's).
    """
    if not isinstance(data_set, dict):
        return None
    if not covers(data_set.get("audience_set"), screen_round):
        return None
    out = {}
    value = data_set.get("value")
    if isinstance(value, dict) and ("audience_set" not in value
                                    or covers(value.get("audience_set"), screen_round)):
        out["value"] = value
    rows = data_set.get("rows")
    if isinstance(rows, list):
        out["rows"] = [r for r in rows if isinstance(r, dict)
                       and ("audience_set" not in r
                            or covers(r.get("audience_set"), screen_round))]
    if not out.get("rows") and "value" not in out:
        # Nothing of it may be shown: the set is not there. Kept as an empty set it
        # could stand as an empty block and tell the screen that rows of other rounds
        # exist (R fix round 2: rows `[member, Y]` and `[X, Z]`, the set their union,
        # the screen `[member, X]`). A set that never came reads the same.
        return None
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


def check_source(src):
    """None when a declared source is `{read, hop, body, rows?, value?}` in the closed form
    of OR-DP.M.1; else the reason. `rows`/`value` are dotted paths into the answer."""
    if not isinstance(src, dict):
        return "source is not an object"
    for k in src:
        if k not in SOURCE_KEYS:
            return "source key %r" % k
    if src.get("read") not in RESIDENTS:
        return "source names no resident the presenter reads"
    hop = src.get("hop", {})
    if not isinstance(hop, dict):
        return "source hop is not an object"
    for k, v in hop.items():
        if k not in SOURCE_HOP_KEYS:
            return "source hop key %r" % k
        if isinstance(v, bool) or not isinstance(v, (str, int)):
            return "source hop %s is no plain value" % k
    if not isinstance(src.get("body", {}), dict):
        return "source body is not an object"
    for k in ("rows", "value"):
        if k in src and not isinstance(src[k], str):
            return "source %s is not a path" % k
    if "rows" not in src and "value" not in src:
        return "source reads neither rows nor a value"
    if len(json.dumps(src)) > SOURCE_MAX_BYTES:
        return "source exceeds %d bytes" % SOURCE_MAX_BYTES
    return None


def check_candidate(cand, cat, own=False):
    if not isinstance(cand, dict):
        return "a candidate is not an object"
    extra = sorted(k for k in cand if k not in CANDIDATE_KEYS)
    if extra:
        return "candidate %s: unknown key %s" % (cand.get("key"), ", ".join(map(repr, extra)))
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
    if "source" in cand:
        # Only the presenter's own topics read a resident (Q.4): an app that named one
        # would have the presenter read for it, in the screen's round.
        if not own:
            return "candidate %s: a source is for the presenter's own topics" % cand.get("key")
        why = check_source(cand["source"])
        if why:
            return "candidate %s: %s" % (cand.get("key"), why)
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


def check_topic(topic, cat, own=False):
    """None when the manifest entry of one topic is well formed; else the reason. `own`
    is true for the presenter's own topics, the only ones that may declare a source."""
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
        why = check_candidate(cand, cat, own)
        if why:
            return "topic %s: %s" % (name, why)
        if cand["key"] == NONE or cand["key"] in keys:
            return "topic %s: candidate key %s twice or reserved" % (name, cand["key"])
        keys.append(cand["key"])
    if topic.get("standard") not in keys:
        return "topic %s: the standard is not one of its candidates" % name
    return None


def accept_topics(known, owner, topics, cat, reserved=(), taken=0):
    """The `shows` table after one app said its topics.

    `known` is `{topic: {owner, manifest, at}}`. The app's own earlier rows are replaced
    as a whole; a topic another app already holds stays with that app (`duplicate`); a
    malformed one gets no row; a name of the presenter's own topics `reserved`; topics
    past the call's ceiling (`taken` places held by the presenter's own), alphabetically last,
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
        if name in reserved:
            refused.append((name, "reserved"))
            continue
        if name in seen:
            refused.append((name, "duplicate"))
            continue
        seen.add(name)
        if name in out:
            refused.append((name, "duplicate"))
            continue
        fresh.append(topic)
    room = MAX_TOPICS - len(out) - taken
    fresh.sort(key=lambda t: t["topic"])
    for i, topic in enumerate(fresh):
        if i >= room:
            refused.append((topic["topic"], "overflow"))
            continue
        out[topic["topic"]] = {"owner": owner, "manifest": topic, "at": now_ms()}
    return out, refused


def builtin_topics(params):
    """The presenter's own topics, `{topic: {owner: "", manifest, at, kind}}`: the observed
    ones switched on in `params.observed_topics` (Q.2/Q.3), then the declared-source ones
    of `params.builtin_topics` (Q.4, written by the strand that ships them). A built-in
    topic that does not stand against the catalogue copy is left out, never offered half."""
    cat = catalogue(params)
    out = {}
    names = params.get("observed_topics")
    for name in names if isinstance(names, list) else []:
        if name in BUILTIN and name not in out:
            out[name] = {"owner": "", "manifest": BUILTIN[name], "at": 0, "kind": "observed"}
    extra = params.get("builtin_topics")
    for m in extra if isinstance(extra, list) else []:
        name = m.get("topic") if isinstance(m, dict) else None
        if name in out or name in BUILTIN or check_topic(m, cat, own=True) is not None:
            continue
        if len(out) >= MAX_TOPICS:
            sys.stderr.write("presenter/stage: built-in topic %s left out -- the decider's "
                             "call holds %d topics (MAX_TOPICS)\n" % (name, MAX_TOPICS))
            continue
        out[name] = {"owner": "", "manifest": m, "at": 0, "kind": "source"}
    return out


def all_topics(r, params):
    """Every topic the decider is asked about: the presenter's own first, then the apps'."""
    out = builtin_topics(params)
    for name, v in r["shows"].items():
        if name not in out:
            out[name] = v
    return out


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
        # A choice needs two options (the decisions wire refuses one with `decide_invalid`,
        # MIN_OPTIONS in translate_decisions.rs -- and with it the WHOLE call, every topic's
        # verdict; measured 03.10. on the first colony run of a one-candidate topic). A
        # topic with one candidate has nothing to choose: no lead question, the standard
        # (its one candidate) leads.
        if len(cands) > 1:
            qs[name + ".lead"] = {
                "kind": "choice",
                "instructions": "Which block shows best what the request asks about %s?"
                                % m["title"],
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


def missing_of(body, asked=None):
    """The question keys the decider left unanswered (GH #977), sorted, keys only.

    The `decisions` cell emits a decision with the answers it got and `missing` beside
    them; only a call with no answer at all is still `decision_incomplete`. What it says is
    read as a list of strings and nothing more -- and, given the keys of the questions
    that were `asked`, only those: a foreign body would otherwise carry any text into the
    journal's column (E2 review m-3).
    """
    d = body.get("decision") if isinstance(body.get("decision"), dict) else body
    m = d.get("missing") if isinstance(d, dict) else None
    if not isinstance(m, list):
        return []
    keys = {k for k in m if isinstance(k, str) and k}
    if asked is not None:
        keys &= set(asked)
    return sorted(keys)


def topic_missing(body):
    """True when the verdict carries no answer to `topic` at all: without it no topic is
    chosen, and the turn is an `error` as before (fail-closed, GH #977). A topic whose own
    questions are missing merely loses its lead (the standard leads) or its also."""
    d = body.get("decision") if isinstance(body.get("decision"), dict) else body
    answers = d.get("answers") if isinstance(d, dict) else None
    return not isinstance(answers, dict) or "topic" not in answers \
        or "topic" in missing_of(body)


def a_threshold(t):
    return isinstance(t, (int, float)) and not isinstance(t, bool) and 0 < t <= 1


def threshold_of(manifest, params, own=None):
    """The threshold a verdict on a topic must reach. `own` names one of the presenter's
    own topics: `params.builtin_thresholds` may give it a threshold of its own (OR-HP-66:
    the decider gave 0.57-0.62 for plain search and work requests against 0.7); an app's
    topic keeps what its manifest says. Then the manifest's, then `threshold`."""
    per = params.get("builtin_thresholds")
    if isinstance(per, str):
        try:
            per = json.loads(per)
        except ValueError:
            per = None
    t = per.get(own) if own is not None and isinstance(per, dict) else None
    if a_threshold(t):
        return float(t)
    t = manifest.get("threshold")
    if a_threshold(t):
        return float(t)
    return knob(params, "threshold")


def judge(v, known, params):
    """`None` for a sure verdict, else the journal's fallback word."""
    if v["topic"] == NONE:
        return "no_topic"
    if v["topic"] is None:
        return "unsure"
    show = known[v["topic"]]
    if v["p"] < threshold_of(show["manifest"], params,
                             None if show.get("owner") else v["topic"]):
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


def window(topic, manifest, turn_id, blocks, hint, touched, relevance=RELEVANCE,
           inner=None, stack=None):
    """The view `show-<topic>`: a pane on the canvas, its tile, and either the working
    hint or the blocks placed so far. `touched` is said once, by the write that opens
    the window; a write that only adds a block leaves it out, and a key left out stands
    (§ 4.8): a block is no new touch. The stamp way (`stamp_window`) hands its own
    `inner` children, `stack` props and the `relevance` of its screen form."""
    vid = "show-" + topic
    props = {"pane_id": vid, "kicker": manifest["title"], "title": "",
             "context": CONTEXT, "relevance": relevance, "topic": "show:" + topic,
             "layer": LAYER_CANVAS}
    if turn_id:
        props["turn_id"] = turn_id
    if touched:
        props["touched"] = str(int(touched))
    tile = {"component": TILE, "key": "tile",
            "props": {"glyph": str(manifest.get("glyph") or ""),
                      "line": shorten(manifest["title"]), "value": ""}}
    if inner is not None:
        inner = list(inner)
    elif blocks:
        inner = list(blocks)
    else:
        inner = [{"component": STATUS, "key": vid + "-hint",
                  "props": {"kind": "working", "text": hint}}]
    sprops = {"gap": "s"}
    sprops.update(stack or {})
    stack = {"component": STACK, "props": sprops, "key": vid + "-stack",
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


def fresh_turn(turn_id, arrived, budget, audience_set, seq=0):
    return {"turn_id": turn_id, "arrived": arrived, "seq": seq, "deadline": arrived + budget,
            "state": "asking", "topic": None, "p": 0.0, "lead": None, "also": None,
            "keys": [], "model": "", "t_verdict": None, "t_window": None,
            "t_content": None, "data_deadline": None, "sets": {}, "placed": [],
            "first": "open", "fallback": None, "late": False, "missing": [],
            "audience_set": audience_set, "owner": None, "done_at": None, "observed": {}}


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
            # A topic with one candidate is asked no lead question: its standard leads
            # by plan, and nothing was invalid (Q review N-2).
            fallback = "none" if rec["lead"] or len(cands) == 1 else "invalid"
            new.append((lead["key"], node))
        elif lead and lead["set"] in rec["sets"]:
            std = cands.get(manifest["standard"])
            if rec.get("star_only"):
                # R-HP-9 (c): no standard block stands in on a shared screen.
                rec["first"] = "failed"
            elif std is not None and std["key"] != lead["key"]:
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
    row = {"turn_id": rec["turn_id"], "topic": rec["topic"] or "",
            "p": float(rec["p"] or 0.0), "lead": rec["lead"] or "",
            "also": rec["also"] or "", "fallback": rec["fallback"] or "",
            "late": 1 if rec["late"] else 0, "t_verdict_ms": ms(rec["t_verdict"]),
            "t_window_ms": ms(rec["t_window"]), "t_content_ms": ms(rec["t_content"]),
            "model": rec["model"] or "", "at": int(at),
            "missing": json.dumps(rec.get("missing") or []),
            "audience_set": json.dumps(rec["audience_set"]) if rec["audience_set"] is not None else ""}
    if rec.get("source") == SOURCE_STAMP:
        # display-hive.md § 9b.7 and § 1.6: the source and the three times of the window
        # under construction. The selector way writes the row it always wrote.
        row.update({"verdict_source": SOURCE_STAMP, "t_building_ms": ms(rec.get("t_building")),
                    "t_form_ms": ms(rec.get("t_form")), "t_done_ms": ms(rec.get("t_done")),
                    # #1417: the form the turn ended with (`referring` when none came,
                    # `form_fallback`) and whether the judge's form came after that.
                    "form": rec.get("form") or "",
                    "form_late": 1 if rec.get("form_late") else 0})
    return row


# ---------------------------------------------------------------------------
# RAM, written through


RAM_V = 1


def ram():
    held = globals().get("_RAM")
    if isinstance(held, dict) and held.get("v") == RAM_V:
        return held
    held = {"v": RAM_V, "phase": "cold", "queue": [], "shows": {}, "pending": {},
            "due": None, "work": [], "seq": 0}
    globals()["_RAM"] = held
    return held


# One row per key, written in ONE step (`insert` with `replace`, GH #1199): as a delete
# leg and an insert leg, an insert that failed left the key with no row, and after a
# restart the turn in flight, its journal line, the app's topics or the work row was gone.
def w_pending(rec):
    tid = rec["turn_id"]
    return [tool_call({"operation": "insert", "table": T_PENDING,
                       "row": {"turn_id": tid, "value": rec, "at": now_ms()},
                       "replace": {"turn_id": tid}},
                      "p-ins-" + tid)]


def w_pending_gone(tid):
    return [tool_call({"operation": "delete", "table": T_PENDING,
                       "where": {"turn_id": tid}}, "p-gone-" + tid)]


def w_journal(rec):
    tid = rec["turn_id"]
    return [tool_call({"operation": "insert", "table": T_JOURNAL,
                       "row": journal_row(rec, now_ms()), "replace": {"turn_id": tid}},
                      "j-ins-" + tid)]


def w_shows(owner, known):
    rows = [{"topic": name, "owner_app": owner, "show_at": known[name].get("show_at") or "",
             "manifest": known[name]["manifest"], "at": known[name]["at"]}
            for name in sorted(known) if known[name]["owner"] == owner]
    if not rows:
        return [tool_call({"operation": "delete", "table": T_SHOWS,
                           "where": {"owner_app": owner}}, "s-del")]
    return [tool_call({"operation": "insert", "table": T_SHOWS, "rows": rows,
                       "replace": {"owner_app": owner}}, "s-ins")]


def boot_read():
    return bundle(PHASE_BOOT, [
        tool_call({"operation": "select", "table": T_SHOWS, "columns": SHOWS_COLUMNS},
                  "b-shows"),
        tool_call({"operation": "select", "table": T_PENDING, "columns": PENDING_COLUMNS},
                  "b-pending"),
        tool_call({"operation": "select", "table": T_WORK, "columns": WORK_COLUMNS},
                  "b-work")])


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
    if old.get("sid") == struck or (old and int(old.get("at") or 0) <= now):
        # GH #1047: a one-shot that struck is `completed` at the timer and gone. Its moment
        # has passed even when its strike still waits in this cell's queue behind the event
        # at hand (the race measured downstream: 6 `clock_refused` `schedule_not_found`
        # in 5 of 109 gate runs). A late strike only re-reads the deadlines.
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


# GH #877 class (release gate 0.64.3): an `add` that runs out of the clock's
# `query_timeout_ms` under database load stored nothing, so no deadline stands and a turn
# waiting on it never times out. Only the order this cell gave last (`ram()["due"]`, the
# answer names it back, GH #904) is such a loss -- a refusal about an order a later one
# replaced changes nothing. The deadline is ordered again ONCE: a second loss of the same
# order is said and orders nothing, so a host too loaded for any write cannot turn
# refusals into a loop.
CLOCK_TRANSIENT = ("query_timeout",)


def lost_deadline(hop, now):
    r = ram()
    sid = str(hop.get("schedule_id") or "")
    due = r.get("due") or {}
    if str(hop.get("error_code") or "") not in CLOCK_TRANSIENT or not sid \
            or due.get("sid") != sid or r.get("due_lost") == sid:
        return []
    r["due"] = None
    ops = clock_ops(r, now)
    r["due_lost"] = (r.get("due") or {}).get("sid") or sid
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


def topic_table(r, params):
    """The table the decider is asked over (`all_topics`: the presenter's own, then the
    apps' accepted ones), as `[{name, purpose, candidates}]` by name. `purpose` is the
    manifest's `describe`, the words the decider reads for the topic too; `candidates` is
    `[{key, describe}]` in the manifest's order, the options `questions` offers for the
    lead -- the talky's `in_screen_topics` contract (`{name, purpose, candidates?:
    [{key, describe}]}`, curator/history `screen_topics_of`, talky/route `screen_topics`).
    Without them talky's route never asks a lead question (#1368). A topic without
    candidates leaves the field out, as the contract's `candidates?` allows."""
    known = all_topics(r, params)
    table = []
    for name in sorted(known):
        m = known[name]["manifest"]
        row = {"name": name, "purpose": m["describe"]}
        cands = m.get("candidates")
        if isinstance(cands, list) and cands:
            row["candidates"] = [{"key": c["key"], "describe": c["describe"]} for c in cands]
        table.append(row)
    return table


def say_topics(r, params, boot=False):
    """`screen_topics {topics}` when the switch is on and the table changed since it was
    last said -- or on boot, once, whatever it holds. Never per turn."""
    if not knob(params, "screen_topics"):
        return []
    table = topic_table(r, params)
    if not boot and r.get("topics_said") == table:
        return []
    r["topics_said"] = table
    return [emission("screen_topics", {"topics": table})]


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
    own = builtin_topics(params)
    known, refused = accept_topics(r["shows"], owner, body.get("topics"),
                                   catalogue(params), set(BUILTIN) | set(own), len(own))
    for v in known.values():
        if v["owner"] == owner and at:
            v["show_at"] = at
    r["shows"] = known
    out = [bundle(PHASE_WRITE, w_shows(owner, known))] + say_topics(r, params)
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
    topics = all_topics(r, params)
    if not text or not topics:
        return []
    turn_id = str(hop.get("turn_id") or ctx.get("turn_id") or "")
    stamped = verdict_source(params) == SOURCE_STAMP
    keyless = not turn_id
    if keyless and stamped:
        # GH #1367: the turn comes through the apps fan-out before the session keeper, so
        # it carries no `turn_id`; its stamp carries the keeper's. A stamp of the same
        # round that still waits for its words takes these (both orders: `lane_verdict`).
        rec = unjoined(r, as_round(ctx.get("audience_set")), params, now, words=False)
        if rec is not None:
            return words_for(r, rec, text, params, now)
    if keyless:
        turn_id = "t-%d" % now
    if turn_id in r["pending"]:
        # The stamp came first (§ 9b.12): its window may stand, waiting for the words.
        return words_for(r, r["pending"][turn_id], text, params, now) if stamped else []
    # `seq` orders turns of the same millisecond by their arrival (P review): one more
    # than any pending turn's, so it survives a restart with the pending rows.
    seq = 1 + max([0] + [int(p.get("seq") or 0) for p in r["pending"].values()])
    rec = fresh_turn(turn_id, now, knob(params, "budget_ms"),
                     as_round(ctx.get("audience_set")), seq)
    # The request travels to the owning app after a sure verdict (it needs the words:
    # "the weather in <place>"); it never reaches the journal and leaves the pending row
    # when the turn is done.
    rec["text"] = text
    if stamped:
        rec["source"] = SOURCE_STAMP
        if keyless:
            rec["keyless"] = True
    r["pending"][turn_id] = rec
    gate = gate_round(rec["audience_set"], params.get("screen_audience"), params)
    rec["star_only"] = gate == "star"
    if gate == "foreign":
        # GH #1027: the round is checked BEFORE anything opens. Measured: a guest's turn
        # (`["agent:…", "person:guest"]`, the member not in it) asked the decider, and its
        # sure verdict opened `show-calendar` with the working hint on all three outputs;
        # the screen's round gated only the data. The open window alone told the screen
        # that someone asked about the member's calendar. A round with no member of the
        # screen's round in it -- or no round at all -- asks nothing, opens nothing, and
        # its journal row says so. A screen without a round sees `*` data only and keeps
        # the old way; a member's turn on a shared screen is `star` (R-HP-9).
        return [bundle(PHASE_WRITE, prune(r, now) + finish(rec, "foreign_round", now))] + \
            clock_ops(r, now)
    legs = prune(r, now) + w_pending(rec)
    if stamped:
        # No call: the verdict is the turn's stamp; `budget_ms` waits for it.
        return [bundle(PHASE_WRITE, legs)] + clock_ops(r, now)
    return [decide_call(text, topics, turn_id), bundle(PHASE_WRITE, legs)] + \
        clock_ops(r, now)


def close(rec, now):
    """The turn is done: the request text, the data sets and the bound blocks leave the
    pending row (OR-DP.P.4 -- the words are transient, review I1). The journal row is
    the one written before; nothing on the screen changes."""
    form_fallback(rec)
    rec["state"] = "done"
    rec["done_at"] = now
    rec["sets"] = {}
    rec["_nodes"] = []
    rec["text"] = ""
    rec["observed"] = {}
    return w_pending(rec)


def form_fallback(rec):
    """#1417: a stamp-way turn that ends (done, timeout or any fallback) without a screen
    form takes `FORM_FALLBACK`; the window's weight stays as it is. The selector way has
    no form and keeps its row."""
    if rec.get("source") == SOURCE_STAMP and rec.get("form") is None:
        rec["form"] = FORM_FALLBACK
        rec["form_fallback"] = True


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


def supersede(r, rec, now, close_older=True):
    """One window per topic (P § 5): a newer turn of the topic replaces the older one.
    Returns `(legs, stale)` -- the writes that close every older open turn of the topic,
    and whether `rec` itself is older than a turn that already opened the window (then
    it opens nothing). A closed turn's deadline and late data never reach the newer
    window (review I2); an older turn without a block journals `no_data`.
    `close_older=False` only asks `stale`: a turn that opens no window on its verdict
    (R-HP-9 (c)) replaces nothing until its first block opens one (`feed`)."""
    legs = []
    stale = False
    # Older/newer is (arrival ms, arrival order): two turns of one millisecond were
    # decided by whichever verdict came last (P review); now the later turn is newer.
    def age(x):
        return (x["arrived"], int(x.get("seq") or 0))
    for other in r["pending"].values():
        if other is rec or other.get("topic") != rec["topic"]:
            continue
        if age(other) > age(rec) and other.get("t_window") is not None:
            stale = True
        elif close_older and age(other) < age(rec) and other["state"] == "showing":
            legs += close(other, now) if other["placed"] else finish(other, "no_data", now)
    return legs, stale


def lane_decision(body, hop, params, now):
    r = ram()
    tid = str(hop.get("show_id") or "")
    failed = hop.get("finish_reason") == "error" or bool(hop.get("error_code"))
    if not tid:
        # `./decide` answered something that was no turn's call -- a params push it
        # refused without naming itself (not the registry's: that one leaves as
        # `model_refused`). It ended in the hive as `no_route` (P review); it leaves
        # here, out loud. Anything else without a turn is nobody's.
        if not failed:
            return []
        return [error("decide_refused", clip(
            "the decider refused a message that was no turn's call: %s: %s"
            % (str(hop.get("error_code") or "error"), text_of(body) or str(body.get("detail") or "")),
            300))]
    rec = r["pending"].get(tid)
    if rec is None:
        return []
    topics = all_topics(r, params)
    asked = set(questions(topics))
    if rec["state"] != "asking":
        if rec["state"] == "done" and rec["fallback"] == "timeout" and not rec["late"]:
            # A late verdict changes nothing on the screen; the journal says it came.
            rec["late"] = True
            if not failed:
                rec["missing"] = missing_of(body, asked)
                v = read_verdict(body, topics)
                rec["topic"], rec["p"], rec["model"] = v["topic"], v["p"], v["model"]
            return [bundle(PHASE_WRITE, w_pending(rec) + w_journal(rec))]
        return []
    rec["t_verdict"] = now - rec["arrived"]
    if failed:
        code = str(hop.get("error_code") or "")
        fb = "no_selector" if code == "decisions_unconfigured" else "error"
        return [bundle(PHASE_WRITE, finish(rec, fb, now))] + clock_ops(r, now)
    rec["missing"] = missing_of(body, asked)
    if topic_missing(body):
        # No topic answer, no topic: nothing opens (GH #977, OR-DP-84 b).
        return [bundle(PHASE_WRITE, finish(rec, "error", now))] + clock_ops(r, now)
    v = read_verdict(body, topics)
    rec["topic"], rec["p"], rec["model"] = v["topic"], v["p"], v["model"]
    rec["lead"], rec["also"] = v["lead"], v["also"] if v["also"] != NONE else None
    why = judge(v, topics, params)
    if why:
        return [bundle(PHASE_WRITE, finish(rec, why, now))] + clock_ops(r, now)
    # R-HP-9 (c): a turn that opens no window on its verdict pulls none either. An older
    # covered turn of the topic keeps its window until this turn's first `*` block
    # replaces it (`feed`); withdrawn at the private verdict, it would cut off the third
    # party's own window, and its leaving would tell the screen that a member's turn of
    # this topic was just judged (XB-M review I-2).
    return open_window(r, rec, v, topics, params, now)


def data_asks(rec, show, manifest, params):
    """What a sure verdict asks for its data: the owning app, or the declared sources of
    one of the presenter's own topics."""
    if show["owner"]:
        return [emission("in_show", {"messages": [], "op": "data", "topic": rec["topic"],
                                     "request": rec.get("text") or "",
                                     "lead": rec["keys"][0], "also": rec["keys"][1:],
                                     "turn_id": rec["turn_id"],
                                     "sets": wanted_sets(manifest, rec["keys"]),
                                     # OR-DP-82: the window's round rides with the question,
                                     # so an app that picks between rows of different rounds
                                     # picks only among rows this window may show (the
                                     # member's part of a shared screen, R-HP-18).
                                     "screen_audience": sorted(set(window_round(rec, params)))},
                         **app_head(show["owner"], show.get("show_at") or ""))]
    # The presenter's own topic (GH #963): no app to ask. A declared source is read
    # now; an observed topic takes what was observed so far, and every later
    # observation of its kind is fed on arrival (`refill`).
    return source_reads(rec, manifest, params)


def open_window(r, rec, v, topics, params, now):
    """A sure verdict opens the topic's window and asks for its data -- the selector's
    verdict (§ 9b.2) and the stamp's (§ 9b.12) alike."""
    stamped = rec.get("source") == SOURCE_STAMP
    tid = rec["turn_id"]
    older, stale = supersede(r, rec, now, close_older=not rec.get("star_only"))
    if stale:
        return [bundle(PHASE_WRITE, older + finish(rec, "no_data", now))] + clock_ops(r, now)
    show = topics[v["topic"]]
    manifest = show["manifest"]
    rec["keys"] = chosen(v, manifest, params)
    rec["state"] = "showing"
    rec["owner"] = show["owner"]
    wait = manifest.get("data_wait_ms") or knob(params, "data_wait_ms")
    rec["data_deadline"] = now + int(wait)
    if not rec.get("star_only"):
        rec["t_window"] = now - rec["arrived"]
        if stamped and rec.get("t_building") is None:
            rec["t_building"] = rec["t_window"]
    if stamped:
        tree = stamp_window(rec, manifest, params, now)
        # The words may not be there yet (the stamp came first, § 9b.12): the data are
        # asked for once they are (`words_for`). An observed topic needs no words.
        wants_words = bool(show["owner"]) or show.get("kind") == "source"
        ask = data_asks(rec, show, manifest, params) if rec.get("text") or not wants_words \
            else []
        rec["asked"] = bool(ask) or not wants_words
    else:
        tree = window(v["topic"], manifest, tid, [], knob(params, "work_hint"), now)
        ask = data_asks(rec, show, manifest, params)
    # R-HP-9 (c): a shared screen gets no window on the verdict -- only `*` data open
    # one (`feed`).
    view = [a_view(v["topic"], tree)] if not rec.get("star_only") else []
    out = view + ask + [bundle(PHASE_WRITE, older + w_pending(rec))]
    if not show["owner"]:
        sets = own_sets(r, rec, v["topic"], params) if show.get("kind") == "observed" else {}
        if sets or rec["sets"]:
            out += feed(r, rec, manifest, sets, params, now)
    return out + clock_ops(r, now)


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
    if not sender or sender != rec["owner"]:
        # The edge stamps `show_app` (OR-DP-54); only the topic's owner fills its window
        # (review I3). Said out loud, never placed.
        return [error("foreign_data", "show_data for topic %s from %r; its owner is %s"
                      % (rec["topic"], sender, rec["owner"]))]
    sets = body.get("sets") if isinstance(body.get("sets"), dict) else {}
    return feed(r, rec, show["manifest"], sets, params, now)


def feed(r, rec, manifest, sets, params, now):
    """Data sets for a shown turn, from its app (`show_data`), a resident (`resident_answer`)
    or the presenter's own observations: gated by the window's round (`window_round`),
    then every block that can stand now is placed (P.6), the first one journaled."""
    if rec.get("source") == SOURCE_STAMP:
        return feed_stamp(r, rec, manifest, sets, params, now)
    tid = rec["turn_id"]
    screen = window_round(rec, params)
    for name, data in sets.items():
        kept = filter_set(data, screen)
        if kept is not None and name not in rec["sets"]:
            rec["sets"][name] = kept
        # A set the screen may not see -- its own round does not cover the screen, or no
        # row and no `value` of it may be shown -- is not kept at all (GH #966, N review
        # I-1 and its side note): stored as empty, its lead fell to the standard at once,
        # while a set that never came waits for the clock and is withdrawn -- the
        # difference would tell the screen that a set or rows of other rounds exist. Not
        # kept, it is the set that never came, in every step that follows.
    new, fallback = resolve(rec, manifest, catalogue(params))
    if not new:
        if rec["placed"] and settled(rec, manifest):
            return [bundle(PHASE_WRITE, close(rec, now))] + clock_ops(r, now)
        return [bundle(PHASE_WRITE, w_pending(rec))]
    opens = rec["t_window"] is None
    older = []
    if opens:
        # R-HP-9 (c): the first `*` block opens the window, so only now does this turn
        # replace an older turn of the topic (one window per topic); a newer turn that
        # opened it meanwhile keeps it, and this one ends with nothing on the screen.
        older, stale = supersede(r, rec, now)
        if stale:
            return [bundle(PHASE_WRITE, finish(rec, "no_data", now))] + clock_ops(r, now)
    first = not rec["placed"]
    rec["placed"] += [k for k, _ in new]
    blocks = [node for _, node in rec.get("_nodes", [])] + [n for _, n in new]
    rec["_nodes"] = rec.get("_nodes", []) + new
    if opens:
        rec["t_window"] = now - rec["arrived"]
    tree = window(rec["topic"], manifest, tid, blocks, "", now if opens else None)
    out = [a_view(rec["topic"], tree)]
    legs = older
    if first:
        rec["t_content"] = now - rec["arrived"]
        rec["fallback"] = fallback or "none"
        legs += w_journal(rec)
    # Every chosen block placed or final: the turn closes now, not at data_wait_ms.
    legs += close(rec, now) if settled(rec, manifest) else w_pending(rec)
    out.append(bundle(PHASE_WRITE, legs))
    return out + clock_ops(r, now)


def lane_tick(hop, now, params=None):
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
            if rec["placed"] and rec.get("source") == SOURCE_STAMP:
                # § 9b.14: the deadline with a block standing is done as well.
                out.append(a_view(rec["topic"], stamp_done(rec, now, params or {})))
                legs += w_journal(rec) + close(rec, now)
            elif rec["placed"]:
                # The blocks stay; the turn closes and its words leave (review I1).
                legs += close(rec, now)
            else:
                if rec.get("t_window") is not None:
                    out.append(withdraw(rec["topic"]))
                legs += finish(rec, "no_data", now)
    if legs:
        out.append(bundle(PHASE_WRITE, legs))
    return out + clock_ops(r, now, struck)


# ---------------------------------------------------------------------------
# live-2 GR (#1357): the verdict from the turn's stamp (display-hive.md § 9b.12-9b.14)
#
# With `verdict_source: stamp` the talky judged the turn once -- topic, candidates and the
# screen form in its stamp (FU2) -- and the presenter asks nobody. A sure verdict opens the
# window UNDER CONSTRUCTION at once (R-FU-1): the pictogram, then one skeleton per chosen
# candidate from its manifest; blocks take their skeleton's place as their data come, and
# until the window is done a set that comes again rebuilds what it binds. Done -- the lead
# stands and nothing chosen waits -- takes the pictogram away and lets the frame settle
# once; from there the window is frozen (§ 1.6).


def verdict_source(params):
    """`stamp` only when the installation says so; anything else is the selector."""
    v = params.get("verdict_source")
    return SOURCE_STAMP if isinstance(v, str) and v.strip().lower() == SOURCE_STAMP \
        else SOURCE_SELECTOR


def form_rung(form, params):
    """The rung a screen form asks for: the fixed map (R-FU-4), where `form_rungs` may
    only say `focus`, `relevant` or `none` for a known form -- never `urgent`."""
    over = params.get("form_rungs")
    if isinstance(over, str):
        try:
            over = json.loads(over)
        except ValueError:
            over = None
    rung = over.get(form) if isinstance(over, dict) else None
    if rung in ("focus", "relevant", "none"):
        return rung
    return FORM_RUNGS.get(form, "relevant")


def stamp_of(body):
    """The stamp as the talky sends it (`{stamp: {...}}`), or the body itself."""
    st = body.get("stamp")
    return st if isinstance(st, dict) else body


def stamp_verdict(screen, known):
    """The stamp's screen verdict in the selector's shape (`read_verdict`): a topic the
    presenter does not know, or a candidate its topic does not carry, is no choice. The
    stamp gives one confidence; `further` counts against `also_threshold` with it."""
    v = {"topic": None, "p": 0.0, "lead": None, "lead_p": 0.0, "also": None, "also_p": 0.0,
         "model": SOURCE_STAMP}
    topic = screen.get("topic") if isinstance(screen.get("topic"), str) else None
    p = screen.get("confidence")
    p = float(p) if isinstance(p, (int, float)) and not isinstance(p, bool) else 0.0
    v["p"] = p
    if topic == NONE:
        v["topic"] = NONE
        return v
    if topic not in known:
        return v
    v["topic"] = topic
    keys = [c["key"] for c in known[topic]["manifest"]["candidates"]]
    lead, also = screen.get("lead"), screen.get("further")
    v["lead"] = lead if lead in keys else None
    v["lead_p"] = p
    v["also"] = also if also in keys else None
    v["also_p"] = p
    return v


def skeleton(topic, cand):
    """The rough form of one candidate (§ 2 *Skeleton*): its block kind, its head -- the
    literal `title` of its binding, else empty -- and its structure as a count of lines:
    a list of rows reads as SKELETON_LINES, a block of bound props as one line each.
    Never a data value: nothing here is read from a set."""
    bind = scalar_bind(cand.get("bind"))
    title = bind.get("title")
    head = title[1:] if isinstance(title, str) and title.startswith("=") else ""
    if children_of(cand):
        lines = SKELETON_LINES
    else:
        lines = max(1, min(SKELETON_LINES + 1,
                           len([s for s in bind.values()
                                if isinstance(s, str) and not s.startswith("=")])))
    return {"component": SKELETON, "key": "show-%s-%s-skeleton" % (topic, cand["key"]),
            "props": {"block": cand["block"], "title": head, "lines": lines}}


def stamp_window(rec, manifest, params, touched):
    """The window of the stamp way: under construction (the pictogram, then per chosen
    candidate its block or its skeleton) or, once done, the blocks alone with the stack
    saying `build: done` and `settle_ms`."""
    topic = rec["topic"]
    vid = "show-" + topic
    cands = {c["key"]: c for c in manifest["candidates"]}
    nodes = {k: n for k, n in rec.get("_nodes") or []}
    done = rec.get("built") == "done"
    inner = [] if done else [{"component": STATUS, "key": vid + "-" + BUILDING,
                              "props": {"kind": BUILDING}}]
    for i, k in enumerate(rec["keys"]):
        node = nodes.get(manifest["standard"]) if i == 0 and rec["first"] == "standard" \
            else nodes.get(k)
        if node is not None:
            inner.append(node)
        elif not done and cands.get(k) is not None:
            inner.append(skeleton(topic, cands[k]))
    stack = {"build": "done", "settle_ms": knob(params, "settle_ms")} if done \
        else {"build": BUILDING}
    return window(topic, manifest, rec["turn_id"], [], "", touched,
                  relevance=rec.get("relevance") or RELEVANCE, inner=inner, stack=stack)


def stamp_done(rec, now, params):
    """Mark the window done (§ 9b.14) and return its last tree."""
    form_fallback(rec)
    rec["built"] = "done"
    rec["t_done"] = now - rec["arrived"]
    show_manifest = rec.get("_manifest") or {}
    return stamp_window(rec, show_manifest, params, None)


def words_for(r, rec, text, params, now):
    """The turn's words after its stamp: kept for the data request, which leaves now when
    the window already stands and waited for them."""
    if rec["state"] == "done" or rec.get("text"):
        return []
    rec["text"] = text
    out = []
    if rec["state"] == "showing" and not rec.get("asked"):
        show = all_topics(r, params).get(rec["topic"])
        if show is not None:
            out = data_asks(rec, show, show["manifest"], params)
            rec["asked"] = True
    return out + [bundle(PHASE_WRITE, w_pending(rec))]


def stamp_round(st, ctx):
    """The round of a stamp: the stamp's own, else the edge's."""
    rnd = st.get("audience_set") if st.get("audience_set") is not None \
        else ctx.get("audience_set")
    return as_round(rnd)


def same_round(a, b, params):
    """True when two records may be one turn: the same round in canonical spelling. A
    round on one side only is no match (fail-closed); no round on either side matches only
    on a screen without a round, which shows `*` data alone anyway."""
    ka, kb = round_key(a), round_key(b)
    if ka is None or kb is None:
        return ka is None and kb is None and not as_round(params.get("screen_audience"))
    return ka == kb


def unjoined(r, rnd, params, now, words):
    """GH #1367 -- the newest open record of the stamp way and of round `rnd`, younger than
    `budget_ms`, that waits for its other half: `words=True` a turn that came without a
    `turn_id` and has no stamp yet, `words=False` a stamp still without words. Measured in a
    lab colony (stamp mode): the turn via the apps fan-out carried no `turn_id` (record
    `t-<now>`), the stamp the keeper's (`keeper:<trace>:<ts>`); the ids never met, no
    `in_show op: data` left, the window was withdrawn at `data_wait_ms`. Newest by arrival
    order, as `turn_for` picks the turn of a call; never across rounds."""
    budget = knob(params, "budget_ms")
    best = None
    for rec in r["pending"].values():
        if rec.get("source") != SOURCE_STAMP or rec["state"] == "done" \
                or now - rec["arrived"] > budget \
                or not same_round(rec.get("audience_set"), rnd, params):
            continue
        if words:
            fits = rec.get("keyless") and rec["state"] == "asking" and bool(rec.get("text"))
        else:
            fits = not rec.get("keyless") and not rec.get("text")
        if fits and (best is None or (rec["arrived"], int(rec.get("seq") or 0)) >
                     (best["arrived"], int(best.get("seq") or 0))):
            best = rec
    return best


def rekey(r, rec, tid):
    """Move an unkeyed turn's record to its stamp's `turn_id`; the old pending row leaves
    (a restart must not find it still asking)."""
    old = rec["turn_id"]
    del r["pending"][old]
    rec.pop("keyless", None)
    rec["turn_id"] = tid
    r["pending"][tid] = rec
    return w_pending_gone(old)


def stamp_turn(r, tid, st, ctx, params, now):
    """A record for a stamp whose turn has not arrived (§ 9b.12): the round is the
    stamp's, else the edge's; a round foreign to the screen opens nothing (GH #1027)."""
    seq = 1 + max([0] + [int(p.get("seq") or 0) for p in r["pending"].values()])
    rec = fresh_turn(tid, now, knob(params, "budget_ms"), stamp_round(st, ctx), seq)
    rec["text"] = ""
    rec["source"] = SOURCE_STAMP
    r["pending"][tid] = rec
    gate = gate_round(rec["audience_set"], params.get("screen_audience"), params)
    rec["star_only"] = gate == "star"
    return rec, gate == "foreign"


def apply_form(r, rec, form, params, now):
    """A screen form for a turn (§ 9b.13): noted while the verdict is out; on a window
    under construction `active` lifts it, `none` withdraws it; on a done window nothing."""
    if rec.get("form") is None:
        rec["t_form"] = now - rec["arrived"]
    rec["form"] = form
    if rec["state"] != "showing" or rec.get("built") == "done":
        return []
    rung = form_rung(form, params)
    if rung == "none":
        out = [withdraw(rec["topic"])] if rec.get("t_window") is not None else []
        return out + [bundle(PHASE_WRITE, finish(rec, "form_none", now))] + clock_ops(r, now)
    relevance = RUNG_RELEVANCE[rung]
    if relevance == (rec.get("relevance") or RELEVANCE):
        # `relevant` keeps the window as it stands: no write, so no touch (§ 4.8).
        return [bundle(PHASE_WRITE, w_pending(rec))]
    rec["relevance"] = relevance
    out = []
    if rec.get("t_window") is not None:
        # A change of an own prop is an app touch (§ 4.8 b): the lift is meant to be seen.
        out.append(a_view(rec["topic"], stamp_window(rec, rec["_manifest"], params, None)))
    return out + [bundle(PHASE_WRITE, w_pending(rec))]


def stamp_open(r, rec, v, topics, params, now):
    """Judge a stamp's verdict and open its window under construction."""
    why = judge(v, topics, params)
    if why:
        return [bundle(PHASE_WRITE, finish(rec, why, now))] + clock_ops(r, now)
    rec["topic"], rec["p"], rec["model"] = v["topic"], v["p"], v["model"]
    rec["lead"], rec["also"] = v["lead"], v["also"]
    rec["_manifest"] = topics[v["topic"]]["manifest"]
    if rec.get("form") is not None:
        rung = form_rung(rec["form"], params)
        if rung == "none":
            # A sure topic on a screen form `none`: no window, however sure (R-FU-4) --
            # the form of this stamp or one kept from an earlier stamp of the turn (a
            # form-only `none` before the verdict opened a window at 0.8, review K-G Minor 1).
            return [bundle(PHASE_WRITE, finish(rec, "form_none", now))] + clock_ops(r, now)
        rec["relevance"] = RUNG_RELEVANCE.get(rung, RELEVANCE)
    return open_window(r, rec, v, topics, params, now)


def restamp(r, rec, v, topics, params, now):
    """A later stamp of the same turn while its window is under construction (§ 9b.12):
    another topic withdraws the window and opens the new one; the same topic with other
    candidates rebuilds its skeleton and asks for their data."""
    if v["topic"] != rec["topic"]:
        out = [withdraw(rec["topic"])] if rec.get("t_window") is not None else []
        rec.update({"topic": None, "keys": [], "placed": [], "_nodes": [], "sets": {},
                    "first": "open", "state": "asking", "owner": None, "data_deadline": None,
                    "t_window": None, "asked": False})
        return out + stamp_open(r, rec, v, topics, params, now)
    manifest = rec["_manifest"]
    keys = chosen(v, manifest, params)
    if keys == rec["keys"]:
        return []
    rec["lead"], rec["also"], rec["p"] = v["lead"], v["also"], v["p"]
    rec.update({"keys": keys, "placed": [], "_nodes": [], "first": "open"})
    show = topics[rec["topic"]]
    ask = data_asks(rec, show, manifest, params) if rec.get("text") else []
    out = feed_stamp(r, rec, manifest, {}, params, now)
    if not any(e["header"]["route"] == "view" for e in out):
        out = [a_view(rec["topic"], stamp_window(rec, manifest, params, None))] + out
    return ask + out


def lane_verdict(body, hop, ctx, params, now):
    """The talky's stamp of a turn (`in_verdict`, § 9b.12): `screen` is the verdict,
    `screen_form` the form (the first stamp may carry both, a second one the form alone);
    a stamp with neither carries no screen verdict -- a `clarify` turn -- and opens
    nothing. Read from the message alone, never from another cell's state."""
    r = ram()
    st = stamp_of(body)
    tid = str(st.get("turn_id") or hop.get("turn_id") or "")
    topics = all_topics(r, params)
    if not tid or not topics:
        return []
    screen = st.get("screen") if isinstance(st.get("screen"), dict) else None
    form = st.get("screen_form") if st.get("screen_form") in FORMS else None
    rec = r["pending"].get(tid)
    moved = []
    if rec is None:
        # GH #1367: the turn may have come first without a `turn_id` (the apps fan-out);
        # the newest such turn of the stamp's round becomes this stamp's record.
        rec = unjoined(r, stamp_round(st, ctx), params, now, words=True)
        if rec is not None:
            moved = [bundle(PHASE_WRITE, rekey(r, rec, tid))]
    if rec is None:
        rec, foreign = stamp_turn(r, tid, st, ctx, params, now)
        if foreign:
            return [bundle(PHASE_WRITE, prune(r, now) + finish(rec, "foreign_round", now))] + \
                clock_ops(r, now)
    return moved + stamp_on(r, rec, screen, form, topics, params, now)


def stamp_on(r, rec, screen, form, topics, params, now):
    """A stamp applied to its record (`lane_verdict`)."""
    if rec["state"] == "done":
        noted = False
        if rec["fallback"] == "timeout" and not rec["late"] and screen is not None:
            # A late verdict changes nothing on the screen; the journal says it came.
            v = stamp_verdict(screen, topics)
            rec["late"] = True
            rec["topic"], rec["p"], rec["model"] = v["topic"], v["p"], v["model"]
            noted = True
        if form is not None and rec.get("form_fallback") and not rec.get("form_late"):
            # #1417: the judge's form after the fallback -- journaled, once; the window
            # stays as the fallback left it.
            rec["form_late"] = True
            noted = True
        return [bundle(PHASE_WRITE, w_pending(rec) + w_journal(rec))] if noted else []
    if rec["state"] == "showing":
        out = restamp(r, rec, stamp_verdict(screen, topics), topics, params, now) \
            if screen is not None else []
        if form is not None and rec["state"] == "showing":
            out += apply_form(r, rec, form, params, now)
        return out
    # Asking: the verdict is out until this stamp.
    if form is not None:
        if rec.get("form") is None:
            rec["t_form"] = now - rec["arrived"]
        rec["form"] = form
    if screen is None:
        if form is not None:
            return [bundle(PHASE_WRITE, w_pending(rec))]
        rec["t_verdict"] = now - rec["arrived"]
        return [bundle(PHASE_WRITE, finish(rec, "no_topic", now))] + clock_ops(r, now)
    rec["t_verdict"] = now - rec["arrived"]
    return stamp_open(r, rec, stamp_verdict(screen, topics), topics, params, now)


def feed_stamp(r, rec, manifest, sets, params, now):
    """`feed` of the stamp way: a set that comes again replaces the kept one and rebuilds
    the blocks it binds while the window is not done (§ 9b.5); a block takes its
    skeleton's place; done (§ 9b.14) writes the last tree, the journal and the close."""
    if rec.get("built") == "done" or rec["state"] != "showing":
        return []
    rec["_manifest"] = manifest
    screen = window_round(rec, params)
    changed = set()
    for name, data in sets.items():
        kept = filter_set(data, screen)
        if kept is None:
            continue
        if name in rec["sets"] and rec["sets"][name] != kept:
            changed.add(name)
        rec["sets"][name] = kept
    cat = catalogue(params)
    cands = {c["key"]: c for c in manifest["candidates"]}
    rebuilt = False
    nodes = [list(kn) for kn in rec.get("_nodes") or []]
    for kn in nodes:
        c = cands.get(kn[0])
        if c is not None and c["set"] in changed:
            node = place(rec["topic"], c, rec["sets"], cat)
            if node is not None and node != kn[1]:
                kn[1] = node
                rebuilt = True
    rec["_nodes"] = nodes
    new, fallback = resolve(rec, manifest, cat)
    if not new and not rebuilt:
        if rec["placed"] and settled(rec, manifest):
            tree = stamp_done(rec, now, params)
            return [a_view(rec["topic"], tree),
                    bundle(PHASE_WRITE, w_journal(rec) + close(rec, now))] + clock_ops(r, now)
        return [bundle(PHASE_WRITE, w_pending(rec))]
    opens = rec["t_window"] is None
    older = []
    if opens:
        # R-HP-9 (c), as in `feed`: the first `*` block opens a shared screen's window.
        older, stale = supersede(r, rec, now)
        if stale:
            return [bundle(PHASE_WRITE, finish(rec, "no_data", now))] + clock_ops(r, now)
    first = bool(new) and not rec["placed"]
    rec["placed"] += [k for k, _ in new]
    rec["_nodes"] = rec["_nodes"] + [[k, n] for k, n in new]
    if opens:
        rec["t_window"] = now - rec["arrived"]
        if rec.get("t_building") is None:
            rec["t_building"] = rec["t_window"]
    legs = older
    if first:
        rec["t_content"] = now - rec["arrived"]
        rec["fallback"] = fallback or "none"
    if rec["placed"] and settled(rec, manifest):
        tree = stamp_done(rec, now, params)
        legs += w_journal(rec) + close(rec, now)
    else:
        tree = stamp_window(rec, manifest, params, now if opens else None)
        legs += (w_journal(rec) if first else []) + w_pending(rec)
    return [a_view(rec["topic"], tree), bundle(PHASE_WRITE, legs)] + clock_ops(r, now)


# ---------------------------------------------------------------------------
# GH #963: observed tool calls and results are DATA (R-29-6, OR-DP-30)
#
# A tool call or result never opens a window and never asks the decider: only a sure
# verdict over a turn does. The presenter keeps what it observes as rows of `work` (one
# per call, its state filled in by the result) and, for a web search, as the hits of the
# turn that caused it. Whether the window `search` or `work` opens is the verdict's alone.


def round_key(value):
    """A round in one canonical spelling, or None where there is none."""
    r = as_round(value)
    return json.dumps(sorted(r)) if r else None


def meet(rounds):
    """The intersection of rounds, for a value derived from several rows (OR-DP-10): `*`
    stands back for any concrete round; no common member is the empty round (shows
    nothing)."""
    out = None
    for r in rounds:
        r = as_round(r) or []
        if out is None:
            out = list(r)
        elif "*" in out:
            out = list(r)
        elif "*" not in r:
            out = [m for m in out if m in r]
    return sorted(set(out or []))


def turn_text(body, kind):
    """The id and text of the one `tool_call` / `tool_result` turn of a body."""
    for m in body.get("messages") or []:
        if isinstance(m, dict) and m.get("type") == kind:
            return str(m.get("id") or ""), str(m.get("text") or "")
    return "", ""


def call_args(body):
    _, text = turn_text(body, "tool_call")
    try:
        args = json.loads(text)
    except (TypeError, ValueError):
        args = None
    if not isinstance(args, dict):
        args = body.get("arguments") if isinstance(body.get("arguments"), dict) else {}
    return args


def is_test(command, patterns):
    low = str(command or "").lower()
    return any(isinstance(p, str) and p and p.lower() in low for p in patterns or [])


def test_patterns(params):
    p = params.get("test_patterns")
    return p if isinstance(p, list) and p else DEFAULT_TEST_PATTERNS


def clip(text, n):
    text = " ".join(str(text or "").split())
    return text if len(text) <= n else text[:n - 1] + "…"


def host_of(url):
    try:
        return (urlsplit(url).hostname or "")
    except ValueError:
        return ""


def safe_link(url):
    """A link only where it is http(s) (#868); anything else is no link at all."""
    url = str(url or "").strip()
    return url if url.lower().startswith(("https://", "http://")) and host_of(url) else ""


def summarise(tool, args, patterns):
    """`(summary, path)` of one call: what it did, never what it read or printed. A shell
    command is named only as a test run or a shell command -- its line can carry a secret,
    and its output is never kept."""
    if tool == "web_search":
        return clip("search: %s" % (args.get("query") or ""), SUMMARY_CHARS), ""
    if tool == "web_fetch":
        return clip("fetch %s" % host_of(safe_link(args.get("url"))), SUMMARY_CHARS), ""
    if tool == "bash":
        return ("test run" if is_test(args.get("command"), patterns) else "shell command"), ""
    path = clip(args.get("path") or args.get("file") or "", SUMMARY_CHARS)
    if tool == "edit":
        return clip("edit %s" % path, SUMMARY_CHARS), path
    return clip("%s %s" % (args.get("op") or "file", path), SUMMARY_CHARS), path


def turn_for(r, hop, ctx, key):
    """The open turn a call belongs to: the one its `turn_id` names, else the newest open
    turn of the same round (the order of arrival, never a time window -- OR-DP.Q.2). A
    call without a round belongs to no turn."""
    tid = str(hop.get("turn_id") or ctx.get("turn_id") or "")
    rec = r["pending"].get(tid)
    if rec is not None and rec["state"] in ("asking", "showing"):
        return tid
    if key is None:
        return ""
    best = None
    for rec in r["pending"].values():
        if rec["state"] in ("asking", "showing") and round_key(rec.get("audience_set")) == key \
                and (best is None or rec["arrived"] > best["arrived"]):
            best = rec
    return best["turn_id"] if best else ""


def w_work_row(row):
    return [tool_call({"operation": "insert", "table": T_WORK, "row": row,
                       "replace": {"call_id": row["call_id"]}},
                      "w-ins-" + row["call_id"])]


def parse_hits(text, round_):
    """The hits of one `web_search` result (`[{title, url, snippet}]` or `{results: [...]}`),
    each stamped with the round of the result. Links only http(s)."""
    try:
        data = json.loads(text)
    except (TypeError, ValueError):
        return []
    if isinstance(data, dict):
        data = data.get("results")
    out = []
    for h in data if isinstance(data, list) else []:
        if not isinstance(h, dict):
            continue
        url = safe_link(h.get("url"))
        title = clip(h.get("title") or "", SUMMARY_CHARS)
        if not title and not url:
            continue
        out.append({"title": title or host_of(url), "url": url, "host": host_of(url),
                    "snippet": clip(h.get("snippet") or "", SNIPPET_CHARS),
                    "audience_set": as_round(round_) or []})
    return out


def search_sets(hits, screen):
    """The sets of `search`: the hits the screen may see, the first as `top`, the sites as
    one table value. The values are derived from the visible hits only and carry the
    intersection of their rounds (OR-DP-10)."""
    seen = [h for h in hits if covers(h.get("audience_set"), screen)][:SEARCH_HITS]
    if not seen:
        return {}
    round_ = meet(h["audience_set"] for h in seen)
    counts = {}
    for h in seen:
        if h["host"]:
            counts[h["host"]] = counts.get(h["host"], 0) + 1
    rows = "".join("<tr><td>%s</td><td>%d</td></tr>" % (html.escape(k), n)
                   for k, n in sorted(counts.items(), key=lambda kv: (-kv[1], kv[0])))
    top = seen[0]
    return {"hits": {"rows": seen, "audience_set": round_},
            "top": {"value": {"title": top["title"], "host": top["host"],
                              "snippet": top["snippet"]}, "audience_set": round_},
            "sources": {"value": {"caption": "Sources",
                                  "head": "<tr><th>Site</th><th>Hits</th></tr>",
                                  "rows": rows} if rows else {}, "audience_set": round_}}


def session_of(rows, rec):
    """The session a `work` window of turn `rec` shows: the one its own rows belong to,
    else the newest session in the turn's round -- never another conversation's work
    (review Q M-4). Without a turn (a table), the newest session at all."""
    if rec is None:
        pool = rows
    else:
        pool = [w for w in rows if w.get("turn_id") == rec.get("turn_id")] or \
            [w for w in rows if w.get("audience_set")
             and w.get("audience_set") == round_key(rec.get("audience_set"))]
    if not pool:
        return None
    return max(pool, key=lambda w: int(w.get("seq") or 0)).get("session")


def work_sets(rows, screen, params, rec=None):
    """The sets of `work`: the steps of the session of the turn (`session_of`) as far as
    the screen may see them, the files it touched, and the verdict of its last test run.
    Only states and names -- never a file's content, never a command's output."""
    seen = [w for w in rows if covers(w.get("audience_set"), screen)]
    session = session_of(seen, rec)
    if session is None:
        return {}
    mine = sorted((w for w in seen if w.get("session") == session),
                  key=lambda w: int(w.get("seq") or 0))
    round_ = meet(w["audience_set"] for w in mine)
    steps = [{"label": w["summary"] or w["tool"], "state": w["state"],
              "detail": w["tool"] if w.get("exit_code") in (None, "") else
              "%s, exit %s" % (w["tool"], w["exit_code"])}
             for w in mine[-WORK_SHOWN:]]
    files = []
    for w in mine:
        if w.get("path") and w["path"] not in [f["path"] for f in files]:
            files.append({"path": w["path"]})
    tests = [w for w in mine if w["tool"] == "bash" and w["summary"] == "test run"
             and w["state"] in ("done", "failed")]
    value = {}
    if tests:
        last = tests[-1]
        value = {"verdict": "passed" if last["state"] == "done" else "failed",
                 "detail": "%d test run%s, the last exit %s" % (
                     len(tests), "" if len(tests) == 1 else "s", last.get("exit_code"))}
    return {"steps": {"rows": steps, "audience_set": round_},
            "files": {"rows": files, "audience_set": round_},
            "tests": {"value": value, "audience_set": round_}}


def observed_on(params):
    names = params.get("observed_topics")
    return isinstance(names, list) and any(n in BUILTIN for n in names)


def lane_tool_call(body, hop, ctx, params, now):
    """An observed call (`in_tool_call`, the builder's tap): one `work` row, `running`."""
    r = ram()
    tool = str(hop.get("tool_name") or "")
    cid = str(hop.get("tool_call_id") or "") or turn_text(body, "tool_call")[0]
    if tool not in OBSERVED_TOOLS or not cid or not observed_on(params):
        return []
    key = round_key(ctx.get("audience_set"))
    summary, path = summarise(tool, call_args(body), test_patterns(params))
    r["seq"] = int(r.get("seq") or 0) + 1
    row = {"seq": r["seq"], "session": str(ctx.get("session_id") or key or ""),
           "turn_id": turn_for(r, hop, ctx, key), "tool": tool, "call_id": cid,
           "state": "running", "summary": summary, "path": path, "exit_code": None,
           "audience_set": key or "", "at": now}
    r["work"] = [w for w in r["work"] if w["call_id"] != cid] + [row]
    legs = w_work_row(row)
    mine = [w for w in r["work"] if w["session"] == row["session"]]
    for old in sorted(mine, key=lambda w: w["seq"])[:max(0, len(mine) - WORK_ROWS)]:
        r["work"].remove(old)
        legs.append(tool_call({"operation": "delete", "table": T_WORK,
                               "where": {"seq": old["seq"]}}, "w-cap-%d" % old["seq"]))
    return [bundle(PHASE_WRITE, legs)] + refill(r, "work", params, now)


def lane_tool_result(body, hop, ctx, params, now):
    """An observed result (`tool_result`): its call's row gets its state; a web search
    gives its turn hits. A result whose call this presenter did not observe is not ours."""
    r = ram()
    cid, text = turn_text(body, "tool_result")
    row = next((w for w in r["work"] if w["call_id"] == cid), None) if cid else None
    if row is None or row["state"] != "running":
        return []
    failed = bool(hop.get("error_code")) or hop.get("finish_reason") == "error"
    code = hop.get("exit_code")
    if isinstance(code, bool) or not isinstance(code, int):
        code = None
    row["exit_code"] = code
    row["state"] = "failed" if failed or (code is not None and code != 0) else "done"
    legs = w_work_row(row)
    out = []
    rec = r["pending"].get(row["turn_id"])
    if row["tool"] == "web_search" and not failed and rec is not None \
            and rec["state"] in ("asking", "showing"):
        # The round of the hits is the round the RESULT carries (the generation's own
        # producers stamp it, OR-DP.Q.3); without one, the hits show nowhere (fail-closed).
        hits = parse_hits(text, ctx.get("audience_set"))
        obs = rec.setdefault("observed", {})
        obs["hits"] = (obs.get("hits") or []) + hits
        legs += w_pending(rec)
    out.append(bundle(PHASE_WRITE, legs))
    return out + refill(r, "search", params, now) + refill(r, "work", params, now)


def own_sets(r, rec, topic, params):
    if topic == "search":
        return search_sets((rec.get("observed") or {}).get("hits") or [],
                           window_round(rec, params))
    if topic == "work":
        return work_sets(r["work"], window_round(rec, params), params, rec)
    return {}


def refill(r, topic, params, now):
    """Feed every open window of an observed topic whose first block is still missing."""
    out = []
    topics = all_topics(r, params)
    show = topics.get(topic)
    if show is None or show.get("kind") != "observed":
        return out
    for rec in sorted(r["pending"].values(), key=lambda x: x["arrived"]):
        if rec["state"] == "showing" and rec["topic"] == topic and rec["first"] == "open":
            sets = own_sets(r, rec, topic, params)
            if sets:
                out += feed(r, rec, show["manifest"], sets, params, now)
    return out


# ---------------------------------------------------------------------------
# GH #963: declared sources (Q.4) -- read after the verdict, in the screen's round


def fill(value, text):
    """`value` with every string that is exactly `$request` replaced by the turn's text
    (equality only, never a part of a string)."""
    if value == REQUEST:
        return text
    if isinstance(value, dict):
        return {k: fill(v, text) for k, v in value.items()}
    if isinstance(value, list):
        return [fill(v, text) for v in value]
    return value


def at_path(body, path):
    """The part of an answer a dotted path names: digits index a list, a string met in the
    middle of the path is read as JSON first, `""` is the whole body. None where it is not."""
    cur = body
    if path == "":
        return cur
    for part in path.split("."):
        if isinstance(cur, str):
            try:
                cur = json.loads(cur)
            except ValueError:
                return None
        if isinstance(cur, list) and part.isdigit():
            i = int(part)
            cur = cur[i] if i < len(cur) else None
        elif isinstance(cur, dict):
            cur = cur.get(part)
        else:
            return None
        if cur is None:
            return None
    return cur


def source_of(manifest, name):
    return next((c["source"] for c in manifest["candidates"]
                 if c["set"] == name and isinstance(c.get("source"), dict)), None)


def source_reads(rec, manifest, params):
    """The reads of a sure verdict on a topic with declared sources: one `resident_read`
    per wanted set that names one, `op_id` = `<turn_id>/<set>`. The round is not the
    presenter's to say: the builder's edge stamps the member's round on the way out, and
    the answer's `resident_round` is the set's only round (OR-DP.M.1)."""
    out = []
    text = rec.get("text") or ""
    for name in wanted_sets(manifest, rec["keys"]):
        src = source_of(manifest, name)
        if src is None:
            continue
        body = fill(dict(src.get("body") or {}), text)
        body.setdefault("messages", [])
        head = fill(dict(src.get("hop") or {}), text)
        head.update({"resident": src["read"], "op_id": "%s/%s" % (rec["turn_id"], name)})
        out.append(emission("resident_read", body, **head))
    return out


def lane_resident_answer(body, hop, ctx, params, now):
    """A resident answered (`resident_answer`, restamped by the builder's edge): the paths of
    the source pick rows and value; the set's round is `hop.resident_round` alone, never
    the body's -- without it the set shows nothing. A refusal or an error is a set without
    data, so the standard or the clock decides."""
    r = ram()
    tid, _, name = str(hop.get("op_id") or "").rpartition("/")
    rec = r["pending"].get(tid)
    if rec is None or rec["state"] != "showing" or rec.get("owner"):
        return []
    show = all_topics(r, params).get(rec["topic"])
    if show is None or show.get("kind") != "source":
        return []
    src = source_of(show["manifest"], name)
    # The builder's edge stamps `hop.resident` on every answer (OR-DP.M.1); an answer
    # without it is not one this presenter asked for (review Q M-3).
    if src is None or hop.get("resident") != src["read"]:
        return []
    data = {}
    failed = str(hop.get("resident_status") or "") in ("reject", "error") \
        or bool(hop.get("error_code")) or body.get("ok") is False
    round_ = as_round(hop.get("resident_round"))
    if failed:
        # The presenter's own read was refused: nothing of another round reaches it, so
        # the set came empty and the standard may stand (only an app's set that the
        # screen may not see is a set that never came, `feed`).
        rec["sets"].setdefault(name, {})
    else:
        if "rows" in src and isinstance(at_path(body, src["rows"]), list):
            data["rows"] = at_path(body, src["rows"])
        if "value" in src and isinstance(at_path(body, src["value"]), dict):
            data["value"] = at_path(body, src["value"])
        if round_ is not None:
            data["audience_set"] = round_
    return feed(r, rec, show["manifest"], {name: data}, params, now)


def registry_entry(presenter, requirement):
    """PE-DP-11: what the presenter's decider is announced to the model registry as -- a
    subscriber speaking the `decisions` protocol, born without a model (the registry fills
    it from its decisions rows), with the need its template states. The recipe that draws
    the road writes exactly this entry."""
    return {"cell_path": presenter.rstrip("/") + "/decide", "start_model": "",
            "requirement": requirement, "protocol": "decisions"}


# ---------------------------------------------------------------------------
# The store's answers


def pass_store(body, hop, ctx, params):
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
        for row in rows_of(body, "b-work"):
            if row.get("call_id"):
                r["work"].append(row)
                r["seq"] = max(int(r["seq"]), int(row.get("seq") or 0))
    r["phase"] = "live"
    if not failed:
        out += say_topics(r, params, boot=True)
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
        return pass_store(body, hop, ctx, params)
    if route == "mutation_committed":
        # A reconfigure may change the table through the params alone (`observed_topics`,
        # the declared sources): said here once the cell is live (review K-G Minor 4).
        return lane_topics() + (say_topics(r, params) if r["phase"] == "live" else [])
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
    # After the lane, so a lane that says the table itself (`show_topics`) is not said
    # twice; a no-op unless the switch is on and the params changed the table.
    out = live(body, params, hop, ctx, origin, route, now, r)
    return out + say_topics(r, params)


def live(body, params, hop, ctx, origin, route, now, r):
    """The lanes of a live cell (`handle` past the boot)."""
    # The decider's answer: the edge back from `./decide` deletes the inner context
    # (no marker leaves the hive, gh494) and restamps the hop `in_decision` with the
    # turn's `show_id` -- a colony run read `context {}` on that hop and dropped every
    # verdict as routeless. Only that edge sets the route: the door from `.` passes a
    # fixed list of routes that does not include it.
    stamped = verdict_source(params) == SOURCE_STAMP
    if origin == "decide" or route == "in_decision":
        # The stamp way makes no call, so no answer of the decider is one of its turns.
        return [] if stamped else lane_decision(body, hop, params, now)
    if route == "in_verdict":
        # live-2 GR: the talky's stamp; the selector way (shipped) does not read it.
        return lane_verdict(body, hop, ctx, params, now) if stamped else []
    if route == "in_tick":
        return lane_tick(hop, now, params)
    if route == "in_tick_error":
        # GH #1047: `stage` orders only `add` under a fresh id and `remove`, so
        # `schedule_not_found` answers a remove whose order already left the clock (it
        # struck on the way). The order is gone, which is what the remove wanted: no fault.
        if str(hop.get("error_code") or "") == "schedule_not_found":
            return []
        said = [error("clock_refused", str(body.get("detail") or hop.get("error_code")
                                             or "the clock refused an order"))]
        return said + lost_deadline(hop, now)
    if route == "show_topics":
        return lane_show_topics(body, hop, ctx, params)
    if route == "show_data":
        return lane_show_data(body, hop, ctx, params, now)
    if route == "turn":
        return lane_turn(body, hop, ctx, params, now)
    # GH #963: the taps of the builder's `observes_tool_calls` (restamped `in_tool_call`)
    # and of the string-form `observes_tool_results` (the producer's lane, unstamped), and
    # a resident's answer to a declared source. None of them is answered.
    if route == "in_tool_call":
        return lane_tool_call(body, hop, ctx, params, now)
    if route == "tool_result":
        return lane_tool_result(body, hop, ctx, params, now)
    if route == "resident_answer":
        return lane_resident_answer(body, hop, ctx, params, now)
    return []


def main():
    return handle(json.load(sys.stdin))


if __name__ == "__main__":
    out = main()
    sys.stdout.write(json.dumps(out[0] if len(out) == 1 else out))
