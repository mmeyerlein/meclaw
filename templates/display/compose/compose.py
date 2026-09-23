"""The compose cell: a view in, one screen out.

THIS FILE IS THE SOURCE. `config.json` carries a byte-identical copy of it in
`params.script_inline`. Edit here, then regenerate the copy:

    python3 -c 'import json,io; p="templates/display/compose/config.json"; \
d=json.load(io.open(p)); \
d["params"]["script_inline"]=io.open("templates/display/compose/compose.py").read(); \
io.open(p,"w").write(json.dumps(d,indent=2,ensure_ascii=False)+"\n")'

Why the copy exists at all: a `code` cell's `script_path` is handed to the
interpreter verbatim with no working directory of its own, so a relative path
would resolve against the daemon's cwd and an absolute path baked into a
template is the exported-tree defect class of GH #20. So the runtime form is
`script_inline`, and this file is what a person reads, greps, diffs and runs:

    python3 templates/display/compose/compose.py < some-stdin-doc.json

# What this cell is

It is the whole of the screen's bookkeeping, and it draws nothing itself. An
agent or an app says "here is a view of mine, put it up"; this cell decides
what the display's object tree must therefore look like, and says so as
`object.*` calls. It holds no model, opens no socket and makes no layout
judgement of its own: the order of the screen is the pass's (`canvas_order` for
the open canvas windows, `dock_order` for the tiles), and never the moment a
view was last written or the `ord` a sender asked for.

It is NOT the owner of a view's content. The content is whatever the sender
sent, rendered by whatever component the sender defined. This cell only ever
wraps it, places it, and takes it away again.

It is NOT the owner of identity either. The owner of a view is
`envelope.reply_to` -- the path of the cell that emitted the message -- and
never a field in the body. A body may repeat it, and a body that repeats it
WRONG is refused rather than believed: that is the whole of `not_owner`.

# The lanes, and the memory between them

The discriminator is the ENVELOPE HEADER, never the shape of the body. A body
is written by whoever sent it; a header is written by the edge that carried it,
and the edges of this hive are the only thing that knows where a message has
already been. Guessing a lane from the body is how a reply gets mistaken for a
request, which is how a loop starts.

Since display@2.7.0 the cell runs `resident` (GH #809): the harness compiles
this script once and executes it again for every message into ONE globals dict
(`crates/meclaw-cells/src/code/harness.py`), so what `ram()` holds survives
from one message to the next -- the curator's state, a mirror of the app rows,
and the object tree this cell last sent. RAM is a cache and never the truth:
the truth lies in the store (the app rows plus ONE small rest row) and at
`web` (the tree), and a killed child draws the same screen out of the two
again. No header carries the screen any more; `display_request` is a small mark.

A request (`in_view`, `in_withdraw`, `in_notice`): validated, then ONE store
bundle -- a `delete` of this owner's row for this `view_id` and, on a write,
an `insert` of the new one -- and in the same turn the pass of § 4 over the
state in memory, ONE `patch` of what changed on the display, and the rest row
when its content moved. Delete-then-insert IS the primary key. A `store`
schema declaration carries column types and nothing else -- no PRIMARY KEY, no
UNIQUE, no index -- so `(owner, view_id)` is an identity this cell keeps by
hand, in one bundle, in that order.

A browser event the `web` cell could not absorb locally: `tap` and `hold` are
the screen's own events and run a pass. Anything else has its object id parsed
back into the `(owner, view_id)` that produced it and leaves the hive with both
attached, so a member can route it to the one agent that put the view up. If
the id does not parse, the event goes out ANYWAY without them: a dead letter
somebody can read beats a silent drop. The clock's strike (`in_tick`) and the
judge's verdict (`in_verdict`) run a pass each; nothing is read for them.

The replies (`context.display_origin`, stamped by the hive's own edges):
`views` -- the store acknowledged a write (a refused leg is a receipt to the
app that wrote) or answered the boot select; `read` -- the display's tree,
asked exactly twice in a life: at the boot and after a refused patch; `patch`
-- nothing, unless a leg was refused, and then the mirror is dirty and ONE
read repairs it. No acknowledgement is answered with a message of its own,
which is what stops the loop (GH #161).

The boot. The first message after a start has neither rows nor a tree, so it
computes nothing: its event is queued, ONE select reads the rows (and the rest
row), ONE read the tree, and then every queued event runs as its own pass and
ONE patch draws them. A write that arrives inside that window still goes to
the store at once. The tree answer settles "is this page MINE", and there are
two ways it is not: no page at `/` at all (`query` is refused), or a page whose
root is somebody else's. BOTH are the bootstrap case. Reading only the refusal
is the GH #402 defect: `display/web` refs the `web` template, which SEEDS a
demo page at `/`, so the query succeeds, the vocabulary is never defined, and
every `object.create` comes back `unknown_component` while the deletes land.
The bootstrap adopts the page and DELETES NOTHING -- those objects are not this
cell's to remove.

An `object.update` whose props the display already holds is not sent at all
(GH #412). The `web` cell applies a bundle through its single database actor,
and a browser's own `object:set` is served by that same actor: a full rewrite
of an unchanged tree holds it for the length of the rewrite, and anything a
person did in that window is written late while the rewrite's diffs re-render
it where it was.
"""
import copy
import hashlib
import json
import sys
import time
import uuid
from datetime import datetime, timezone

# The table this cell keeps, and the projection it reads back. Both are the
# store's own vocabulary; the column list is written once so a select can never
# disagree with an insert about what a row is.
TABLE = "views"
COLUMNS = [
    "owner",
    "view_id",
    "region",
    "ord",
    "kind",
    "content",
    "components",
    "ttl_ms",
    "updated_at",
]

# The regions a screen has, in the order they stand on the page, and a closed
# list rather than a free string: an unknown region is a view nobody would ever
# see, which is worse than a refusal the sender can read.
#
# `main` is the wide column and the DEFAULT, so every view that named no region
# before this version lands exactly where it landed then. `aside` is the narrow
# column beside it, and it is what the second region is FOR: a clock, a weather
# tile, a countdown -- things that are true whether or not anybody is talking,
# and that have no business pushing a conversation down the page (GH #609).
REGIONS = ("main", "aside")
REGION_INDEX = dict((name, i) for i, name in enumerate(REGIONS))

# The screen's judgement (GH #679). A WINDOW is an object of one of these four
# components; everything else on the screen is content inside one. The curator
# writes the curator props on a window and on nothing else; an application
# writes the hints, and may say exactly two words about the state. `touched`
# is one of the hints (GH #689): an epoch the application changes when the
# window's content is new. The screen compares a window's OWN props, and an answer that lives
# in a child component is invisible to that comparison unless the application
# says so -- a child's props are not read on purpose, or a clock that rewrites
# a child every twenty seconds would touch its window on every tick.
WINDOWS = ("display-pane", "display-panel", "display-overlay", "display-view-prose")
# The hints of § 3 an application may send about its own window. There is no
# `CURATOR_KEYS` beside them any more: since 2.5.0 no curator value stands on a display
# object as memory -- the state lies in the store (§ 3.1), and what the objects wear is
# the RENDERING of it (`window_attrs`).
HINT_KEYS = ("context", "relevance", "class", "pinned", "relevant_until", "touched",
             "topic", "layer", "seat", "seat_ord", "linger", "state", "turn_id")

# The two ladders (R-23-2). A window competes for the focus only inside its
# own layer, so a chat over a document does not take the document's rung away:
# one focus per ladder, and at most one modal is ever on the screen.
# The seats in the dock (R-23-3). One place for now, because one place is what
# the design asked for: the bottom edge, where the clock and the weather stand.
# The two events the screen answers ITSELF (R-23-1, R-23-5, OR-F6). Everything
# else a browser says leaves the hive on the `event` lane, as it always has:
# which window stands large is display hygiene and nobody else's business,
# and a button inside an application's own tree is the application's.
TAP_EVENT = "tap"
HOLD_EVENT = "hold"
# The namespace an order's id is derived from. Deterministic per due time, so
# two passes that compute the same moment order the same id and the second
# `add` collides instead of standing beside the first (GH #681).
DUE_NAMESPACE = uuid.UUID("6f2d7c1a-3a1e-4d7b-9d5a-1c2b3e4f5a60")
# The knobs as shipped; `main()` reads the member's dials over them.
# Per notice class, `[relevance, ttl_ms]` as shipped (`params.notice_defaults`
# says otherwise). `CLASS_RELEVANCE` is the first half, the one the score reads.
NOTICE_DEFAULTS = {"system_error": (0.9, 60000), "error": (0.8, 60000),
                   "warning": (0.7, 120000), "important_note": (0.7, 300000),
                   "note": (0.4, 300000)}
# The screen settings of § 3 that the pass reads, and the two module globals that carry
# them. RAW, as `params` said them: the door of § 4.7 normalises them in the state, once,
# and says out loud what it replaced. There is no resolved copy beside them -- a second,
# already-normalised word is a value nobody refuses and nobody reads.
SETTING_KEYS = ("linger_ms", "fade_ms", "focus_default", "judge_min_interval_ms",
                "judge", "default_screen")
KNOB_SETTINGS = {}
KNOB_SCREENS = {}
# The sheet's ground: `day`, or `night` when the operator says so
# (`params.ground`). A word, not a clock -- nothing switches it by itself.
GROUND = "day"

# The object ids, by class. Deterministic and prefixed: an id has to be
# derivable from the row without a side table, and it has to say which class it
# belongs to, because deletion sweeps by prefix.
ROOT_ID = "display.root"
REGION_PREFIX = "display.region."
# The OS mark (D-2, D-3): the one fixed anchor at the bottom right, and the
# push-to-talk button itself. A NEW id rather than the microphone's, because
# it is another thing in another place with another markup (OR-D7); the old
# `display.mic` is swept as an orphan on the first pass after the lift.
OS_ID = "display.os"
OLD_MIC_ID = "display.mic"
VIEW_PREFIX = "view."
# The dock (D-1, D-4): one tile per PRESENT window, the highest rank on top.
# A child of the root and not of a region, because it is a layer over the
# canvas rather than a column beside it.
DOCK_ID = "display.dock"
DOCK_PREFIX = DOCK_ID + "/"
# The key an application gives the child that is its tile (R-D1).
TILE_KEY = "tile"
# One line, and a line is a line: the sheet gives a tile one row of text.
TILE_LINE_MAX = 24
# The glyph a window gets when it brought no tile of its own. Text, not
# images: the two faces cover these, and so does a kiosk's system font.
TILE_GLYPHS = {
    "conversation": "\U0001F4AC",
    "ambient": "\u25d4",
    "system": "\u26a0",
    "system_error": "\u26a0",
    "error": "\u26a0",
    "warning": "\u25b3",
    "important_note": "!",
    "note": "\u00b7",
}
TILE_FALLBACK_GLYPH = "\u2022"
# The exits of one screen state (R-D3) and their profiles. A member has ONE
# display hive; a physical screen is an exit of it, and the profile is what
# the renderer knows about that exit.
# A screen whose `params` name no outputs still has to be a screen. `inputs: []`,
# because § 4.7 forces a television to it whatever the profile says.
DEFAULT_SCREENS = {"tv": {"display_type": "tv", "viewing_distance_m": 3.0,
                          "physical_size_in": 55, "inputs": []}}
# No formula out of DPI, a table (OR-D9): three types are what there is, and
# everything past this would be tuning. The distance modulates linearly
# against the reference distance of the type, clamped.
SCREEN_BASE = {"tv": 1.6, "monitor": 1.0, "phone": 1.0}
SCREEN_REFERENCE = {"tv": 3.0, "monitor": 0.7, "phone": 0.35}
SCALE_MIN = 0.8
SCALE_MAX = 2.2
PAGE_ROUTE = "/"
PAGE_TITLE = "display"

# The components that bring a script (§ 2.5). It used to be exactly one and
# the lock said so as a number; a number is not a decision, so this is the
# list, and the lock reads it. Two: the OS mark, whose hook is the gesture,
# and the shell, whose hook is the screen's own motion.
SCRIPTED = ("display-os", "display-shell")

# `view_id` is `[a-z0-9-]{1,64}`. Spelled as a character set rather than a
# regex so this script needs nothing but the three standard modules.
ID_CHARS = frozenset("abcdefghijklmnopqrstuvwxyz0123456789-")
ID_MAX = 64
# A node key is an identity, not a name a person types: a cell path with its
# slashes written as tildes is already 120 characters deep in a real colony.
KEY_MAX = 512

# `ord` is a sort key, not a list index: gaps leave room to insert without
# renumbering anything the display already holds.
ORD_STEP = 10

# How deep a component tree may be. The `web` cell stops rendering at 64 levels
# and reports the object it stopped at; refusing earlier, at the door, turns
# that into an answer to whoever wrote the tree.
MAX_DEPTH = 32

# The closed error surface of this hive. Every refusal leaves on the `receipt`
# lane carrying exactly one of these.
ERRORS = (
    "not_owner",
    "owner_unknown",
    "invalid_view",
    "component_prefix",
    "store_failed",
    # The door of the pass (§ 4.6, § 4.7). A refused view, a refused setting or profile
    # value, and the two errors nothing replaces: a profile without `display_type` and a
    # `default_screen` that names no output.
    "view_refused",
    "setting_refused",
    "profile_refused",
    "setting_error",
    "profile_error",
)

# --- The pass: display-hive.md § 4, model/pass.py verbatim (begin) ---
RUNGS = ("hidden", "ambient", "relevant", "focus", "urgent")
LADDERS = ("canvas", "modal")
STATE_WORDS = ("urgent", "hidden")
SEAT_WORDS = ("bottom",)
NUMERIC_HINTS = ("relevance", "linger", "seat_ord", "touched", "relevant_until")
CLASS_RELEVANCE = {"system_error": 0.9, "error": 0.8, "warning": 0.7,
                   "important_note": 0.7, "note": 0.4}
DEFAULT_WEIGHT = 0.5
DEFAULT_RELEVANCE = 0.5
RANK_FLOOR = 0.05
AFTER_UNTIL_CAP = 0.2
DEFAULT_SETTINGS = {"linger_ms": 20000, "fade_ms": 120000, "focus_default": 0.3,
                    "judge_min_interval_ms": 3000, "judge": "off"}
PROFILE_DEFAULTS = {"tv": ("shown", 7), "monitor": ("shown", 8), "phone": ("hidden", 5)}
INPUT_WORDS = ("audio", "touch", "pointer", "keyboard")
TRIGGERS = ("app_write", "app_withdraw", "verdict", "tap", "hold", "stroke")
APP_SOURCES = ("a", "b", "c", "f")
FINGER_SOURCES = ("d", "e")
# Keys of a view that are not own props of the window (§ 4.8 b): the bookkeeping of the
# view beside the window, the children, and the two hints with their own rules.
NOT_OWN_PROPS = ("owner", "children", "ttl_ms", "written_at", "withdrawn", "verdict",
                 "curator", "state", "touched")


def step1_triggers(state, event, now):
    """§ 4.1: a pass runs on, closed list: an app writes or withdraws a view (the clock's
    minute included), a verdict arrives, a tap, a hold, a stroke. Nothing else.
    § 4.5: the judge writes `judged_relevance`/`judged_hidden` per window, `bar` and
    `weights` on the state — a `judged_relevance` outside 0–1 is clamped to 0–1 on
    arrival (Decision 16.09. 23); a verdict holds until the next judge run or until a tap, a
    hold or an app touch of source (b)/(c) clears it for that window (Decision 18.09.) —
    which also drops that window's `verdict_cleared` mark, because a judge run replaces the
    whole verdict; a put-away clears no verdict. The judge decides what is open, never what exists: presence
    never ends by a verdict (step 4; Leitlinie; Ruling 14.09. "size, never existence"). The
    verdict is applied here, on arrival, before the touches and the score (compose.py applies
    it before the score as well)."""
    kind = event.get("kind")
    if kind not in TRIGGERS:
        state["pass"]["runs"] = False          # § 4.1: nothing else triggers a pass
        return state
    state["pass"]["runs"] = True
    if kind == "verdict":
        if state["settings"].get("judge") != "on":
            # § 3 `judge`: off, missing or any other word = the hive has no judge cell,
            # so no verdict can arrive.  # Decision 16.09. 1
            state["pass"]["refused"].append(("verdict", "no judge cell"))
            return state
        # § 4.5: the next judge run replaces the whole verdict: bar, weights, and the
        # per-window values of every window — a window the verdict does not name loses
        # its old verdict.  # Decision 16.09. 2
        bar = event.get("bar")                 # missing → focus_default (§ 4.16)  # Decision 16.09. 18
        weights = {str(k): float(v) for k, v in (event.get("weights") or {}).items()}
        state["judge"]["verdict"] = {"bar": bar, "at": now}
        state["weights"] = weights
        named = event.get("windows") or {}
        for oid, v in state["views"].items():
            entry = named.get(oid) or {}
            jr = entry.get("judged_relevance")
            if jr is not None:
                # § 3: 0–1; outside it, clamped on arrival like `relevance` at the door
                # (§ 1.3: no sender has a bonus).  # Decision 16.09. 23
                jr = min(1.0, max(0.0, float(jr)))
            v["verdict"] = {"judged_relevance": jr,
                            "judged_hidden": entry.get("judged_hidden")}
            # "replaces the whole verdict": the mark a touch left goes with it, so the
            # window counts with the judge's weights again.  # Decision 18.09. 27
            v["curator"]["verdict_cleared"] = False
    return state


def step1_judge_call(state, now):
    """§ 4.3: the curator calls the judge at the end of a pass in which at least one window
    received a touch from an app source (§ 4.8 a–c, f), at the earliest
    `judge_min_interval_ms` after the last call; never after a pass that only a tap, a hold,
    a stroke or the clock's minute triggered. A call that falls into the interval is
    discarded, not made up: the judge sees the situation in the next pass with an app touch
    after the interval (Decision 16.09.). § 4.16: with `judge: off` there is no judge cell."""
    state["judge"]["called"] = False
    if state["settings"].get("judge") != "on":
        return state
    app_touch = any(src in APP_SOURCES for src in state["pass"]["touched"].values())
    if not app_touch:
        return state
    last = state["judge"]["last_call"]
    if last is not None and now - last < state["settings"]["judge_min_interval_ms"]:
        return state                                   # discarded, not made up
    state["judge"]["called"] = True
    state["judge"]["last_call"] = now
    return state


def step2_door(state, event, now):
    """§ 4.6: the screen normalises what an app sends and rejects what it does not know: a
    `state` other than `urgent`/`hidden`, a `seat` other than `bottom`, a `ttl_ms` that is
    not a non-negative integer (Decision 16.09.). Numeric hints travel as text (§ 3.3);
    `0` and a missing value are the same, so a `seat` without `seat_ord` stands at 0
    (Decision 16.09. 22). `relevance` is clamped to 0–1 (§ 1.3: no sender has a bonus);
    what clamps to 0 reads as empty like `0` (Decision 16.09. 23). The write that passes
    enters the store; a withdrawal marks the view withdrawn (it leaves the state by § 4.35).
    § 4.7: profiles pass the same door: `display_type` is mandatory — missing is an error,
    not a silent tv; `default_screen` is mandatory and names an entry of `screens`; a
    missing `inputs` is `[]`, for `phone` too; type `tv` → `inputs: []`, whatever the
    profile says (Marcus 14.09.; as a type rule Decision 16.09.). An error is reported in
    every pass; the profile stays raw. `dock_default` is one of `shown`/`hidden` and
    `dock_max` a whole number ≥ 1, as number or as text (§ 3.3); any other value is the
    type default and is refused once, in the pass that replaces it (Decision 16.09. 21).
    The settings pass the door too: `linger_ms`, `fade_ms`, `judge_min_interval_ms` are
    integers ≥ 1, `focus_default` a number in 0–1; any other value is the setting's default
    of § 3 and is refused once, in the pass that replaces it (Decision 16.09. 20)."""
    # Settings, every pass (idempotent): an invalid value is the default of § 3, refused
    # once — in the pass that replaces it.  # Decision 16.09. 20
    settings = state["settings"]
    for key in ("linger_ms", "fade_ms", "judge_min_interval_ms"):
        val = settings.get(key)
        if isinstance(val, bool) or not isinstance(val, int) or val < 1:
            state["pass"]["refused"].append(("settings", key, val))
            settings[key] = DEFAULT_SETTINGS[key]
    fd = settings.get("focus_default")
    if isinstance(fd, bool) or not isinstance(fd, (int, float)) or not 0 <= fd <= 1:
        state["pass"]["refused"].append(("settings", "focus_default", fd))
        settings["focus_default"] = DEFAULT_SETTINGS["focus_default"]

    # Profiles, every pass (idempotent).
    screens = {}
    for name, prof in (state["screens"] or {}).items():
        prof = dict(prof or {})
        dtype = prof.get("display_type")
        if dtype not in PROFILE_DEFAULTS:
            # An error, not a silent tv: the profile stays raw and is reported every pass.
            state["pass"]["errors"].append(("screen", name, "display_type missing"))
            prof["error"] = "display_type missing"
            screens[name] = prof
            continue
        shown, dmax = PROFILE_DEFAULTS[dtype]
        inputs = [w for w in (prof.get("inputs") or []) if w in INPUT_WORDS]
        if dtype == "tv":
            inputs = []                                # § 4.7: a type rule
        prof["inputs"] = inputs
        # `dock_default` and `dock_max` are normalised: any other value is the type
        # default, refused once — in the pass that replaces it.  # Decision 16.09. 21
        dd = prof.get("dock_default")
        if dd in (None, ""):
            dd = shown
        elif dd not in ("shown", "hidden"):
            state["pass"]["refused"].append(("screen", name, "dock_default", dd))
            dd = shown
        prof["dock_default"] = dd
        dm = prof.get("dock_max")
        if dm in (None, ""):
            dm = dmax
        else:
            n = _whole(dm)
            if n is None or n < 1:
                state["pass"]["refused"].append(("screen", name, "dock_max", dm))
                n = dmax
            dm = n
        prof["dock_max"] = dm
        screens[name] = prof
    state["screens"] = screens
    default = state["settings"].get("default_screen")
    if not default or default not in screens:
        state["pass"]["errors"].append(("settings", "default_screen", "missing or unknown"))

    kind = event.get("kind")
    views = state["views"]
    if kind == "app_withdraw":
        oid = event.get("oid")
        if oid in views:
            views[oid]["withdrawn"] = True
        return state
    if kind != "app_write":
        return state

    oid = event.get("oid")
    sent = dict(event.get("view") or {})
    # Reject what the door does not know.
    word = sent.get("state")
    if word not in (None, "") and word not in STATE_WORDS:
        state["pass"]["refused"].append((oid, "state", word))
        return state
    seat = sent.get("seat")
    if seat not in (None, "") and seat not in SEAT_WORDS:
        state["pass"]["refused"].append((oid, "seat", seat))
        return state
    ttl = sent.get("ttl_ms", 0)
    if ttl is None:
        ttl = 0
    if isinstance(ttl, bool) or not isinstance(ttl, int) or ttl < 0:
        state["pass"]["refused"].append((oid, "ttl_ms", ttl))
        return state
    # Normalise numeric hints (text → number, empty → unset).
    for k in NUMERIC_HINTS:
        if k in sent:
            sent[k] = _number(sent[k])
    # § 3 `relevance` 0–1, § 1.3 "no sender has a bonus": clamped at the door; what clamps
    # to 0 reads as empty like `0` (§ 3.3) and removes the key.  # Decision 16.09. 23
    if sent.get("relevance") is not None:
        r = min(1.0, max(0.0, float(sent["relevance"])))
        sent["relevance"] = None if r == 0 else (int(r) if r == int(r) else r)
    if "layer" in sent and sent["layer"] not in LADDERS:
        sent["layer"] = "canvas"                       # § 3: default canvas  # Decision 16.09. 3
    if sent.get("class") not in CLASS_RELEVANCE:
        sent.pop("class", None)                        # § 3: an unknown class counts as unset

    prior = views.get(oid)
    if prior is None or prior.get("withdrawn"):
        # A new window enters the store (§ 4.8 a). A view written again after its
        # withdrawal is new.  # Decision 16.09. 4
        view = {"owner": sent.get("owner", oid.split(".")[1] if "." in oid else oid),
                "children": {}, "ttl_ms": ttl, "written_at": now, "withdrawn": False,
                "verdict": {"judged_relevance": None, "judged_hidden": None},
                "curator": {"since": None, "dismissed_at": 0, "led_until": 0,
                            "verdict_cleared": False,
                            "topic_dupe": False, "present": False, "rung": None,
                            "score": 0.0, "decay": 0.0, "age": None, "level": 0,
                            "rank": 0.0, "front": False, "open": False}}
        state["pass"]["new"].append(oid)
        state["pass"]["prior_props"] = state["pass"].get("prior_props", {})
        state["pass"]["prior_props"][oid] = None
    else:
        view = prior
        state["pass"]["prior_props"] = state["pass"].get("prior_props", {})
        state["pass"]["prior_props"][oid] = {k: v for k, v in prior.items()
                                             if k not in ("children", "verdict", "curator")}
    # The write merges per key (compose.py: `object.update` merges per key); a key left
    # out stands; an explicit empty value removes the key.  # Decision 16.09. 5
    # `end_at` and the rest of the tile are children: not normalised, not read by the
    # pass (§ 3.3 names no wire format for it).  # Decision 16.09. 17
    for k, v in sent.items():
        if k in ("children",):
            view["children"] = dict(view.get("children") or {})
            view["children"].update(v or {})
            continue
        if k in ("owner",):
            view["owner"] = v
            continue
        if v is None or v == "":
            view.pop(k, None)
        else:
            view[k] = v
    view["ttl_ms"] = ttl
    view["written_at"] = now                           # § 4.34: ttl counts from the last write  # Decision 16.09. 6
    view["withdrawn"] = False
    views[oid] = view
    state["pass"]["written"] = (oid, sent)
    return state


def _number(value):
    """§ 3.3: numeric hints travel as text; `0` and `''` read as empty."""
    if value is None or value == "" or value is False:
        return None
    try:
        f = float(value)
    except (TypeError, ValueError):
        return None
    if f == 0:
        return None
    return int(f) if f == int(f) else f


def _whole(value):
    """§ 4.7: a profile number (`dock_max`) as number or text; a whole number, else None."""
    if isinstance(value, bool):
        return None
    try:
        f = float(value)
    except (TypeError, ValueError):
        return None
    return int(f) if f == int(f) else None


def step3_touches(state, event, now):
    """§ 4.8: sources, closed list: (a) a window appears new; (b) the app changes own props
    of the window (`state` excepted), not those of its children; (c) the app writes
    `touched` greater than the last seen; (d) a tap on the tile of a window that is not
    open (§ 5.1); (e) a hold (§ 5.4); (f) is the topic touch of § 4.12 (step 4) — that is
    how an answer reaches a standing window. Not a touch: a child change without `touched`
    (the clock's minute, a weather measurement, § 9.3–9.4), a judge run, a press, a put-away
    tap (§ 5.2), a dock cut, a change of `state` alone — `state: urgent` set or removed with
    no other own prop and no `touched` in the same write (the end of ringing, § 9.1)
    (Decision 16.09.).
    § 4.9: what a finger touch writes beyond `since`; an app touch writes `since` and, for
    the sources (b) and (c), clears the verdict of that window — (a) has no verdict yet and
    (f), the topic touch, leaves the standing window's verdict alone (Decision 18.09.).
    Clearing a verdict means the whole standing judgement about that window: the two judge
    values and, through the mark `verdict_cleared`, the weight of its context (§ 4.14,
    Decision 18.09.). `bar` is the screen's and stays (§ 4.16).
    § 5.2: a tap on the tile of an open window is a put-away: `dismissed_at = now`,
    `led_until = 0`, `since` stays, the verdict stays (§ 4.5); the tile keeps its rank (§ 4.27)
    and the decay runs on (R-24-1). A later app touch lifts the put-away, because then
    `since > dismissed_at` — except the chat under step 5. § 5.3: the modal-closing rule —
    a tap on the tile of a canvas app while a modal is open puts the modal away, whether the
    tap opens the canvas app or puts it away (R-23-2; "also on put-away": Decision 16.09.).
    "A canvas app" is the app whose window carries `layer: canvas` (`layer_of`, the hint as
    written, the default included), whatever its rung: an urgent window stands on no ladder
    for the focus choice (§ 4.18), but its app's `layer` still decides this rule — a tap on
    the tile of an urgent with `layer: canvas` puts the open modal away, whether it puts
    the urgent away or brings it to the front; only the modal side of § 4.18 is exempt
    (a front urgent with `layer: modal` is no modal here) (Decision 16.09. 19).
    § 5.4: a hold acts on the window with `topic: chat` like a tap by § 5.1, even when it is
    open — a hold never puts away (Brief § 16; R-23-5); without such a view the hold is
    absorbed: nothing opens, no error (Decision 16.09.). § 5.8: ten taps on one tile produce
    no error; each applies to the state after the previous one (Decision 16.09.)."""
    kind = event.get("kind")
    views = state["views"]
    settings = state["settings"]
    touched = state["pass"]["touched"]

    if kind == "app_write" and "written" in state["pass"]:
        oid, sent = state["pass"]["written"]
        view = views[oid]
        prior = state["pass"]["prior_props"].get(oid)
        if prior is None:
            touched[oid] = "a"                         # (a) the window appears new
            _app_touch(view, now)
        else:
            own_changed = any(k not in NOT_OWN_PROPS and prior.get(k) != view.get(k)
                              for k in sent.keys())
            said = sent.get("touched")
            last_seen = prior.get("touched")
            touched_up = said is not None and (last_seen is None or said > last_seen)
            if own_changed:
                touched[oid] = "b"                     # (b) own props changed
                # since = now, not the `touched` value  # Decision 16.09. 7
                _app_touch(view, now, clears_verdict=True)   # Decision 18.09. 26
            elif touched_up:
                touched[oid] = "c"                     # (c) `touched` greater than last seen
                _app_touch(view, now, clears_verdict=True)   # Decision 18.09. 26
            # else: a child change, `state` alone, or a repeated write: no touch.
        if view.get("topic") == "chat" and "turn_id" in sent:
            state["pass"]["chat_wrote_turn_id"] = True  # § 4.13 trigger "the chat app writes that turn_id"

    elif kind == "tap":
        oid = event.get("for")
        view = views.get(oid)
        if view is None or not view["curator"].get("present"):
            state["pass"]["refused"].append((oid, "tap", "no tile"))  # Decision 16.09. 8
            return state
        c = view["curator"]
        was_open = c.get("open", False)                # open = level 1–3 of the last pass (§ 4.24)
        if was_open:
            c["dismissed_at"] = now                    # § 5.2: put away; `since` stays
            c["led_until"] = 0
            state["pass"]["put_away"].append(oid)
        else:
            touched[oid] = "d"                         # § 5.1
            _finger_touch(view, now, settings)
        # § 5.3 modal-closing rule: a tap on the tile of a canvas app while a modal is open
        # (a `layer: modal` window on level 2, never an urgent) puts the modal away,
        # whether the tap opens the canvas app or puts it away. "Canvas app" = the tapped
        # window's `layer`, its rung unread: an urgent with `layer: canvas` co-closes
        # too.  # Decision 16.09. 19
        if layer_of(view) == "canvas":
            for o, w in views.items():
                if o != oid and layer_of(w) == "modal" and w["curator"].get("level") == 2:
                    w["curator"]["dismissed_at"] = now
                    w["curator"]["led_until"] = 0
                    state["pass"]["co_closed"].append(o)

    elif kind == "hold":
        chat = sorted(o for o, w in views.items() if w.get("topic") == "chat" and not w.get("withdrawn"))
        if not chat:
            state["pass"]["refused"].append(("hold", "absorbed", "no chat view"))  # § 5.4
            return state
        # § 8.5: exactly one window carries `topic: chat`; should there be two, the present
        # one (a dupe is not present, § 4.12), then the smaller id.
        oid = ([o for o in chat if views[o]["curator"].get("present")] or chat)[0]
        touched[oid] = "e"
        _finger_touch(views[oid], now, settings)       # § 5.4: a hold never puts away
    return state


def _finger_touch(view, now, settings):
    """§ 4.9: a tap (d) and a hold (e) set `since = now`, `dismissed_at = 0`, clear the
    verdict for that window, and set `led_until = since + linger` (the linger of § 4.15).
    `led_until` is what tells a finger touch from an app touch in every later pass; nothing
    else is remembered: `since`, `dismissed_at` and `led_until` are the whole statement
    (Decision 16.09.)."""
    c = view["curator"]
    c["since"] = now
    c["dismissed_at"] = 0
    view["verdict"] = {"judged_relevance": None, "judged_hidden": None}
    c["verdict_cleared"] = True                        # the whole verdict, weight included  # Decision 18.09. 27
    c["led_until"] = now + linger_of(view, settings)


def _app_touch(view, now, clears_verdict=False):
    """§ 4.8: a touch sets `since = now` and restarts the decay; § 4.9: an app touch (a, b,
    c, f) writes nothing else — never `led_until`, never `dismissed_at`. The verdict: the
    sources (b) and (c) clear it for that window, the sources (a) and (f) do not
    (Decision 18.09.). The app has just changed the window, and the situation is judged anew
    at every real event (§ 1.3); until the judge speaks again the app's own `relevance`
    counts, as it does after a tap (§ 4.9). A window of source (a) carries no verdict yet;
    the topic touch (f) reaches a window the app did not write, and the judge may have hidden
    it on purpose."""
    view["curator"]["since"] = now
    if clears_verdict:
        # The woken window would otherwise stay closed under its old verdict, because
        # § 4.14 puts `judged_relevance` over `relevance` and the judge needs one to two
        # runs to speak again.  # Decision 18.09. 26
        view["verdict"] = {"judged_relevance": None, "judged_hidden": None}
        # And the context weight is that same standing judgement: cleared by halves, the
        # window stayed shut at `relevance 0.8 × weights.ambient 0.3 = 0.24 < bar 0.3`
        # (measured, run M10-h3b-berlin).  # Decision 18.09. 27
        view["curator"]["verdict_cleared"] = True


def step4_presence(state, now):
    """§ 4.11: present = the view stands in the store, is not `topic_dupe`, and meets one of:
    pinned, `decay > 0`, `relevant_until` in the future. Present means: its tile stands in
    the screen state (whether an output draws it, `dock` says); not present means no window,
    no tile, anywhere (Brief § 6, § 7). Presence ends only by decay (`decay = 0`, not pinned,
    `relevant_until` passed or unset), by `ttl_ms`, by `topic_dupe`, or by the app
    withdrawing the view; never by a verdict (Leitlinie; Ruling 14.09.). Decay takes the
    app's presence, not its view: the view stands in the store until the app withdraws it
    or `ttl_ms` runs out. A standing view that is not present carries no rung, has no tile,
    is not shown to the judge; "leaving the state" (§ 4.12, § 4.35) means losing presence
    (Decision 16.09.).
    § 4.12: a fresh window of another app on the topic of a present window: the standing
    window receives the touch (f) and, for § 4.13, the fresh window's `turn_id` counts for
    it; the fresh window receives `topic_dupe` — neither open nor present, no window, no
    tile, no rung, not computed — until the standing window loses its presence; windows of
    the same owner are never compared; the judge does not lift it (Ruling 13.09.)."""
    views = state["views"]
    settings = state["settings"]
    touched = state["pass"]["touched"]
    new = set(state["pass"]["new"])

    # Views that finished their leaving pass (§ 4.34/4.35) are gone: withdrawn or expired
    # ones leave the store, decayed ones stand in it without presence.
    for oid in list(views):
        v = views[oid]
        if v["curator"].get("age") == "leaving" and (v.get("withdrawn") or _ttl_expired(v, now)):
            del views[oid]
    # A withdrawal or an expired `ttl_ms` of a view that was not present: gone at once, no
    # leaving pass — the store holds it only until the app withdraws it or `ttl_ms` runs
    # out (§ 4.11), and its seat goes with it (§ 4.29).
    for oid in list(views):
        v = views[oid]
        if (v.get("withdrawn") or _ttl_expired(v, now)) and not v["curator"].get("present"):
            del views[oid]

    def standing(oid):
        v = views[oid]
        return (not v.get("withdrawn") and not _ttl_expired(v, now)
                and not v["curator"].get("topic_dupe") and _meets_presence(v, now, settings))

    # § 4.12 — the topic check for the fresh windows of this pass. "A standing window"
    # here is a present one: a decayed view has no window to duplicate.  # Decision 16.09. 12
    for oid in [o for o in new if o in views]:
        v = views[oid]
        topic = v.get("topic")
        if not topic:
            continue
        rivals = [o for o, w in views.items()
                  if o != oid and o not in new and w.get("owner") != v.get("owner")
                  and w.get("topic") == topic and standing(o)]
        if not rivals:
            continue
        v["curator"]["topic_dupe"] = True
        for o in rivals:
            touched[o] = "f"
            _app_touch(views[o], now)
            if v.get("turn_id"):
                state["pass"]["topic_turn_ids"][o] = v["turn_id"]
    # `topic_dupe` falls when the standing window loses its presence.
    for oid, v in views.items():
        if v["curator"].get("topic_dupe"):
            still = [o for o, w in views.items()
                     if o != oid and w.get("owner") != v.get("owner")
                     and w.get("topic") == v.get("topic") and standing(o)]
            if not still:
                v["curator"]["topic_dupe"] = False

    # Presence.
    for oid, v in views.items():
        c = v["curator"]
        c["was_present"] = c.get("present", False)
        c["present"] = standing(oid)
        if not c["present"]:
            c["rung"] = None
            c["level"] = 0
            c["open"] = False
            c["front"] = False
    return state


def linger_of(view, settings):
    """§ 4.15: linger = the app's `linger`, capped at `linger_ms + fade_ms`; without
    `linger` (missing or 0, § 3.3), the screen's `linger_ms` (defaults § 3; Decision 16.09.).
    § 7.5: a request with a cap; the app does not learn whether it was capped."""
    said = view.get("linger")
    if isinstance(said, (int, float)) and said > 0:
        return int(min(said, settings["linger_ms"] + settings["fade_ms"]))
    return settings["linger_ms"]


def decay_of(view, now, settings):
    """§ 4.15: `decay` is 1 during the linger since `since`, then falls linearly to 0 within
    `fade_ms`: `1 − (now − since − linger) / fade_ms`, clamped to [0, 1] (Decision 16.09.).
    Pinned: the presence stays — the tile stands; for score and rank the decay runs as for
    any window (R-24-3)."""
    since = view["curator"].get("since")
    if since is None:
        return 0.0
    past = now - since - linger_of(view, settings)
    if past <= 0:
        return 1.0
    return max(0.0, min(1.0, 1.0 - past / float(settings["fade_ms"])))


def _meets_presence(view, now, settings):
    """§ 4.11: pinned, or `decay > 0`, or `relevant_until` in the future."""
    until = view.get("relevant_until")
    return (view.get("pinned") is True or decay_of(view, now, settings) > 0.0
            or (isinstance(until, (int, float)) and until > now))


def _ttl_expired(view, now):
    ttl = view.get("ttl_ms") or 0
    return ttl > 0 and view.get("written_at") is not None and view["written_at"] + ttl <= now


def step5_chat_closes(state, now):
    """§ 4.13: if a canvas window carries the `turn_id` of the last turn the chat app has
    seen (§ 8.8), the curator sets at the chat window `dismissed_at = now`, `led_until = 0`
    — in every pass in which that canvas window receives a touch (fresh, own prop change,
    topic touch), the chat app writes that `turn_id`, or the chat receives a touch from an
    app source (a–c, f). A tap on the chat tile or a hold (d, e) is no trigger: it opens the
    chat by § 5.1 and § 5.4 also while such a canvas window is present, until the next
    trigger closes it again. Runs after the touches and before the score: the chat scores 0
    in that same pass, so the chat app's answer touch (§ 8.8) does not lift the put-away,
    whichever pass it arrives in. Only while such a canvas window is present (§ 4.11); once
    it is no longer present, § 5.2 applies again (R-23-2; the mechanics through `turn_id`,
    "present", `led_until` and the order: Decision 16.09.)."""
    views = state["views"]
    chat = _chat_oid(views)
    state["chat"]["last_turn_id"] = views[chat].get("turn_id") if chat else None
    if chat is None:
        return state
    tid = state["chat"]["last_turn_id"]
    if not tid:
        return state
    touched = state["pass"]["touched"]
    topic_tids = state["pass"]["topic_turn_ids"]
    carriers = [o for o, w in views.items()
                if o != chat and layer_of(w) == "canvas" and w["curator"].get("present")
                and (w.get("turn_id") == tid or topic_tids.get(o) == tid)]
    if not carriers:
        return state
    # Every app source counts for the carrier, `touched` (c) included: the main clause
    # says "receives a touch" and § 8.6 says "the next app touch"; the parenthetical of
    # § 4.13 lists only a, b, f.  # Decision 16.09. 11
    # "the chat app writes that turn_id": any write of the key in this pass, the same
    # value included.  # Decision 16.09. 13
    trigger = (any(touched.get(o) in APP_SOURCES for o in carriers)
               or state["pass"]["chat_wrote_turn_id"]
               or touched.get(chat) in APP_SOURCES)
    if trigger:
        views[chat]["curator"]["dismissed_at"] = now
        views[chat]["curator"]["led_until"] = 0
        state["pass"]["put_away"].append(chat)
    return state


def _chat_oid(views):
    for oid, v in views.items():
        if v.get("topic") == "chat" and not v.get("withdrawn"):
            return oid
    return None


def step6_score(state, now):
    """§ 4.14: score = `w × relevance × decay` with the zero rules; a `topic_dupe` window is
    not computed. § 4.15: decay and linger (see `decay_of`, `linger_of`). § 4.16: the bar is
    `bar` as the judge wrote it; while no verdict stands, `focus_default`, with and without
    judge; without a judge (`judge: off`: the hive has no judge cell) every weight is 0.5
    and the relevance chain of `relevance_of` runs without the verdict (Decision 16.09.)."""
    settings = state["settings"]
    verdict = state["judge"].get("verdict")
    if verdict is not None and verdict.get("bar") is not None:
        state["bar"] = float(verdict["bar"])
    else:
        state["bar"] = float(settings["focus_default"])
    if settings.get("judge") != "on":
        state["weights"] = {}
    for oid, v in state["views"].items():
        c = v["curator"]
        if not in_state(v):
            c["score"] = 0.0
            c["decay"] = decay_of(v, now, settings)
            continue
        c["decay"] = decay_of(v, now, settings)
        if zero_rule(v):
            c["score"] = 0.0
        else:
            c["score"] = round(weight_of(v, state["weights"]) * relevance_of(v, now) * c["decay"], 6)
    return state


def weight_of(view, weights):
    """§ 4.14: `w = weights[context]`, 0.5 for an unnamed context, unraised. § 4.16 per
    window: a window whose verdict a touch cleared (`verdict_cleared`) is weighted 0.5 as
    well, until the next judge run. `weights` is the same standing judgement as
    `judged_relevance` (§ 4.5) and was written for the situation the touch ended; clearing
    one half and keeping the other would judge the new situation with the old weight
    (Decision 18.09. 27). The bar is the screen's and is not touched (§ 4.16)."""
    if view["curator"].get("verdict_cleared"):
        return DEFAULT_WEIGHT                          # Decision 18.09. 27
    ctx = view.get("context")
    if ctx is None or ctx not in weights:
        return DEFAULT_WEIGHT
    return float(weights[ctx])


def relevance_of(view, now):
    """§ 4.14: `judged_relevance` while a verdict stands, else the hint `relevance`, else the
    default of `class`, else 0.5; after `relevant_until` at most 0.2."""
    judged = view["verdict"].get("judged_relevance")
    if judged is not None:
        r = float(judged)
    elif view.get("relevance") is not None:
        r = float(view["relevance"])
    elif view.get("class") in CLASS_RELEVANCE:
        r = CLASS_RELEVANCE[view["class"]]
    else:
        r = DEFAULT_RELEVANCE
    until = view.get("relevant_until")
    if isinstance(until, (int, float)) and now >= until:
        r = min(r, AFTER_UNTIL_CAP)
    return r


def put_away(view):
    """§ 4.14 zero rule: put away = `since ≤ dismissed_at` (with a put-away on record)."""
    c = view["curator"]
    at = c.get("dismissed_at") or 0
    return at > 0 and (c.get("since") or 0) <= at


def zero_rule(view):
    """§ 4.14: put away → 0; `state: hidden` or `judged_hidden` → 0."""
    return (put_away(view) or view.get("state") == "hidden"
            or view["verdict"].get("judged_hidden") is True)


def step7_rungs(state, now):
    """§ 4.17: each window declares its ladder (`layer_of`); § 4.18 takes the urgents out of
    both ladders first, then per ladder § 4.19–4.22.
    § 4.18: `state: urgent` → rung `urgent` whatever the score and whatever zero rule
    applies (Decision 16.09.). The front urgent is, among the urgents not put away, the
    highest score, tie younger `since`, then smaller id (R-23-2); a tapped urgent (§ 5.1)
    is front as long as `led_until > now` and no other urgent not put away carries a
    younger `since` — a younger touch on another urgent takes its precedence, after that
    the choice by score holds again (Decision 16.09.). Only the front urgent is open; every
    other urgent rings in its tile: rung `urgent`, level 0. A put-away urgent keeps rung
    `urgent`, is not front and keeps ringing in its tile; a window that becomes urgent after
    its put-away opens through the touch the app writes together with `state: urgent`
    (§ 4.8 c; the timer does, § 9.1): the finger puts away what it sees, not what comes
    after; `state` alone is no touch and lifts no put-away — a put-away urgent whose
    ringing ends stays put away (Decision 16.09.). An urgent competes on no ladder: while a
    window carries rung `urgent` its `layer` is not read for the ladders — a front urgent
    with `layer: modal` stands on level 3 and is no modal for § 4.21 and § 5.3; the one
    open modal may stand on level 2 beside it (Decision 16.09.). The tapped side of § 5.3
    does read the `layer`: an urgent with `layer: canvas` is a canvas app there (step 3,
    Decision 16.09. 19).
    § 4.19: while `led_until > now` the finger holds the window above the score: it leads
    (`focus`) as long as no other window of the same ladder carries a younger `since`; a
    younger touch of any source ends that lead — then § 4.20 chooses by score among all
    windows of the ladder, the held one included (with the highest score ≥ bar it stays
    `focus`) — and when the younger touch is a tap or a hold, that window leads; a held
    window that does not lead stays above the score until `led_until`: canvas `relevant`,
    modal `ambient` (at most one modal is open, § 4.21); after `led_until`, § 4.20
    (R-24-1; Decision 16.09.).
    § 4.20: focus = among the non-urgent windows with score ≥ bar and > 0, not `fresh` (and
    not `leaving`, step 12), the highest score, tie younger `since`, then smaller id
    (Decision 16.09.); only while nobody leads by § 4.19.
    § 4.21: every further canvas window with score ≥ bar and > 0 is `relevant`; on the modal
    ladder there is no `relevant`: at most one window is open (level 2), every further modal
    `ambient`; an urgent is on no ladder.
    § 4.22: `ambient` and `hidden` both mean "not open, tile only". `hidden` = a zero rule of
    § 4.14 applies; `ambient` = no zero rule, score below the bar or 0 — also 0 by decay —
    or a second modal. A score of 0 is never open and never `focus`, even at bar 0, unless
    the finger holds the window by § 4.19 (Decision 16.09.).
    § 4.23: a modal changes the rung of no canvas window; there is no stack — when the modal
    goes, there is nothing to restore (R-23-2)."""
    views = state["views"]
    bar = state["bar"]
    live = [o for o, v in views.items() if in_state(v)]
    for o in live:
        views[o]["curator"]["front"] = False

    # § 4.18 — urgents.
    urgents = [o for o in live if views[o].get("state") == "urgent"]
    for o in urgents:
        views[o]["curator"]["rung"] = "urgent"
    front = None
    cands = [o for o in urgents if not put_away(views[o])]
    if cands:
        youngest = max(views[o]["curator"]["since"] or 0 for o in cands)
        led = [o for o in cands if (views[o]["curator"]["led_until"] or 0) > now
               and (views[o]["curator"]["since"] or 0) == youngest]
        if led:
            front = sorted(led)[0]                     # two led at the same ms: smaller id  # Decision 16.09. 10
        else:
            front = _by_score(views, cands)[0]
    if front is not None:
        views[front]["curator"]["front"] = True

    # § 4.19–4.22 — per ladder.
    for ladder in LADDERS:
        members = [o for o in live if o not in urgents and layer_of(views[o]) == ladder]
        held = [o for o in members if (views[o]["curator"]["led_until"] or 0) > now]
        # A younger app touch ends the finger's lead only; then § 4.20 chooses by score
        # among all windows of the ladder, the held one included (the parenthetical of
        # § 4.19, not "takes the lead" read as "becomes focus").  # Decision 16.09. 16
        leader = None
        if held:
            youngest = max(views[o]["curator"]["since"] or 0 for o in members)
            leading = [o for o in held if (views[o]["curator"]["since"] or 0) == youngest]
            if leading:
                leader = sorted(leading)[0]            # § 4.19; equal `since`: smaller id  # Decision 16.09. 10
        if leader is None:
            pool = [o for o in members
                    if views[o]["curator"]["score"] >= bar and views[o]["curator"]["score"] > 0
                    and views[o]["curator"]["age"] not in ("fresh", "leaving")]
            if pool:
                leader = _by_score(views, pool)[0]     # § 4.20
        for o in members:
            v = views[o]
            c = v["curator"]
            if o == leader:
                c["rung"] = "focus"
            elif o in held:
                c["rung"] = "relevant" if ladder == "canvas" else "ambient"   # § 4.19
            elif zero_rule(v):
                c["rung"] = "hidden"                   # § 4.22
            elif ladder == "canvas" and c["score"] >= bar and c["score"] > 0:
                c["rung"] = "relevant"                 # § 4.21
            else:
                c["rung"] = "ambient"                  # § 4.21 second modal / § 4.22
    return state


def layer_of(view):
    """§ 4.17: `layer: canvas` (default) or `layer: modal`."""
    return "modal" if view.get("layer") == "modal" else "canvas"


def _by_score(views, oids):
    """§ 4.18/4.20 tiebreak: highest score, then the younger `since`, then the smaller id."""
    return sorted(oids, key=lambda o: (-views[o]["curator"]["score"],
                                       -(views[o]["curator"]["since"] or 0), o))


def step8_level(state):
    """§ 4.24: level = f(ladder, rung): the front urgent → 3; `modal` ∧ `focus` → 2;
    `canvas` ∧ (`focus` ∨ `relevant`) → 1; else 0. Systemwide, one value per window. Open =
    a window on level 1, 2 or 3 — rung `relevant` or `focus`, or the front urgent — and is
    drawn large on every output, in addition to its tile (Brief § 7; R-24-2). § 4.25 (blur,
    the OS level) is rendering and has no value in the state (§ 6)."""
    for oid, v in state["views"].items():
        c = v["curator"]
        if not in_state(v):
            c["level"], c["open"] = 0, False
            continue
        rung = c["rung"]
        if c.get("front"):
            level = 3
        elif layer_of(v) == "modal" and rung == "focus":
            level = 2
        elif layer_of(v) == "canvas" and rung in ("focus", "relevant"):
            level = 1
        else:
            level = 0
        c["level"] = level
        c["open"] = level > 0
    return state


def step9_dock(state, now):
    """§ 4.26: the dock is a column filled from the bottom. § 4.27: the rank per tile
    (`rank_of`); tie: the younger `since` stands higher, then the smaller id. § 4.28: from
    the bottom: seat tiles by `seat_ord` ascending (the clock's 0 at the very bottom, the
    weather's 10 above it), a seat without `seat_ord` at 0 (Decision 16.09. 22), equal
    `seat_ord`: the smaller id lower; above them the other tiles by rank, the highest rank
    on top (Brief § 5). § 4.29: a seat guarantees nothing; when its tile is missing, its place
    stays visibly empty — empty space, no placeholder object; no seat tile moves into the
    gap, no ranked tile slides into a seat (R-23-3); a seat is known as long as the view
    stands in the store, also when the app is not present or its tile was cut; when the app
    withdraws the view or its `ttl_ms` runs out, the seat is gone too (step 4;
    Decision 16.09.). § 4.30 (the cut per output) is `dock(state, screen)`; this step writes the
    systemwide order before any cut. § 4.26 (right edge, mark at the bottom, one tile
    size), § 4.31 (tile content) and § 4.32 (opacity) are rendering (§ 6)."""
    views = state["views"]
    settings = state["settings"]
    entries = []
    # Seats: known while the view stands in the store (also not present, also cut).
    seats = [(o, v) for o, v in views.items() if v.get("seat") == "bottom" and not v.get("withdrawn")]
    seats.sort(key=lambda ov: (ov[1].get("seat_ord") or 0, ov[0]))   # missing = 0; equal: smaller id lower  # Decision 16.09. 22
    for o, v in seats:
        if in_state(v):
            v["curator"]["rank"] = rank_of(v, state["weights"], now, settings)
            entries.append({"oid": o, "seat": True, "seat_ord": v.get("seat_ord") or 0,
                            "rank": v["curator"]["rank"], "pinned": v.get("pinned") is True,
                            "urgent": v["curator"]["rung"] == "urgent", "empty": False})
        else:
            v["curator"]["rank"] = 0.0
            entries.append({"oid": o, "seat": True, "seat_ord": v.get("seat_ord") or 0,
                            "rank": 0.0, "pinned": False, "urgent": False, "empty": True})
    ranked = [(o, v) for o, v in views.items() if in_state(v) and v.get("seat") != "bottom"]
    for o, v in ranked:
        v["curator"]["rank"] = rank_of(v, state["weights"], now, settings)
    top_down = sorted(ranked, key=lambda ov: (-ov[1]["curator"]["rank"],
                                              -(ov[1]["curator"]["since"] or 0), ov[0]))
    for o, v in reversed(top_down):
        entries.append({"oid": o, "seat": False, "seat_ord": None, "rank": v["curator"]["rank"],
                        "pinned": v.get("pinned") is True,
                        "urgent": v["curator"]["rung"] == "urgent", "empty": False})
    for o, v in views.items():
        if not in_state(v) and v.get("seat") != "bottom":
            v["curator"]["rank"] = 0.0
    state["dock_order"] = entries
    return state


def rank_of(view, weights, now, settings):
    """§ 4.27: rank = `max(w, 0.05) × relevance × decay` with `w`, `relevance` (including
    the verdict and the cap after `relevant_until`) and `decay` as in § 4.14–4.15, but
    without the zero rules of the score; rung `urgent` → 1. Weights below 0.05 are raised
    only here, so that the dock stays readable."""
    if view["curator"].get("rung") == "urgent":
        return 1.0
    w = max(weight_of(view, weights), RANK_FLOOR)
    # The formula, literally: a window present only by `relevant_until` with decay 0 has
    # rank 0 (compose.py reads RANK_FLOOR there; the description does not).  # Decision 16.09. 14
    return round(w * relevance_of(view, now) * decay_of(view, now, settings), 6)


def dock(state, screen):
    """§ 4.30: `dock_max` per output is the maximum number of drawn tiles, without exception
    (Decision 16.09.). Cut order: the other tiles by rank (lowest first), then urgent tiles (lowest rank
    first), then pinned tiles (lowest rank first), then seat tiles (highest `seat_ord`
    first); a tile falls in the latest stage that applies to it; at equal rank, what stands
    lower by § 4.28 falls (Decision 16.09.). What does not fit is missing on this output
    (R-23-6): the app stays present, its window stays open if it is open (R-24-2); on every
    other output with a tile it can be put away (§ 6). A cut seat tile leaves an empty seat
    (§ 4.29); an empty seat does not count against `dock_max`: drawn tiles are counted. The
    cut does not change the order: what remains stands by § 4.28. Returns the tiles
    bottom → top; an empty seat is `None`."""
    prof = state["screens"][screen]
    limit = prof["dock_max"]
    order = state["dock_order"]
    drawn = [e for e in order if not e["empty"]]
    n = len(drawn)
    falling = set()

    def stage(e):
        return 4 if e["seat"] else 3 if e["pinned"] else 2 if e["urgent"] else 1

    for st in (1, 2, 3, 4):
        if n <= limit:
            break
        pool = [e for e in drawn if stage(e) == st]
        if st == 4:
            pool.sort(key=lambda e: (-e["seat_ord"], -order.index(e)))
        else:
            pool.sort(key=lambda e: (e["rank"], order.index(e)))
        for e in pool:
            if n <= limit:
                break
            falling.add(e["oid"])
            n -= 1
    out = []
    for e in order:
        if e["empty"] or (e["oid"] in falling and e["seat"]):
            out.append(None)
        elif e["oid"] in falling:
            continue
        else:
            out.append(e["oid"])
    return out


def step10_unseen(state, now):
    """§ 4.33: `unseen`, systemwide and independent of the dock cut, counts the present apps
    whose window is not open and which either carry rung `urgent` or whose linger since the
    last touch still runs. Put-away windows do not count, a put-away urgent neither. No
    "seen" memory; the count falls by decay, opening or put-away (R-23-4). A leaving window
    is not present and does not count. Whether an output draws the dot: § 6.8."""
    n = 0
    for oid, v in state["views"].items():
        c = v["curator"]
        if not c.get("present") or c.get("open") or put_away(v):
            continue
        since = c.get("since") or 0
        if c["rung"] == "urgent" or now - since < linger_of(v, state["settings"]):
            n += 1
    state["unseen"] = n
    return state


def step11_strokes(state, now):
    """§ 4.34: the curator orders a stroke for the `ttl_ms` moment of every standing view and
    for every decay transition of every window in the state (end of the linger, end of
    `fade_ms`, `relevant_until`); no polling. After a pass in which a window carries `fresh`
    or `leaving`, also one second later: that stroke's pass makes the fresh window
    `settled`, and from then on it competes by § 4.20; the leaving window is gone after it
    (step 12; Decision 16.09.)."""
    settings = state["settings"]
    due = set()
    for oid, v in state["views"].items():
        c = v["curator"]
        ttl = v.get("ttl_ms") or 0
        if ttl > 0 and not v.get("withdrawn"):
            due.add(v["written_at"] + ttl)
        if c.get("age") in ("fresh", "leaving"):
            due.add(now + 1000)
        if not in_state(v):
            continue
        since = c.get("since")
        if since is not None:
            lg = linger_of(v, settings)
            due.add(since + lg)
            due.add(since + lg + settings["fade_ms"])
        until = v.get("relevant_until")
        if isinstance(until, (int, float)):
            due.add(int(until))
    state["strokes"] = sorted(t for t in due if t > now)
    return state


def step12_age(state, now):
    """§ 4.35: `age` is `fresh` in the pass in which a window appears — enters the store —
    and it takes no focus in that pass; `settled` afterwards; `leaving` in the last pass
    before the window leaves the state (decay, `ttl_ms`, withdrawal), so that the sheet can
    fade it out. Systemwide like every value. § 4.10: a standing view that was not present
    and receives a touch becomes present again without appearing anew: it carries
    `settled`, not `fresh` (Decision 16.09.). A leaving window stands in the state one last
    pass: it is computed, takes no focus, and does not count for `unseen`.  # Decision 16.09. 9"""
    # `settled` from the next pass on, whatever triggered it (§ 4.35 literal); the stroke
    # of § 4.34 only guarantees that such a pass comes within a second.  # Decision 16.09. 15
    new = set(state["pass"]["new"])
    for oid, v in state["views"].items():
        c = v["curator"]
        if c["present"]:
            c["age"] = "fresh" if oid in new else "settled"
        elif c.get("was_present"):
            c["age"] = "leaving"
        else:
            c["age"] = None
    return state


def in_state(view):
    """A window the pass computes: present, or in its leaving pass (§ 4.35)."""
    return view["curator"].get("present") or view["curator"].get("age") == "leaving"


def empty_state(settings=None, screens=None):
    """A screen state with no views: the settings of § 3 with their defaults, the profiles raw.

    The profiles pass the door (§ 4.7) in every pass, so a raw profile may be given here.
    """
    s = dict(DEFAULT_SETTINGS)
    s.update(settings or {})
    return {"settings": s, "screens": copy.deepcopy(screens or {}), "views": {},
            "bar": s["focus_default"], "weights": {}, "chat": {"last_turn_id": None},
            "unseen": 0, "strokes": [], "dock_order": [],
            "judge": {"last_call": None, "called": False, "verdict": None},
            "pass": {}}


def run_pass(state, event, now):
    """One pass (§ 4 head): steps 2–11 in their order; step 1 says when a pass runs and, at
    the end, whether the judge is called; step 12 names the values of `age` the steps read,
    so it is computed right after presence (step 4) and before anything reads it.
    Returns the new state; the input is not mutated."""
    s = copy.deepcopy(state)
    s["pass"] = {"now": now, "event": event.get("kind"), "runs": False, "touched": {},
                 "new": [], "refused": [], "errors": [], "chat_wrote_turn_id": False,
                 "topic_turn_ids": {}, "put_away": [], "co_closed": []}
    s = step1_triggers(s, event, now)
    if not s["pass"]["runs"]:
        return s
    s = step2_door(s, event, now)
    s = step3_touches(s, event, now)
    s = step4_presence(s, now)
    s = step12_age(s, now)
    s = step5_chat_closes(s, now)
    s = step6_score(s, now)
    s = step7_rungs(s, now)
    s = step8_level(s)
    s = step9_dock(s, now)
    s = step10_unseen(s, now)
    s = step11_strokes(s, now)
    s = step1_judge_call(s, now)
    return s


def judge_sees(state, now=None):
    """§ 4.4: per present window the owner, `context`, `class`, `topic`, `relevance` (the
    hint), its own last verdict, the rung, `age`, the age since `since`, `pinned`, whether it
    leads its ladder; on the state `bar` and `weights`; a glimpse of the text props (≤ 200
    characters); the last turn and the last answer, which the curator reads from the lines
    of the chat window (children, § 8.5): the newest turn and the newest answer among them,
    none while no chat window stands — the only children the curator reads for the judge
    (Decision 16.09.). Not a standing view that is not present (§ 4.11). No geometry: not
    the rank, not `tile`, not the level, not `dock_max`, not `dismissed_at`, nothing per
    output (R-23-6). What the Leitlinie asks beyond that (activity, interruption cost, the
    member's preferences, § 1.3) is judge input as soon as it exists; it is never geometry."""
    now = state["pass"].get("now") if now is None else now
    windows = []
    last_turn, last_answer = None, None
    for oid in sorted(state["views"]):
        v = state["views"][oid]
        c = v["curator"]
        if not c.get("present"):
            continue
        glimpse = " ".join(str(v.get(k) or "") for k in ("title", "kicker", "text")).strip()[:200]
        windows.append({"id": oid, "owner": v.get("owner"), "context": v.get("context"),
                        "class": v.get("class"), "topic": v.get("topic"),
                        "relevance": v.get("relevance"),
                        "verdict": dict(v["verdict"]), "rung": c["rung"], "age": c["age"],
                        "age_ms": (now - (c["since"] or now)) if now is not None else None,
                        "pinned": v.get("pinned") is True, "leads": c["rung"] == "focus",
                        "text": glimpse})
        if v.get("topic") == "chat":
            lines = (v.get("children") or {}).get("lines") or []
            for line in lines:
                if line.get("kind") == "turn":
                    last_turn = line.get("text")
                elif line.get("kind") == "answer":
                    last_answer = line.get("text")
    return {"bar": state["bar"], "weights": dict(state["weights"]), "windows": windows,
            "last_turn": last_turn, "last_answer": last_answer}


def _view(state, oid):
    return state["views"].get(oid)


def rung(state, oid):
    v = _view(state, oid)
    return v["curator"]["rung"] if v else None


def level(state, oid):
    v = _view(state, oid)
    return v["curator"]["level"] if v else None


def open_windows(state):
    return sorted(o for o, v in state["views"].items() if v["curator"].get("open"))


def present(state):
    return sorted(o for o, v in state["views"].items() if v["curator"].get("present"))


def in_store(state):
    return sorted(state["views"])


def unseen(state):
    return state["unseen"]


def led_until(state, oid):
    v = _view(state, oid)
    return v["curator"]["led_until"] if v else None


def dismissed_at(state, oid):
    v = _view(state, oid)
    return v["curator"]["dismissed_at"] if v else None


def since(state, oid):
    v = _view(state, oid)
    return v["curator"]["since"] if v else None


def score(state, oid):
    v = _view(state, oid)
    return round(v["curator"]["score"], 4) if v else None


def weight(state, oid):
    """The `w` of § 4.14 for one window: the judge's weight for its context, or the
    default 0.5 when the context is unnamed or a touch cleared the verdict
    (Decision 18.09. 27)."""
    v = _view(state, oid)
    return round(weight_of(v, state["weights"]), 4) if v else None


def decay(state, oid):
    v = _view(state, oid)
    return round(v["curator"]["decay"], 4) if v else None


def rank(state, oid):
    v = _view(state, oid)
    return round(v["curator"]["rank"], 4) if v else None


def age(state, oid):
    v = _view(state, oid)
    return v["curator"]["age"] if v else None


def topic_dupe(state, oid):
    v = _view(state, oid)
    return v["curator"]["topic_dupe"] if v else None


def verdict(state, oid):
    v = _view(state, oid)
    return dict(v["verdict"]) if v else None


def front_urgent(state):
    for o, v in state["views"].items():
        if v["curator"].get("front"):
            return o
    return None


def bar(state):
    return round(state["bar"], 4)


def weights(state):
    return dict(state["weights"])


def strokes_ordered(state):
    return list(state["strokes"])


def judge_called(state):
    return state["judge"]["called"]


def refused(state):
    return [list(r) for r in state["pass"].get("refused", [])]


def errors(state):
    return [list(e) for e in state["pass"].get("errors", [])]


def touched(state):
    return dict(state["pass"].get("touched", {}))


def last_turn_id(state):
    return state["chat"]["last_turn_id"]


def canvas_order(state):
    """§ 6.3: the open canvas windows on every output: the leading one first, then by score
    descending, then the younger `since`, then the id."""
    views = state["views"]
    opens = [o for o, v in views.items() if v["curator"].get("open") and layer_of(v) == "canvas"
             and v["curator"]["rung"] != "urgent"]
    return sorted(opens, key=lambda o: (views[o]["curator"]["rung"] != "focus",
                                        -views[o]["curator"]["score"],
                                        -(views[o]["curator"]["since"] or 0), o))


def inputs(state, screen):
    """§ 4.7 / § 6.4: the profile's `inputs` after the door."""
    return list(state["screens"][screen]["inputs"])


def tap_bound(state, screen):
    """§ 6.4: without `pointer` and without `touch` no tap binding, no binding of the mark."""
    ins = inputs(state, screen)
    return "pointer" in ins or "touch" in ins


def hold_bound(state, screen):
    """§ 6.4: a hold exists only with `audio` on a bound mark."""
    return tap_bound(state, screen) and "audio" in inputs(state, screen)


def input_line(state, screen):
    """§ 6.4 / § 7.3: the input line appears only with `keyboard` or `touch`."""
    ins = inputs(state, screen)
    return "keyboard" in ins or "touch" in ins


def judge_window_keys(state):
    """The keys of one window entry of § 4.4, to prove what the judge does not see."""
    seen = judge_sees(state)
    return sorted(seen["windows"][0].keys()) if seen["windows"] else []


def judge_window_ids(state):
    return [w["id"] for w in judge_sees(state)["windows"]]


def last_turn(state):
    return judge_sees(state)["last_turn"]


def last_answer(state):
    return judge_sees(state)["last_answer"]


def pass_ran(state):
    """§ 4.1: whether the event was a trigger of the closed list."""
    return bool(state["pass"].get("runs"))


def put_away_list(state):
    """The windows put away in this pass (§ 5.2, § 5.3, § 4.13)."""
    return list(state["pass"].get("put_away", []))


def co_closed(state):
    """The modals the modal-closing rule (§ 5.3) put away in this pass."""
    return list(state["pass"].get("co_closed", []))


def dock_default(state, screen):
    """§ 3 / § 6.1: whether the output ships the dock open (`shown`) or closed (`hidden`)."""
    return state["screens"][screen]["dock_default"]


def dock_max(state, screen):
    return state["screens"][screen]["dock_max"]
# --- The pass: display-hive.md § 4, model/pass.py verbatim (end) ---

# ---------------------------------------------------------------------------
# The display's own vocabulary
#
# Four components, defined by message on the bootstrap pass and stored as rows.
# The layer of each one is a decision the `web` cell ENFORCES rather than
# documents: glass is a navigation-layer material there, and a content
# component that writes `glass--thin` is refused at definition time.

# The canvas, as the display's OWN rule rather than a line in a token sheet:
# `vision.css` belongs to the `web` template and describes a design language,
# and so does the sheet below -- while "the canvas is one centred column and
# the dock floats beside it" is a statement about THIS screen. The rules
# travel in the shell, in the one `<style>` block, ahead of the sheet. Written
# with no two `{` adjacent, because `{{` is the component language's own marker.
#
# Since 2.3.0 there is no narrow column. `aside` is still accepted -- ambient
# and older senders say it, and refusing a word for nothing is a broken
# contract -- but it is drawn as canvas (OR-D3). WHERE the regions draw is the
# sheet's § 6.3 since 2.5.0, and it is there alone: this constant used to say
# `display: contents` for every region while the sheet said `display: grid`
# for the same one, and only the order inside the single `<style>` decided --
# a second stylesheet or a reordered concatenation would have flipped the
# layout without a word.
#
# What stays here is the BOX itself, because the box is a statement about this
# screen rather than about a design language: a column as tall as the output
# and no taller. The height is the load-bearing half of § 6.3: `overflow-y` on
# the canvas is a promise a box can only keep when its own height is bounded,
# and `body` (§ 5.9) is bounded, so this is the box that hands the bound on.
# Two declarations, no `@supports`, like `body`: an engine without `dvh` keeps
# the `vh` line. `border-box`, because the base sheet has no global reset and
# the dock's gutter is padding on this very box -- with `content-box` the
# column would be exactly that padding taller than the screen, which is how a
# page that "never scrolls" starts scrolling. `align-items: stretch` and not
# `center`, or the canvas would be a shrink-to-fit box and
# `repeat(3, minmax(0, 1fr))` would divide a width nobody set (the windows
# centre themselves, § 11).
LAYOUT_RULES = (
    ".display-columns { box-sizing: border-box; display: flex;"
    " flex-direction: column; align-items: stretch;"
    " block-size: 100vh; block-size: 100dvh;"
    " gap: var(--gap, 16px); }"
)

# The design language of the screen, byte-identical to `display-dna.css`
# beside this file. THAT FILE IS THE SOURCE; this constant is the copy a
# running cell renders, and a drift lock compares the two with the copy of this
# script inside `config.json`. A constant and not a file read, because a `code`
# cell runs out of `script_inline` and has no working directory to read a
# sheet from. The sheet is a raw string with no `"""` in it, and it is never
# minified: two `{` or two `}` side by side would be the component language's
# marker, and the lock refuses both.
KIT_CSS = r"""/* The display's design language -- meclaw display DNA v1 (kit 6).
 *
 * Source: `templates/display/compose/display-dna.css`, and there every rule is
 * argued for. THIS COPY MAY BE ONE OF THE TWO THAT TRAVEL, which carry the
 * rules and this comment and nothing else: `compose/compose.py` holds the sheet
 * in its `KIT_CSS` constant and `compose/config.json` holds that file again in
 * `params.script_inline`. `scripts/display_sync.py` writes both, and
 * `scripts/display_sheet_strip.py` is the rule by which they lose the
 * reasoning (OR-G0.1: about 47 kB off every page). A drift lock holds all
 * three together through that same rule
 * (`the_design_language_is_the_same_sheet_in_three_places`).
 *
 * So: a change is argued at the source, and the argument costs a screen
 * nothing. If you are reading this anywhere else, the sentences that say WHY
 * are in the file named above.
 *
 * The compose cell writes the constant into the `display-shell` template, last
 * in the shell's one style block, so a screen needs nothing installed beside it
 * and no hand ever sends a sheet to a running tree.
 *
 * It belongs to the TEMPLATE, not to the binary: a screen that should look
 * different costs a template edit, not a release.
 *
 * What it is: warm glass on a creme ground, one light, one accent, three
 * alphas of one ink, windows carried by a specular edge and two shadows and
 * nothing else. Every class is `display-<name>`; the three glass classes and
 * `.inner` are borrowed from `/vision.css` and keep their names. Loads AFTER
 * `/vision.css` and re-tokenises it to light: the same custom property names,
 * the DNA's values. The nine base components of the `web` template keep
 * working.
 *
 * Rules the sheet obeys: glass only on the navigation layer, never glass on
 * glass, text never on the material (always on `.inner`). Templates write
 * classes and data attributes, never colours. No external request and no
 * face: the two faces are an operator asset, declared from `params.font_base`
 * or not at all, and the fallback stacks in `--font-ui` and `--font-voice`
 * carry the type until then. Never minified: two `{` or two `}` side by side
 * are the component language's own marker.
 */

:root {
  color-scheme: light;

  --glass-tint: rgba(255, 251, 246, 0.62);
  --glass-tint-thin: rgba(255, 251, 246, 0.42);
  --glass-tint-thick: rgba(255, 251, 246, 0.78);
  --glass-blur: 18px;
  --glass-saturate: 140%;
  --glass-brightness: 1.02;
  --glass-focus: rgba(255, 251, 246, 0.8);
  --glass-opaque: #fffbf6;

  --inner-fill: transparent;
  --inner-fill-strong: rgba(255, 255, 255, 0.55);

  --fg-primary: rgba(43, 29, 25, 0.96);
  --fg-secondary: rgba(43, 29, 25, 0.82);
  --fg-tertiary: rgba(43, 29, 25, 0.55);

  --hairline: transparent;
  --hairline-strong: transparent;

  --accent: #b93a25;
  --accent-soft: #e8664f;
  --accent-wash: rgba(232, 102, 79, 0.14);

  --accent-ring: rgba(232, 102, 79, 0.5);

  --r-window: 22px;
  --r-pad: 8px;
  --r-inner: calc(var(--r-window) - var(--r-pad));
  --r-control: 12px;
  --r-chip: 14px;
  --r-capsule: 999px;
  --pad-window: 22px;
  --pad-inner: 16px;
  --gap: 16px;
  --gap-s: 8px;
  --gap-l: 24px;

  --rim-top: inset 0 1.5px 0 rgba(255, 255, 255, 1);
  --rim-bottom: 0 0 transparent;

  --sheen: linear-gradient(135deg, rgba(255, 255, 255, 0.55) 0%,
    rgba(255, 255, 255, 0.16) 26%, rgba(255, 255, 255, 0) 56%);

  --shadow-hue: 18 31% 23%;
  --shadow-strength: 10%;
  --shadow-contact: 0 1px 2px hsl(var(--shadow-hue) / calc(var(--shadow-strength) * 1.4)),
    0 3px 8px -4px hsl(var(--shadow-hue) / calc(var(--shadow-strength) * 1.8));
  --shadow-cast: 0 26px 52px -20px
    hsl(var(--shadow-hue) / calc(var(--shadow-strength) * 4));
  --shadow-spatial: var(--shadow-contact), var(--shadow-cast);
  --shadow-1: var(--shadow-spatial);
  --shadow-2: 0 2px 4px hsl(var(--shadow-hue) / calc(var(--shadow-strength) * 1.6)),
    0 44px 84px -26px hsl(var(--shadow-hue) / calc(var(--shadow-strength) * 5.5)),
    0 0 0 1px var(--accent-ring);

  --grain-opacity: 0.045;
  --grain: url("data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' width='120' height='120'><filter id='n'><feTurbulence type='fractalNoise' baseFrequency='0.85' numOctaves='2'/></filter><rect width='120' height='120' filter='url(%23n)'/></svg>");

  --font-ui: Inter, "SF Pro Text", "SF Pro", system-ui, -apple-system,
    "Segoe UI", Roboto, sans-serif;
  --font-voice: Fraunces, "Iowan Old Style", Georgia, serif;
  --w-body: 500;
  --w-title: 600;
  --t-caption: 12px;
  --t-small: 14px;
  --t-body: 16px;
  --t-title-3: 20px;
  --t-title-2: 24px;
  --t-title-1: 34px;
  --t-value: 34px;
  --t-voice: 30px;
  --leading-body: 1.5;
  --leading-title: 1.18;

  --ease: cubic-bezier(0.32, 0.72, 0, 1);
  --t-hover: 150ms;

  --t-dock: 140ms;
  --t-leave: 240ms;
  --t-enter: 360ms;
  --t-focus: 420ms;

  --hold-ms: 250ms;

  --ground: #f7efe6;
  --ground-2: #efe2d4;
  --bg-void: #f7efe6;

  --scale: 1;
  --type-scale: 1;
  --tile: calc(5.25rem * var(--scale));
  --os: calc(3.25rem * var(--scale));
  --dock-pad: calc(0.75rem * var(--scale));
  --dock-gap: calc(0.5rem * var(--scale));

  --tile-open-opacity: 0.88;

  --mark-dim: 0.5;

  --lead-min-phone: 48vh;
  --lead-min-phone: 48dvh;
  --sibling-max-phone: 40vh;
  --sibling-max-phone: 40dvh;
  --modal-max-phone: 72vh;
  --modal-max-phone: 72dvh;

  --rung-lift-scale: 1.02;
  --rung-scale: 1;

  --f-back-2: blur(4px);
  --f-back-3: blur(10px);

  --gutter: 28px;

  --plane-canvas: 5;
  --plane-modal: 20;
  --plane-urgent: 30;
  --plane-os: 40;
}

html {
  background-color: var(--ground);

  overscroll-behavior: none;
}

body {

  min-height: 100vh;
  min-height: 100dvh;

  height: 100vh;
  height: 100dvh;
  overflow: hidden;
  overscroll-behavior: none;
  margin: 0;
  background-color: var(--ground);
  background-image:
    radial-gradient(52% 48% at 76% 22%, rgba(246, 201, 176, 0.9) 0%, rgba(246, 201, 176, 0) 62%),
    radial-gradient(38% 34% at 8% 96%, rgba(226, 87, 63, 0.14) 0%, rgba(226, 87, 63, 0) 70%),
    linear-gradient(178deg, var(--ground) 0%, var(--ground-2) 100%);
  background-attachment: fixed;
  background-repeat: no-repeat;
  font-family: var(--font-ui);
  font-size: var(--t-body);
  font-weight: var(--w-body);
  line-height: var(--leading-body);
  letter-spacing: 0.004em;
  color: var(--fg-primary);
  -webkit-font-smoothing: antialiased;
  text-rendering: optimizeLegibility;
  animation: display-light-drift 30s ease-in-out infinite alternate;
}

@keyframes display-light-drift {
  from { background-position: 0 0, 0 0, 0 0; }
  to { background-position: 2vw -1vw, -2vw 1vw, 0 0; }
}

body::after {
  content: "";
  position: fixed;
  inset: 0;
  z-index: 0;
  pointer-events: none;
  opacity: var(--grain-opacity);
  background-image: var(--grain);
  mix-blend-mode: overlay;
}

.glass,
.glass--thin,
.glass--thick {
  box-shadow: var(--rim-top), var(--shadow-1);
}

.glass { --glass-blur: 18px; }
.glass--thin { --glass-blur: 14px; }
.glass--thick { --glass-blur: 22px; }

.inner {
  border-radius: var(--r-inner);
  padding: var(--pad-inner);
  display: flex;
  flex-direction: column;
  gap: var(--gap);
  background-color: var(--inner-fill);
  box-shadow: none;
}

.inner > .inner {
  box-shadow: none;
  background: none;
  padding: 0;
}

.table th,
.table td { border-bottom-color: var(--hairline); }
.table th { color: var(--fg-tertiary); }
.table thead { background-color: rgba(74, 46, 39, 0.06); }

.button {
  color: var(--fg-primary);
  background-color: rgba(255, 255, 255, 0.55);
  box-shadow: var(--rim-top), var(--shadow-contact);
}

.display-columns:is([data-inputs~="pointer"], [data-inputs~="touch"]) .button:hover {
  background-color: rgba(255, 255, 255, 0.78);
}

.input {
  color: var(--fg-primary);
  background-color: rgba(255, 255, 255, 0.5);
  border-color: var(--hairline-strong);
}

.badge {
  color: var(--fg-secondary);
  background-color: rgba(255, 255, 255, 0.5);
  box-shadow: none;
}

.button:focus-visible,
.input:focus-visible { outline-color: var(--accent); }

.title-1, .title-2, .title-3 {
  font-weight: var(--w-title);
  letter-spacing: -0.015em;
}

.title-1 { font-size: var(--t-title-1); line-height: 1.14; }
.title-2 { font-size: var(--t-title-2); line-height: 1.2; }
.title-3 { font-size: var(--t-title-3); line-height: 1.25; }

.text,
.text--secondary { color: var(--fg-secondary); }
.text--tertiary { color: var(--fg-tertiary); }

.display-pane,
.display-panel,
.display-overlay,
.display-ornament {
  position: relative;
  isolation: isolate;
  display: flex;
  flex-direction: column;
  border-radius: var(--r-window);
  padding: var(--r-pad);
  transition:
    transform var(--t-focus) var(--ease),
    opacity var(--t-focus) var(--ease),
    filter var(--t-focus) var(--ease),
    box-shadow var(--t-focus) var(--ease),
    background-color var(--t-focus) var(--ease);
}

:is(.display-pane, .display-panel, .display-overlay, .display-ornament)::before {
  content: "";
  position: absolute;
  inset: 0;
  z-index: -1;
  border-radius: inherit;
  background-image: var(--sheen);
  pointer-events: none;
}

.display-pane > .inner,
.display-panel > .inner,
.display-overlay > .inner {
  flex: 1 1 auto;
  gap: 10px;
}

.display-pane[data-region="aside"] {
  background-color: var(--glass-tint-thin);
  --glass-blur: 14px;
}

.display-pane-body,
.display-panel-body,
.display-stack {
  display: flex;
  flex-direction: column;
  gap: var(--gap);
  min-inline-size: 0;
}

.display-ornament-text,
.display-status-text { color: var(--fg-secondary); }

.display-kicker,
.display-pane-kicker,
.display-value-label,
.display-list-title,
.display-weather-place,
.display-clock-zone,
.display-timer-label,
.display-chat-title,
.display-notification-source,
.display-choice-label,
.display-progress-label,
.display-table-caption,
.display-document-source,
.display-table-grid thead th {
  margin: 0;
  font-size: var(--t-caption);
  font-weight: var(--w-title);
  line-height: 1.35;
  letter-spacing: 0.1em;
  text-transform: uppercase;
  color: var(--fg-tertiary);
}

.display-value-number,
.display-weather-temp,
.display-clock-time,
.display-timer-remaining {
  margin: 0;
  font-size: var(--t-value);
  font-weight: var(--w-title);
  line-height: 1.08;
  letter-spacing: -0.028em;
  font-variant-numeric: tabular-nums;
  color: var(--fg-primary);
}

.display-pane-title,
.display-panel-title,
.display-overlay-title,
.display-document-title,
.display-notification-title {
  margin: 0;
  font-size: var(--t-title-3);
  font-weight: var(--w-title);
  line-height: 1.25;
  letter-spacing: -0.014em;
  color: var(--fg-primary);
}

.display-list-items,
.display-weather-series,
.display-chat-lines,
.display-choice-options {
  margin: 0;
  padding: 0;
  list-style: none;
  display: flex;
  flex-direction: column;
}

.display-panel--scroll > .inner {
  max-block-size: 44vh;
  overflow-y: auto;
  -webkit-mask-image: linear-gradient(to bottom, transparent 0, #000 20px,
    #000 calc(100% - 20px), transparent 100%);
  mask-image: linear-gradient(to bottom, transparent 0, #000 20px,
    #000 calc(100% - 20px), transparent 100%);
}

.display-overlay {
  max-inline-size: 34rem;
}

.display-overlay[data-position="bottom"] {
  position: fixed;
  left: 50%;
  bottom: 96px;
  translate: -50%;
}

.display-overlay[data-position="center"] {
  position: fixed;
  left: 50%;
  top: 50%;
  translate: -50% -50%;
}

.display-overlay::after {
  content: "";
  position: absolute;
  left: var(--r-pad);
  right: var(--r-pad);
  bottom: 3px;
  block-size: 2px;
  border-radius: var(--r-capsule);
  background-color: var(--accent-soft);
  transform-origin: left center;
  animation: display-timer-drain calc(var(--ttl, 0) * 1ms) linear both;
}

.display-overlay-body { margin: 0; color: var(--fg-secondary); }

.display-overlay-actions,
.display-notification-actions {
  display: flex;
  flex-wrap: wrap;
  gap: var(--gap-s);
  align-items: center;

  flex: 0 0 auto;
}

.display-ornament {
  z-index: 10;
  align-self: center;
  margin-top: -20px;
  padding: 5px;
  border-radius: var(--r-capsule);
  background-color: var(--glass-tint-thin);
  --glass-blur: 14px;
}

.display-ornament > .inner {
  flex-direction: row;
  align-items: center;
  gap: 12px;
  padding: 6px 16px;
  border-radius: var(--r-capsule);
  font-size: var(--t-small);
  color: var(--fg-secondary);
  white-space: nowrap;
}

.display-ornament-dot,
.display-status-dot {
  inline-size: 8px;
  block-size: 8px;
  flex: none;
  border-radius: var(--r-capsule);
  background-color: var(--fg-tertiary);
}

.display-ornament-dot {
  background-color: var(--accent-soft);
  box-shadow: 0 0 0 4px var(--accent-wash);
}

.display-ornament-count {
  font-size: var(--t-caption);
  font-weight: var(--w-title);
  font-variant-numeric: tabular-nums;
  color: var(--fg-tertiary);
}

.display-stack--row {
  flex-direction: row;
  flex-wrap: wrap;
  align-items: flex-start;
}

.display-stack[data-gap="s"] { gap: var(--gap-s); }
.display-stack[data-gap="m"] { gap: var(--gap); }
.display-stack[data-gap="l"] { gap: var(--gap-l); }

.display-value,
.display-clock,
.display-timer,
.display-weather,
.display-chat,
.display-list,
.display-choice,
.display-document,
.display-progress {
  display: flex;
  flex-direction: column;
  gap: 6px;
  min-inline-size: 0;
}

.display-value-unit {
  margin-inline-start: 3px;
  font-size: 0.5em;
  letter-spacing: 0;
  color: var(--fg-tertiary);
}

.display-value[data-size="s"], .display-clock[data-size="s"] { --t-value: 24px; }
.display-value[data-size="l"], .display-clock[data-size="l"] { --t-value: 52px; }

.display-text {
  margin: 0;
  max-inline-size: 62ch;
  font-size: var(--t-body);
  line-height: 1.5;
  color: var(--fg-primary);
}

.display-text--secondary { color: var(--fg-secondary); }

.display-voice {
  margin: 0;
  max-inline-size: 30ch;
  font-family: var(--font-voice);
  font-variation-settings: "SOFT" 55, "WONK" 0, "opsz" 96;
  font-size: var(--t-voice);
  font-weight: 500;
  line-height: 1.22;
  letter-spacing: -0.012em;
  color: var(--fg-primary);
  text-wrap: balance;
}

.display-voice-accent {
  font-style: italic;
  font-weight: 400;
  color: var(--accent);

  padding-inline-end: 0.06em;
}

.display-voice[data-size="l"] { --t-voice: 40px; }

.display-item {
  display: flex;
  align-items: baseline;
  gap: 12px;
  padding: 8px 0;
  border-top: 1px solid var(--hairline);
  font-size: var(--t-small);
  line-height: 1.45;
}

.display-list-items > .display-item:first-child,
.display-weather-series > .display-item:first-child { border-top: 0; }
.display-list--plain .display-item { border-top: 0; padding: 3px 0; }

.display-item-marker {
  flex: none;
  min-inline-size: 2.4em;
  font-family: var(--font-voice);
  font-variation-settings: "SOFT" 55, "opsz" 72;
  font-size: 17px;
  font-weight: var(--w-title);
  color: var(--fg-tertiary);
}

.display-item-k { flex: 1 1 auto; min-inline-size: 0; color: var(--fg-primary); }

.display-item-v {
  flex: 0 1 auto;
  min-inline-size: 0;
  overflow-wrap: anywhere;
  font-variant-numeric: tabular-nums;
  color: var(--fg-secondary);
}
.display-item--accent .display-item-marker { color: var(--accent); }
.display-item--accent .display-item-k { font-weight: var(--w-title); }

.display-table { margin: 0; }

.display-table-grid {
  width: 100%;
  border-collapse: collapse;
  font-size: var(--t-small);
  font-variant-numeric: tabular-nums;
}

.display-table-caption { caption-side: top; text-align: left; margin-bottom: 10px; }

.display-table-grid th,
.display-table-grid td {
  text-align: left;
  padding: 7px 10px 7px 0;
  border-bottom: 1px solid var(--hairline);
}

.display-table-grid thead th {
  letter-spacing: 0.08em;
  white-space: nowrap;
  border-bottom-color: var(--hairline-strong);
}

.display-table-grid td:first-child { color: var(--fg-primary); }
.display-table-grid tbody tr:last-child td { border-bottom: 0; }

.display-weather-now {
  display: flex;
  align-items: center;
  justify-content: flex-start;
  gap: 16px;
  margin: 0;
}

.display-weather-temp {
  font-family: var(--font-ui);
  font-size: var(--t-value);
  line-height: 1;
  font-variant-numeric: tabular-nums;
  display: inline-flex;
  flex-direction: column;
  align-items: start;
}

.display-weather-unit {
  font-size: var(--t-small);
  font-weight: var(--w-body);
  color: var(--fg-tertiary);
}

.display-weather-glyph {
  order: 2;
  flex: none;
  font-size: calc(44px * var(--type-scale));
  line-height: 1;
}

.display-weather-condition,
.display-clock-date { margin: 0; font-size: var(--t-small); color: var(--fg-secondary); }

.display-weather-range {
  display: flex;
  gap: 10px;
  margin: 0;
  font-size: var(--t-small);
  font-variant-numeric: tabular-nums;
  color: var(--fg-secondary);
}

.display-weather-hi { color: var(--fg-primary); font-weight: var(--w-title); }
.display-weather-lo { color: var(--fg-tertiary); }

.display-weather-place {
  font-size: var(--t-caption);
  color: var(--fg-tertiary);
  letter-spacing: 0.06em;
  text-transform: uppercase;
}

.display-weather-range:not(:has(.display-weather-hi, .display-weather-lo)) { display: none; }

.display-timer-bar,
.display-progress-track {
  block-size: 6px;
  border-radius: var(--r-capsule);
  background-color: rgba(74, 46, 39, 0.1);
  overflow: hidden;
}

.display-timer-fill,
.display-progress-fill {
  display: block;
  block-size: 100%;
  inline-size: 100%;
  border-radius: inherit;
  background-color: var(--accent-soft);
  transform-origin: left center;
}

.display-chat-lines { gap: 8px; }

.display-chat-line {
  display: flex;
  flex-direction: column;
  gap: 2px;
  max-inline-size: 34ch;
  padding: 9px 14px;
  border-radius: var(--r-chip);
  font-size: var(--t-small);
  line-height: 1.45;
  color: var(--fg-primary);
}

.display-chat-line[data-role="you"] {
  align-self: flex-end;
  align-items: flex-end;
  background-color: var(--accent-wash);
  box-shadow: inset 0 0 0 2px rgba(74, 46, 39, 0.35);
  border-end-end-radius: 5px;
}

.display-chat-line[data-role="companion"] {
  align-self: flex-start;
  background-color: rgba(255, 255, 255, 0.55);
  box-shadow: inset 0 0 0 1px rgba(74, 46, 39, 0.08);
  border-end-start-radius: 5px;
}

.display-chat-line-text { margin: 0; }

.display-chat-line-meta {
  order: -1;
  display: flex;
  flex-wrap: nowrap;
  align-items: baseline;
  gap: 0.6ch;
}

.display-chat-line-meta:empty { display: none; }

.display-chat-line-source,
.display-chat-line-time {
  font-size: 11px;
  font-weight: var(--w-title);
  font-variant-caps: all-small-caps;
  letter-spacing: 0.06em;
  color: var(--fg-tertiary);
  white-space: nowrap;
}

.display-chat-line-time { font-variant-numeric: tabular-nums; }

.display-chat-line--partial .display-chat-line-text {
  color: var(--fg-secondary);
  font-style: italic;
}

.display-chat-line--partial .display-chat-line-text::after { content: "\2009…"; }

.display-input {
  display: flex;
  margin: 0;

  flex: 0 0 auto;
}

.display-input-field {
  flex: 1 1 auto;
  min-inline-size: 0;
  margin: 0;
  padding: 10px 14px;
  border: 0;
  border-radius: var(--r-control);
  background-color: var(--inner-fill-strong);
  box-shadow: var(--rim-top), var(--shadow-contact);
  font-family: var(--font-ui);

  font-size: max(16px, var(--t-body));
  font-weight: var(--w-body);
  line-height: 1.4;
  color: var(--fg-primary);
}

.display-input-field::placeholder { color: var(--fg-tertiary); }

.display-input-field:focus-visible {
  outline: 2px solid var(--accent);
  outline-offset: 2px;
}

.display-notification { display: flex; flex-direction: column; gap: 5px; }

.display-notification-title { font-size: var(--t-body); line-height: 1.3; }
.display-notification-body { margin: 0; font-size: var(--t-small); color: var(--fg-secondary); }

.display-notification-time {
  font-size: var(--t-caption);
  font-variant-numeric: tabular-nums;
  color: var(--fg-tertiary);
}

.display-notification[data-notice="urgent"] {

  box-shadow: inset 6px 0 0 var(--accent-soft),
    inset 0 0 0 1px rgba(74, 46, 39, 0.1),
    inset 0 1px 0 rgba(255, 255, 255, 0.9),
    var(--shadow-2);

  animation: display-notification-breathe 1000ms ease-in-out infinite;
}

@keyframes display-notification-breathe {
  0%, 100% {
    box-shadow: inset 4px 0 0 var(--accent-soft),
      inset 0 0 0 1px rgba(74, 46, 39, 0.1),
      inset 0 1px 0 rgba(255, 255, 255, 0.9),
      var(--shadow-2);
  }
  50% {
    box-shadow: inset 10px 0 0 var(--accent-soft),
      inset 0 0 0 1px rgba(74, 46, 39, 0.1),
      inset 0 1px 0 rgba(255, 255, 255, 0.9),
      var(--shadow-2);
  }
}

.display-notification[data-notice="urgent"] .display-notification-source { color: var(--accent); }

.display-media,
.display-chart { display: flex; flex-direction: column; gap: 8px; margin: 0; }

.display-media-frame {
  overflow: hidden;
  border-radius: var(--r-inner);
  background-color: var(--inner-fill-strong);
}

.display-media[data-ratio="16:9"] .display-media-frame { aspect-ratio: 16 / 9; }
.display-media[data-ratio="1:1"] .display-media-frame { aspect-ratio: 1 / 1; }

.display-media-image,
.display-media-frame > svg,
.display-chart-figure > svg { display: block; inline-size: 100%; block-size: auto; }

.display-media-image {
  block-size: 100%;
  object-fit: contain;
  border-radius: var(--r-inner);
}

.display-media[data-ratio] .display-media-frame > svg { block-size: 100%; object-fit: contain; }

.display-media-caption,
.display-chart-caption { font-size: var(--t-caption); color: var(--fg-tertiary); }

.display-document-body { font-size: var(--t-body); color: var(--fg-secondary); }
.display-document-body > * { margin: 0 0 10px; }
.display-document-body > *:last-child { margin-bottom: 0; }
.display-document-body strong { color: var(--fg-primary); font-weight: var(--w-title); }

.display-document-pages {
  margin: 0;
  padding-top: 8px;
  border-top: 1px solid var(--hairline);
  font-size: var(--t-caption);
  font-variant-numeric: tabular-nums;
  color: var(--fg-tertiary);
}

.display-document-total::before { content: " / "; }

.display-status {
  display: flex;
  align-items: center;
  gap: 8px;
  margin: 0;
  font-size: var(--t-small);
  color: var(--fg-secondary);
}

.display-status[data-kind="listening"] .display-status-dot {
  background-color: var(--accent-soft);
  box-shadow: 0 0 0 4px var(--accent-wash);
  animation: display-pulse 2.4s var(--ease) infinite;
}

.display-status[data-kind="speaking"] .display-status-dot {
  background-color: var(--accent);
  box-shadow: 0 0 0 4px var(--accent-wash);
}

.display-status[data-kind="thinking"] .display-status-dot {
  background-color: var(--fg-secondary);
  animation: display-pulse 1.6s var(--ease) infinite;
}

.display-status[data-kind="offline"] .display-status-dot {
  background-color: transparent;
  box-shadow: inset 0 0 0 1.5px var(--fg-tertiary);
}

@keyframes display-pulse {
  0%, 100% { opacity: 1; }
  50% { opacity: 0.45; }
}

.display-action,
.display-option-chip {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 8px;
  border: 0;
  font: inherit;
  font-size: var(--t-small);
  font-weight: var(--w-title);
  color: var(--fg-primary);
  background-color: rgba(255, 255, 255, 0.62);
  box-shadow: var(--rim-top), var(--shadow-contact);
  cursor: pointer;
  transition: filter var(--t-hover) var(--ease),
    transform var(--t-hover) var(--ease),
    background-color var(--t-hover) var(--ease);
}

.display-action { min-block-size: 40px; padding: 9px 18px; border-radius: var(--r-control); }

.display-option-chip {
  min-block-size: 36px;
  padding: 7px 14px;
  border-radius: var(--r-chip);
  font-weight: var(--w-body);
  color: var(--fg-secondary);
}

.display-columns:is([data-inputs~="pointer"], [data-inputs~="touch"]) .display-action:hover,
.display-columns:is([data-inputs~="pointer"], [data-inputs~="touch"]) .display-option-chip:hover {
  filter: brightness(1.06);
  transform: translateY(-2px);
}
.display-action:active,
.display-option-chip:active { transform: translateY(0) scale(0.985); }

.display-action:focus-visible,
.display-option-chip:focus-visible { outline: 2px solid var(--accent); outline-offset: 3px; }

.display-action--primary,
.display-option--selected > .display-option-chip {
  color: #2b1d19;
  font-weight: var(--w-title);
  background-color: var(--accent-soft);
  box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.45), var(--shadow-contact);
}

.display-choice-options { flex-direction: row; flex-wrap: wrap; gap: 8px; }
.display-option { display: inline-flex; }

.display-chart-figure { display: block; }

.display-progress-fill {
  inline-size: calc(var(--value, 0) * 1%);
  transition: inline-size var(--t-focus) var(--ease);
}

.display-list,
.display-table,
.display-chat,
.display-document,
.display-media,
.display-chart,
.display-choice,
.display-progress,
.display-weather,
.display-clock,
.display-timer,
.display-notification,
.display-status {
  padding: 14px 16px;
  border-radius: var(--r-inner);
  background-color: var(--inner-fill-strong);

  box-shadow: inset 0 0 0 1px rgba(74, 46, 39, 0.22),
    inset 0 1px 0 rgba(255, 255, 255, 0.8);
}

.display-media .display-media-frame { background-color: transparent; }
.display-table-grid thead { background-color: rgba(74, 46, 39, 0.06); }

.display-browser {
  display: flex;
  flex-direction: column;
  gap: 6px;
  margin: 0;
  min-inline-size: 0;
}

.display-browser-view {
  inline-size: 100%;
  block-size: auto;
  aspect-ratio: var(--browser-ratio, 16 / 10);
  border-radius: var(--r-inner);
  background-color: var(--inner-fill-strong);
  box-shadow: inset 0 0 0 1px rgba(74, 46, 39, 0.22);
  image-rendering: auto;
  touch-action: none;
}

.display-browser-bar {
  display: flex;
  gap: 8px;
  align-items: baseline;
  min-inline-size: 0;
  color: var(--fg-tertiary);
}

.display-browser-title {
  flex: 0 1 auto;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--fg-secondary);
}

.display-browser-url {
  flex: 1 1 auto;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: var(--t-caption);
}

.display-browser[data-page-state="loading"] .display-browser-view { opacity: 0.55; }

.display-browser[data-page-state="suspended"] .display-browser-view {
  opacity: 0.5;
  filter: grayscale(1);
}

.display-browser[data-page-state="error"] .display-browser-view,
.display-browser[data-page-link="down"] .display-browser-view {
  box-shadow: inset 0 0 0 2px var(--accent-ring);
}

.display-browser[data-page-state="error"] .display-browser-url,
.display-browser[data-page-link="down"] .display-browser-url { color: var(--accent); }

.display-browser-reason {
  flex: 0 1 auto;
  font-size: var(--t-caption);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--accent);
}

.display-browser-reason:empty { display: none; }

.display-browser-view:focus-visible {
  outline: 2px solid var(--accent);
  outline-offset: 2px;
}

.display-browser[data-keys="true"] .display-browser-view {
  outline: 2px solid var(--accent);
  outline-offset: 2px;
}

.display-browser-keys {
  position: fixed;
  inset-block-start: -100px;
  inset-inline-start: -100px;
  inline-size: 1px;
  block-size: 1px;
  padding: 0;
  border: 0;
  opacity: 0;
}

:where(.display-pane, .display-panel, .display-overlay)[data-rung="hidden"]:not([data-age="leaving"]) {
  display: none;
}

:is(.display-pane, .display-panel):where(:not([data-rung]), [data-rung=""]) {
  box-shadow: inset 0 0 0 2px var(--accent-soft), var(--rim-top), var(--shadow-2);
}

:where(.display-pane, .display-panel, .display-overlay)[data-rung="focus"] {
  --rung-scale: var(--rung-lift-scale);
  transform: translateY(-4px) scale(var(--rung-scale));
  opacity: 1;
  background-color: var(--glass-focus);
  box-shadow: inset 0 0 0 2px var(--accent-soft), var(--rim-top),
    var(--shadow-2);
}

:where(.display-pane, .display-panel, .display-overlay)[data-rung="urgent"] {
  --rung-scale: var(--rung-lift-scale);
  transform: translateY(-4px) scale(var(--rung-scale));
  opacity: 1;
  background-color: var(--glass-tint-thick);
  box-shadow: inset 0 0 0 4px var(--accent-soft), var(--rim-top), var(--shadow-2);

  animation: display-urgent-breathe 1000ms ease-in-out infinite;
}

@keyframes display-urgent-breathe {
  0%, 100% {
    box-shadow: inset 0 0 0 2px var(--accent-soft), var(--rim-top), var(--shadow-2),
      0 0 0 0 rgba(232, 102, 79, 0);
  }
  50% {
    box-shadow: inset 0 0 0 7px var(--accent-soft), var(--rim-top), var(--shadow-2),
      0 0 0 5px rgba(232, 102, 79, 0.35);
  }
}

[data-rung="focus"] :is(.display-pane-title, .display-panel-title) {
  font-size: var(--t-title-2);
  color: var(--fg-primary);
}

[data-rung="urgent"] :is(.display-pane-title, .display-panel-title) {
  font-size: var(--t-title-2);
  color: var(--accent);
  animation: display-title-breathe 1000ms ease-in-out infinite;
}

@keyframes display-title-breathe {
  0%, 100% { opacity: 1; }
  50% { opacity: 0.55; }
}

[data-tone="accent"] :is(.display-value-number, .display-weather-temp, .display-clock-time, .display-timer-remaining) {
  color: var(--accent);
}

[data-tone="muted"] .inner { color: var(--fg-secondary); }

.display-lead, .display-line, .display-detail { display: block; }

:where(.display-pane, .display-panel, .display-overlay)[data-level="1"] {
  z-index: var(--plane-canvas);
}

:where(.display-pane, .display-panel, .display-overlay)[data-level="2"] {
  z-index: var(--plane-modal);
}

:where(.display-pane, .display-panel, .display-overlay)[data-level="3"] {
  z-index: var(--plane-urgent);
}

.display-columns > [data-region] :is([data-level="2"], [data-level="3"]) {
  position: fixed;
  inset-block-start: 50%;
  inset-inline-start: 50%;
  translate: -50% -50%;
  inline-size: min(100% - 2 * var(--pad-window), clamp(22rem, 44vw, 40rem));
  margin-inline: 0;
}

.display-columns > [data-region] :is([data-level="1"], [data-level="2"], [data-level="3"]) {
  box-sizing: border-box;
  max-block-size: calc((100vh - 2 * var(--pad-window)) / var(--rung-scale));
  max-block-size: calc((100dvh - 2 * var(--pad-window)
    - env(safe-area-inset-top, 0px) - env(safe-area-inset-bottom, 0px))
    / var(--rung-scale));
}

.display-columns > [data-region] :is([data-level="1"], [data-level="2"], [data-level="3"]) :is(.inner, .display-pane-body, .display-panel-body) {
  min-block-size: 0;
}

.display-columns > [data-region] :is([data-level="1"], [data-level="2"], [data-level="3"]) :is(.display-pane-body, .display-panel-body) > :not(.display-input),
.display-columns > [data-region] :is([data-level="1"], [data-level="2"], [data-level="3"]) > .inner > .display-overlay-body {
  min-block-size: 0;
  overflow-y: auto;
  overscroll-behavior: contain;
}

.display-columns:has([data-level="2"]) [data-level="1"] {
  filter: var(--f-back-2);
  opacity: 0.72;
}

.display-columns:has([data-level="3"]) :is([data-level="1"], [data-level="2"]) {
  filter: var(--f-back-3);
  opacity: 0.5;
}

:where(.display-pane, .display-panel, .display-overlay)[data-age="fresh"] {
  animation: display-enter var(--t-enter) var(--ease) both;
}

:where(.display-pane, .display-panel, .display-overlay)[data-age="leaving"] {
  animation: display-leave var(--t-leave) var(--ease) both;
  pointer-events: none;
}

@keyframes display-enter {
  from { opacity: 0; transform: translateY(10px) scale(0.985); }
  to { opacity: 1; transform: none; }
}

@keyframes display-leave {
  from { opacity: 1; transform: none; filter: blur(0); }
  to { opacity: 0; transform: translateY(-6px); filter: blur(2px); }
}

@starting-style {
  :where(.display-pane, .display-panel, .display-overlay)[data-age="fresh"] {
    opacity: 0;
    transform: translateY(10px) scale(0.985);
  }
}

.display-timer {
  --start-at: calc(var(--end-at) - var(--total));
  --elapsed: calc(var(--now, var(--start-at)) - var(--start-at));
}

.display-timer-fill {
  animation:
    display-timer-drain calc(var(--total, 0) * 1ms) linear
      calc(var(--elapsed, 0) * -1ms) both,
    display-timer-heat calc(var(--total, 0) * 1ms) linear
      calc(var(--elapsed, 0) * -1ms) both;
}

.display-timer-remaining {
  animation: display-timer-alarm calc(var(--total, 0) * 1ms) linear
    calc(var(--elapsed, 0) * -1ms) both;
}

@keyframes display-timer-drain {
  from { transform: scaleX(1); }
  to { transform: scaleX(0); }
}

@keyframes display-timer-heat {
  0%, 82% { background-color: var(--accent-soft); }
  92%, 100% { background-color: var(--accent); }
}

@keyframes display-timer-alarm {
  0%, 88% { color: var(--fg-primary); }
  96%, 100% { color: var(--accent); }
}

.display-timer--done .display-timer-fill {
  animation: none;
  transform: scaleX(1);
  background-color: var(--accent);
}

.display-timer--done .display-timer-remaining { animation: none; color: var(--accent); }

::view-transition-group(*) {
  animation-duration: var(--t-focus);
  animation-timing-function: var(--ease);
}

.display-columns {
  position: relative;
  z-index: 1;
  --gap: 20px;
  padding: 0;

  padding-inline-end: calc(var(--tile) + 3 * var(--dock-pad) - var(--gutter));
  gap: 24px;
  font-family: var(--font-ui);
  font-size: 15px;
  font-weight: var(--w-body);
  line-height: 1.5;
  letter-spacing: 0.004em;
  color: var(--fg-primary);
}

.display-columns > [data-region="aside"] { display: contents; }

.display-columns > [data-region="aside"] :where(.display-pane, .display-panel, .display-overlay) {
  min-block-size: 0;

  margin-inline-start: var(--gutter);
}

.display-columns > [data-region="aside"]
  :where(.display-pane, .display-panel, .display-overlay):last-child {
  margin-block-end: var(--gutter);
}

.display-columns > [data-region="main"] {
  display: grid;
  padding: var(--gutter);
  grid-auto-rows: max-content;
  align-content: start;
  gap: var(--gap);
  flex: 1 1 auto;
  min-block-size: 0;
  overflow-y: auto;
  overscroll-behavior: contain;
}

.display-columns[data-exit="monitor"] > [data-region="main"] { grid-template-columns: repeat(3, minmax(0, calc((100% - 2 * var(--gap)) / 3))); }
.display-columns[data-exit="tv"] > [data-region="main"] { grid-template-columns: repeat(2, minmax(0, calc((100% - var(--gap)) / 2))); }
.display-columns[data-exit="phone"] > [data-region="main"] { grid-template-columns: minmax(0, 1fr); }

.display-columns[data-exit="phone"] > [data-region="main"] [data-level="1"][data-rung="focus"] {
  order: -1;
  min-block-size: var(--lead-min-phone);
}

.display-columns[data-exit="phone"] > [data-region="main"] [data-level="1"]:not([data-rung="focus"]) {
  max-block-size: var(--sibling-max-phone);
}

.display-columns[data-exit="phone"] > [data-region="main"] [data-level="2"] {
  max-block-size: var(--modal-max-phone);
}

.display-columns > [data-region] [data-level="0"] { display: none; }

.display-os {
  position: fixed;

  inset-inline-end: calc(var(--dock-pad) + env(safe-area-inset-right, 0px));
  inset-block-end: calc(var(--dock-pad) + env(safe-area-inset-bottom, 0px));
  inline-size: var(--os);
  block-size: var(--os);
  z-index: calc(var(--plane-os) + 1);
  background: transparent;
  touch-action: none;
}

.display-os-mark {
  display: block;
  inline-size: 100%;
  block-size: 100%;
  margin: 0;
  padding: 0;
  border: 0;
  background: transparent;
  color: var(--fg-secondary);
  cursor: pointer;
  touch-action: none;
  -webkit-user-select: none;
  user-select: none;
  transition: color var(--t-hover) var(--ease), filter var(--t-hover) var(--ease);
}

.display-os-glyph {
  display: block;
  inline-size: 100%;
  block-size: 100%;
}

.display-os-ring {
  fill: none;
  stroke: currentColor;
  stroke-width: 5;
  stroke-linecap: round;
}

.display-os-s {
  fill: none;
  stroke: currentColor;
  stroke-width: 4.5;
  stroke-linecap: round;
  stroke-linejoin: round;
}

.display-os-mark[aria-pressed="true"] { color: var(--accent); }

.display-os-mark:focus-visible {
  outline: 2px solid var(--accent);
  outline-offset: 3px;
  border-radius: var(--r-capsule);
}

.display-columns[data-inputs~="audio"] .display-os[data-phase="listening"] .display-os-mark {
  color: var(--accent);
  filter: drop-shadow(0 0 calc(0.5rem * var(--scale)) var(--accent-ring));
}

.display-os[data-phase="sending"] .display-os-mark { color: var(--accent-soft); }

.display-os[data-phase="speaking"] .display-os-mark {
  color: var(--accent-soft);
  animation: display-os-pulse 1400ms ease-in-out infinite;
}

.display-os[data-phase="error"] .display-os-mark { color: var(--fg-tertiary); }

.display-columns[data-inputs~="audio"] .display-os[data-phase="error"] .display-os-mark {
  opacity: var(--mark-dim);
}

html:not([data-dock-open]) .display-columns[data-dock="hidden"] .display-os[data-unseen]:not([data-unseen="0"]):not([data-unseen=""])::after,
html[data-dock-open="0"] .display-columns .display-os[data-unseen]:not([data-unseen="0"]):not([data-unseen=""])::after {
  content: "";
  position: absolute;

  pointer-events: none;
  inset-block-start: calc(var(--os) * 0.06);
  inset-inline-end: calc(var(--os) * 0.06);
  inline-size: calc(0.55rem * var(--scale));
  block-size: calc(0.55rem * var(--scale));
  border-radius: var(--r-capsule);
  background-color: var(--dot-fill, var(--accent));
  box-shadow: 0 0 calc(0.4rem * var(--scale)) var(--accent-ring);
}

@keyframes display-os-pulse {
  0%, 100% { opacity: 1; }
  50% { opacity: 0.55; }
}

.display-os-state {
  position: absolute;
  inline-size: 1px;
  block-size: 1px;
  margin: -1px;
  padding: 0;
  overflow: hidden;
  clip-path: inset(50%);
  white-space: nowrap;
}

.display-columns[data-inputs~="audio"] .display-os[data-phase="error"] .display-os-state {
  inset-block-end: calc((var(--os) - 1.3em) / 2);
  inset-inline-end: calc(100% + var(--dock-gap));
  margin: 0;
  inline-size: max-content;
  max-inline-size: calc(100vw - var(--os) - 2 * var(--dock-pad) - var(--dock-gap)
    - env(safe-area-inset-left, 0px) - env(safe-area-inset-right, 0px));
  block-size: auto;
  overflow: hidden;
  clip-path: none;
  font-size: var(--t-caption);
  line-height: 1.3;
  color: var(--fg-secondary);
  pointer-events: none;
  opacity: var(--mark-dim);
}

.display-scene {
  position: relative;
  isolation: isolate;
  align-items: flex-start;
  padding: 30px;
  overflow: hidden;
  border-radius: var(--r-window);
  background-color: var(--ground);
  background-image:
    radial-gradient(52% 48% at 76% 22%, rgba(246, 201, 176, 0.9) 0%, rgba(246, 201, 176, 0) 62%),
    radial-gradient(38% 34% at 8% 96%, rgba(226, 87, 63, 0.14) 0%, rgba(226, 87, 63, 0) 70%),
    linear-gradient(178deg, var(--ground) 0%, var(--ground-2) 100%);
  box-shadow: var(--shadow-1);
}

.display-scene[data-ratio="16:9"] {
  aspect-ratio: 16 / 9;
  inline-size: 100%;
  max-inline-size: 1280px;
  margin-inline: auto;
  align-items: center;
}

.display-scene > .display-pane,
.display-scene > .display-panel { flex: 1 1 0; min-inline-size: 0; }
.display-scene > .display-pane[data-region="aside"] { flex: 0 0 clamp(190px, 20%, 264px); }
.display-scene > .display-ornament { align-self: flex-end; margin: auto auto 0; }
.display-scene .display-voice { max-inline-size: 34ch; }

.display-dock {
  position: fixed;
  inset-block: 0;
  inset-inline-end: 0;
  z-index: var(--plane-os);
  display: flex;
  flex-direction: column;
  justify-content: flex-end;
  gap: var(--dock-gap);
  inline-size: calc(var(--tile) + 2 * var(--dock-pad));
  padding: var(--dock-pad);

  padding-block-end: calc(var(--os) + 2 * var(--dock-pad) + env(safe-area-inset-bottom, 0px));

  pointer-events: none;

  transition: opacity var(--t-dock) var(--ease), translate var(--t-dock) var(--ease),
    display var(--t-dock) allow-discrete;
}

.display-tile {
  position: relative;
  isolation: isolate;
  pointer-events: auto;
  display: grid;
  grid-template-rows: auto 1fr auto;
  gap: 2px;
  inline-size: var(--tile);
  aspect-ratio: 1;
  padding: calc(var(--dock-pad) * 0.8);
  border-radius: var(--r-control);

  background-color: var(--glass-tint-thick);
  -webkit-backdrop-filter: blur(14px) saturate(var(--glass-saturate));
  backdrop-filter: blur(14px) saturate(var(--glass-saturate));
  box-shadow: var(--rim-top), var(--shadow-contact);
  color: var(--fg-primary);
  overflow: hidden;
  transition:
    transform var(--t-focus) var(--ease),
    opacity var(--t-focus) var(--ease),
    box-shadow var(--t-focus) var(--ease);
}

.display-tile-glyph {
  font-size: var(--t-title-3);
  line-height: 1;
  color: var(--fg-secondary);
}

.display-tile-value {
  align-self: end;
  font-size: var(--t-title-2);
  font-weight: var(--w-title);
  line-height: 1.05;
  font-variant-numeric: tabular-nums;
  color: var(--fg-primary);
  max-inline-size: 100%;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.display-tile-line {
  font-size: var(--t-caption);
  line-height: 1.2;
  color: var(--fg-tertiary);
  max-inline-size: 100%;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.display-tile[data-rung="ambient"],
.display-tile[data-rung="hidden"] { opacity: var(--tile-open-opacity); }

.display-tile[data-rung="relevant"] { opacity: 1; }

.display-tile[data-rung="focus"] {
  opacity: 1;
  box-shadow: inset 0 0 0 2px var(--accent-soft), var(--rim-top), var(--shadow-contact);
}

.display-tile[data-rung="urgent"] {
  opacity: 1;
  background-color: var(--glass-tint-thick);
  box-shadow: inset 0 0 0 3px var(--accent), var(--rim-top), var(--shadow-contact);
  animation: display-urgent-breathe 1000ms ease-in-out infinite;
}

.display-tile[data-open="1"] { opacity: var(--tile-open-opacity); }

.display-seat {
  inline-size: var(--tile);
  aspect-ratio: 1;
  background: none;
  box-shadow: none;
  pointer-events: none;
}

.display-tile[data-pinned="1"]::after {
  content: "";
  position: absolute;
  inset-block-start: 6px;
  inset-inline-end: 6px;
  inline-size: 6px;
  block-size: 6px;
  border-radius: var(--r-capsule);
  background-color: var(--accent-soft);
}

.display-tile[data-zoomed="true"] {
  box-shadow: inset 0 0 0 2px var(--accent-soft), var(--rim-top), var(--shadow-2);
}

.display-tile[phx-click] { cursor: pointer; }
.display-tile[phx-click]:active { transform: scale(0.96); }

.display-tile[data-topic="chat"] .display-tile-glyph {
  font-size: var(--t-title-1);
  color: var(--accent);
}

.display-tile[data-topic="chat"] .display-tile-line {
  align-self: end;
  font-size: var(--t-small);
  color: var(--fg-secondary);
  white-space: normal;
  display: -webkit-box;
  -webkit-box-orient: vertical;
  -webkit-line-clamp: 2;
  line-clamp: 2;
  overflow: hidden;
}

.display-tile[data-unread="1"]::before {
  content: "";
  position: absolute;
  inset-block-start: 6px;
  inset-inline-start: 6px;
  inline-size: 7px;
  block-size: 7px;
  border-radius: var(--r-capsule);
  background-color: var(--accent);
}

.display-tile-unit {
  font-size: var(--t-caption);
  font-weight: var(--w-body);
  color: var(--fg-tertiary);
  margin-inline-start: 0.1em;
}

.display-columns > [data-region] :where(.display-pane, .display-panel, .display-overlay):not(:where(.display-pane, .display-panel, .display-overlay) *) {
  inline-size: 100%;
  max-inline-size: clamp(22rem, 44vw, 40rem);
  margin-inline: auto;
}

.display-columns > [data-region] > [data-view]:not(.display-pane, .display-panel, .display-overlay) { display: contents; }

.display-columns > [data-region] > [data-view] > .display-stack { display: contents; }

[data-ground="night"] {
  color-scheme: dark;
  --ground: #17100e;
  --ground-2: #241612;
  --bg-void: #17100e;
  --glass-tint: rgba(255, 246, 236, 0.085);
  --glass-tint-thin: rgba(255, 246, 236, 0.05);
  --glass-tint-thick: rgba(255, 246, 236, 0.13);
  --glass-focus: rgba(255, 246, 236, 0.13);
  --glass-saturate: 160%;
  --glass-brightness: 1.05;
  --glass-opaque: #2a1a16;
  --inner-fill: rgba(0, 0, 0, 0.16);
  --inner-fill-strong: rgba(0, 0, 0, 0.24);
  --fg-primary: rgba(247, 239, 230, 0.96);
  --fg-secondary: rgba(247, 239, 230, 0.72);
  --fg-tertiary: rgba(247, 239, 230, 0.46);
  --hairline: rgba(247, 239, 230, 0.13);
  --hairline-strong: rgba(247, 239, 230, 0.24);
  --accent: #f0876e;
  --accent-soft: #f0876e;
  --accent-wash: rgba(240, 135, 110, 0.18);
  --accent-ring: rgba(240, 135, 110, 0.28);
  --rim-top: inset 0 1px 0 rgba(255, 255, 255, 0.26);
  --rim-bottom: 0 0 transparent;
  --sheen: linear-gradient(135deg, rgba(255, 255, 255, 0.14) 0%,
    rgba(255, 255, 255, 0.04) 26%, rgba(255, 255, 255, 0) 56%);

  --shadow-hue: 20 32% 8%;
  --shadow-strength: 30%;
}

[data-ground="night"],
[data-ground="night"] .display-scene {
  background-color: var(--ground);
  background-image:
    radial-gradient(52% 48% at 72% 38%, rgba(240, 135, 110, 0.3) 0%, rgba(240, 135, 110, 0) 64%),
    radial-gradient(40% 36% at 10% 8%, rgba(246, 201, 176, 0.1) 0%, rgba(246, 201, 176, 0) 70%),
    linear-gradient(178deg, var(--ground) 0%, var(--ground-2) 100%);
}

[data-ground="night"] :is(.inner, .display-notification, .display-media-frame) {
  box-shadow: none;
}

[data-ground="night"] :is(.display-action, .display-option-chip, .button, .badge,
  .input, .display-chat-line[data-role="companion"]) {
  color: var(--fg-primary);
  background-color: rgba(255, 255, 255, 0.12);
  box-shadow: var(--rim-top);
}

[data-ground="night"] :is(.display-action--primary,
  .display-option--selected > .display-option-chip) {
  color: #1b1210;
  background-color: var(--accent-soft);
  box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.3);
}

[data-ground="night"] .display-browser-view {
  box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.12);
}

[data-ground="night"] :is(.display-timer-bar, .display-progress-track) {
  background-color: rgba(255, 255, 255, 0.12);
}

[data-ground="night"] .display-chat-line[data-role="you"] {
  background-color: var(--accent-wash);
}

.display-columns {
  --type-scale: var(--scale);
  --tile: calc(5.25rem * var(--scale));
  --os: calc(3.25rem * var(--scale));
  --dock-pad: calc(0.75rem * var(--scale));
  --dock-gap: calc(0.5rem * var(--scale));
  --t-caption: calc(12px * var(--type-scale));
  --t-small: calc(14px * var(--type-scale));
  --t-body: calc(16px * var(--type-scale));
  --t-title-3: calc(20px * var(--type-scale));
  --t-title-2: calc(24px * var(--type-scale));
  --t-title-1: calc(34px * var(--type-scale));
  --t-value: calc(34px * var(--type-scale));
  --t-voice: calc(30px * var(--type-scale));
  font-size: var(--t-body);
}

.display-columns[data-exit="tv"] {
  --fg-secondary: rgba(43, 29, 25, 0.92);
  --fg-tertiary: rgba(43, 29, 25, 0.78);
}

.display-columns[data-exit="tv"][data-ground="night"] {
  --fg-secondary: rgba(247, 239, 230, 0.9);
  --fg-tertiary: rgba(247, 239, 230, 0.74);
}

.display-columns:not([data-inputs~="audio"]) .display-os-mark { opacity: var(--mark-dim); }

.display-columns:not([data-inputs~="pointer"]):not([data-inputs~="touch"]) .display-os-mark {
  cursor: default;
}

.display-columns[data-exit="phone"] {

  --pad-window: 18px;
  --pad-inner: 14px;
  --r-window: 18px;
  --gutter: 14px;
  gap: 14px;
}

.display-columns[data-exit="phone"] > [data-region]
  :where(.display-pane, .display-panel, .display-overlay):not(:where(.display-pane, .display-panel, .display-overlay) *) {
  max-inline-size: 100%;
}

.display-columns[data-dock="hidden"] .display-dock,
html[data-dock-open="0"] .display-columns[data-dock="shown"] .display-dock {
  display: none;
  opacity: 0;
  translate: 12px 0;
}

.display-columns[data-dock="hidden"] {
  padding-inline-end: calc(var(--pad-window) - var(--gutter));
}

html[data-dock-open="1"] .display-columns[data-dock="hidden"] .display-dock {
  display: flex;

  opacity: 1;
  translate: none;
}

@starting-style {
  html[data-dock-open="1"] .display-columns[data-dock] .display-dock {
    opacity: 0;
    translate: 12px 0;
  }
}

html[data-dock-open="1"] .display-columns[data-dock="hidden"] {
  padding-inline-end: calc(var(--tile) + 3 * var(--dock-pad) - var(--gutter));
}

html[data-dock-open="0"] .display-columns[data-dock="shown"] {
  padding-inline-end: calc(var(--pad-window) - var(--gutter));
}

body:has([data-exit="tv"]) { animation: none; }

body:has([data-exit="tv"])::after { display: none; }

@supports not ((backdrop-filter: blur(1px)) or (-webkit-backdrop-filter: blur(1px))) {
  :is(.glass, .glass--thin, .glass--thick) { background-color: var(--glass-opaque); }
}

@media (prefers-reduced-transparency: reduce) {

  :is(.glass, .glass--thin, .glass--thick), .display-tile {
    background-color: var(--glass-opaque);
    -webkit-backdrop-filter: none;
    backdrop-filter: none;
  }

  :is(.display-pane, .display-panel, .display-overlay, .display-ornament)::before { display: none; }
  body { background-image: none; animation: none; }
  body::after { display: none; }

  .display-input-field {
    background-color: var(--glass-opaque);
    box-shadow: none;
  }

  .display-browser-view {
    background-color: var(--glass-opaque);
    box-shadow: none;
  }

  :root { --f-back-2: none; --f-back-3: none; }
}

@media (prefers-contrast: more) {
  :root {
    --fg-secondary: rgba(43, 29, 25, 0.92);
    --fg-tertiary: rgba(43, 29, 25, 0.76);
    --hairline: rgba(74, 46, 39, 0.3);
    --hairline-strong: rgba(74, 46, 39, 0.5);
    --inner-fill: rgba(255, 255, 255, 0.72);
    --inner-fill-strong: rgba(255, 255, 255, 0.86);
    --accent: #962a17;
    --f-back-2: none;
    --f-back-3: none;
  }

  [data-ground="night"] {
    --fg-secondary: rgba(247, 239, 230, 0.9);
    --fg-tertiary: rgba(247, 239, 230, 0.74);
  }

  :is(.glass, .glass--thin, .glass--thick) {
    background-color: var(--glass-opaque);
    -webkit-backdrop-filter: none;
    backdrop-filter: none;
  }
}

@media (forced-colors: active) {
  .display-pane,
  .display-panel,
  .display-overlay,
  .display-ornament,
  .display-notification,
  .display-action,
  .display-option-chip,
  .display-media-frame,
  .display-chat-line,
  .display-tile,
  .display-dock,
  .display-scene {
    background: Canvas;
    color: CanvasText;
    border: 1px solid CanvasText;
    box-shadow: none;
    -webkit-backdrop-filter: none;
    backdrop-filter: none;
    forced-color-adjust: none;
  }

  .display-action--primary,
  .display-option--selected > .display-option-chip { background: Highlight; color: HighlightText; }

  .display-ornament-dot,
  .display-status-dot,
  .display-timer-fill,
  .display-progress-fill { background: Highlight; }

  .display-os[data-unseen]:not([data-unseen="0"]):not([data-unseen=""])::after {
    --dot-fill: Highlight;
    forced-color-adjust: none;
  }

  :is(.display-pane, .display-panel, .display-overlay, .display-ornament)::before { display: none; }
  .display-scene, body { background-image: none; }
  .display-os-ring,
  .display-os-s { stroke: CanvasText; }
}

@media (prefers-reduced-motion: reduce) {
  body { animation: none; }

  .display-pane,
  .display-panel,
  .display-overlay,
  .display-ornament,
  .display-action,
  .display-option-chip,
  .display-progress-fill,

  .display-tile {
    transition: opacity 150ms linear !important;
    animation: none !important;
  }

  :where(.display-pane, .display-panel, .display-overlay)[data-rung="focus"],
  :where(.display-pane, .display-panel, .display-overlay)[data-rung="urgent"],
  .display-tile[phx-click]:active,
  .display-columns:is([data-inputs~="pointer"], [data-inputs~="touch"]) .display-action:hover,
  .display-columns:is([data-inputs~="pointer"], [data-inputs~="touch"]) .display-option-chip:hover { transform: none; }

  .display-dock { transition: none !important; }

  .display-os[data-phase="speaking"] .display-os-mark,
  .display-status .display-status-dot { animation: none !important; }

  :where(.display-pane, .display-panel, .display-overlay)[data-rung="urgent"],
  .display-tile[data-rung="urgent"] {
    animation: display-urgent-breathe 2000ms ease-in-out infinite !important;
  }
  [data-rung="urgent"] :is(.display-pane-title, .display-panel-title) {
    animation: display-title-breathe 2000ms ease-in-out infinite !important;
  }
  .display-notification[data-notice="urgent"] {
    animation: display-notification-breathe 2000ms ease-in-out infinite !important;
  }

  .display-timer-fill,
  .display-timer-remaining {
    animation-duration: calc(var(--total, 0) * 1ms) !important;
  }

  .display-overlay::after { animation-duration: calc(var(--ttl, 0) * 1ms) !important; }
}

@media (max-width: 80rem) {
  .display-scene:not(.display-columns *) { padding: 22px; }
}

@media (max-width: 48rem) {
  .display-scene:not(.display-columns *) {
    --pad-window: 18px;
    --pad-inner: 14px;
    --r-window: 18px;
    --t-voice: 24px;
    --t-value: 30px;
    aspect-ratio: auto;
    flex-direction: column;
    align-items: stretch;
    padding: 16px;
  }
  .display-scene:not(.display-columns *) > .display-pane[data-region="aside"] { flex: 1 1 auto; }
  .display-scene:not(.display-columns *) .display-ornament > .inner { white-space: normal; }
}
"""

# One style block, in this order: the layout rules, the faces (a prop, raw,
# empty unless the operator serves fonts), and the sheet LAST -- it
# re-tokenises `vision.css` at equal specificity and needs nothing stacked to
# win.
# The root carries the screen's own motion (`phx-hook="DisplayScene"`, the
# script in `client_js`, raw): the hook has to see the whole screen, because a
# window that left the canvas and the tile it went into are two elements one
# patch touches, and only somebody holding both ends can move one into the
# other. The script stands AFTER the shell's element and before the socket
# boot, in the dead render, the way the OS mark's does.
SHELL_TEMPLATE = (
    '{{#if stylesheet}}<link rel="stylesheet" href="vision.css">{{/if}}'
    "<style>" + LAYOUT_RULES + "{{&faces}}" + KIT_CSS + "</style>"
    + '<div class="stack display-columns" id="display-shell"'
    + ' phx-hook="DisplayScene" data-ground="{{ground}}"'
    + ' data-exit="{{exit}}" data-inputs="{{inputs}}"'
    + ' data-dock="{{dock}}" data-switch="{{switch}}"'
    + ' data-default="{{default_screen}}" data-screens="{{screens}}"'
    + ' data-screen-name="{{screen_name}}" style="--scale: {{scale}}">{{children}}</div>'
    + "<script>{{&client_js}}</script>"
)

REGION_TEMPLATE = '<div class="stack" data-region="{{region}}">{{children}}</div>'

# The prose view wears the catalogue: a thin `display-pane` outside, a
# `display-pane-title` for the title and a `display-text` for the paragraph
# inside, on the pane's `.inner` fill. It is a WINDOW (GH #679): the curator
# writes state, age, since and score on it, and the sheet reads them here.
# One title slot, no fixed kicker -- how loud the title is follows the rung.
PROSE_TEMPLATE = (
    '<section class="display-pane glass--thin" data-view="{{view_id}}"'
    ' data-owner="{{owner}}" data-rung="{{rung}}" data-age="{{age}}"'
    ' data-level="{{level}}" data-layer="{{layer}}" data-front="{{front}}"'
    ' data-led="{{led}}" data-pinned="{{pinned}}" data-topic="{{topic}}"'
    ' data-since="{{since}}" data-acted="{{acted}}" data-score="{{score}}"><div class="inner">'
    '{{#if title}}<h2 class="display-pane-title display-lead">{{title}}</h2>{{/if}}'
    '<div class="display-pane-body">'
    '<p class="display-text display-line">{{body}}</p></div></div></section>'
)

# A bare wrapper: `data-view` and `data-owner` are its identity, and it wears
# no class, because no sheet on this screen has a rule for one. What it looks
# like is the business of the tree hung inside it.
CUSTOM_TEMPLATE = (
    '<div data-view="{{view_id}}" data-owner="{{owner}}">{{children}}</div>'
)

# ---------------------------------------------------------------------------
# Catalogue A. Windows -- navigation layer, and the only glass in the catalogue.
# Taken from the kit that drew the design language, under the screen's own prefix.
#
# `glass{{#if thin}}--thin{{/if}}` renders as `glass` or as `glass--thin`. The
# language has no `else`, and the alternative -- two classes and a sheet rule
# that undoes one of them -- would put the material in two places. The class
# scanner in the `web` cell blanks every tag before it splits on whitespace, so
# what it sees here is the word `glass`, which is what this component means.

PANE_TEMPLATE = (
    '<section class="display-pane glass{{#if thin}}--thin{{/if}}" id="{{pane_id}}"'
    ' data-rung="{{rung}}" data-age="{{age}}"'
    ' data-since="{{since}}" data-acted="{{acted}}" data-score="{{score}}" data-tone="{{tone}}" data-pinned="{{pinned}}"'
    ' data-level="{{level}}" data-layer="{{layer}}" data-front="{{front}}"'
    ' data-led="{{led}}" data-topic="{{topic}}"'
    ' data-region="{{region}}"'
    ' style="view-transition-name: {{pane_id}}">'
    '<div class="inner">'
    '{{#if kicker}}<p class="display-pane-kicker display-line">{{kicker}}</p>{{/if}}'
    '{{#if title}}<h2 class="display-pane-title display-lead">{{title}}</h2>{{/if}}'
    '<div class="display-pane-body">{{children}}</div>'
    "</div></section>"
)

PANEL_TEMPLATE = (
    '<section class="display-panel glass{{#if scroll}} display-panel--scroll{{/if}}"'
    ' id="{{pane_id}}" data-rung="{{rung}}" data-age="{{age}}"'
    ' data-since="{{since}}" data-acted="{{acted}}" data-score="{{score}}" data-tone="{{tone}}" data-pinned="{{pinned}}"'
    ' data-level="{{level}}" data-layer="{{layer}}" data-front="{{front}}"'
    ' data-led="{{led}}" data-topic="{{topic}}" data-region="{{region}}"'
    ' style="view-transition-name: {{pane_id}}">'
    '<div class="inner">'
    '{{#if title}}<h2 class="display-panel-title display-lead">{{title}}</h2>{{/if}}'
    '<div class="display-panel-body">{{children}}</div>'
    "</div></section>"
)

# `--ttl` is the overlay's own countdown, spent by the sheet as an animation
# duration. The server never ticks: the browser owns elapsed time.
OVERLAY_TEMPLATE = (
    '<aside class="display-overlay glass--thick" id="{{pane_id}}"'
    ' data-rung="{{rung}}" data-age="{{age}}" data-since="{{since}}" data-acted="{{acted}}" data-score="{{score}}"'
    ' data-level="{{level}}" data-layer="{{layer}}" data-front="{{front}}"'
    ' data-led="{{led}}" data-topic="{{topic}}" data-pinned="{{pinned}}"'
    ' data-region="{{region}}"'
    ' data-position="{{position}}"'
    ' style="view-transition-name: {{pane_id}}; --ttl: {{ttl_ms}}">'
    '<div class="inner">'
    '{{#if title}}<h2 class="display-overlay-title display-lead">{{title}}</h2>{{/if}}'
    '{{#if body}}<p class="display-overlay-body display-line">{{body}}</p>{{/if}}'
    '<div class="display-overlay-actions">{{children}}</div>'
    "</div></aside>"
)

ORNAMENT_TEMPLATE = (
    '<nav class="display-ornament glass"><div class="inner">'
    '{{#if dot}}<span class="display-ornament-dot"></span>{{/if}}'
    '{{#if text}}<span class="display-ornament-text display-lead">{{text}}</span>{{/if}}'
    '{{#if count}}<span class="display-ornament-count">{{count}}</span>{{/if}}'
    "{{children}}</div></nav>"
)

# ---------------------------------------------------------------------------
# The dock (GH #695, spec § 2.4). A vertical column at the right edge, on its
# own layer above the canvas, holding one tile per PRESENT window ordered by
# rank. It is not a region: a region is a place on the canvas, and the dock is
# the answer to a different question -- "what is active at all" rather than
# "what is in focus". `count` and `profile` are copies the sheet and a reader
# can use without walking back up to the root.
DOCK_TEMPLATE = (
    '<div class="display-dock" id="display-dock" aria-label="dock"'
    ' data-count="{{count}}">{{children}}</div>'
)

# One tile. ONE size, always (D-1): a glyph, a value and a line, and the
# presence rung as a colour and a ring rather than as a size. `for` is the
# `pane_id` of the window it stands for, which is what lets the client match
# the two halves of one object for the zoom; `oid` is the window's OBJECT id
# and the only thing a tap has to carry, because the screen answers the tap
# itself (R-23-1). The binding is written only where the exit has a finger
# (`tap`, OR-F18): a television takes no taps, and a dead `phx-click` on it
# would be a promise the screen cannot keep. `data-end-at` is the epoch the
# seconds run to; the client writes the remainder into `[data-role=…]` once a
# second, and the server never ticks for it.
TILE_TEMPLATE = (
    '<div class="display-tile" data-for="{{for}}" data-rung="{{rung}}"'
    ' data-rank="{{rank}}" data-pinned="{{pinned}}"'
    ' data-open="{{open}}" data-topic="{{topic}}"'
    ' data-seat="{{seat}}" data-unread="{{unread}}"'
    ' data-end-at="{{end_at}}"'
    # Interpolated and not spelled out: the absorber listens for TAP_EVENT,
    # and a tile that shouted a name nobody opens would send the tap out of
    # the hive as an ordinary application event, silently.
    + ('{{#if tap}} phx-click="%s" phx-value-for="{{oid}}"{{/if}}>' % TAP_EVENT)
    + '{{#if glyph}}<span class="display-tile-glyph">{{glyph}}</span>{{/if}}'
    '{{#if value}}<span class="display-tile-value" data-role="remaining">{{value}}</span>{{/if}}'
    '{{#if unit}}<span class="display-tile-unit">{{unit}}</span>{{/if}}'
    '{{#if line}}<span class="display-tile-line">{{line}}</span>{{/if}}'
    "</div>"
)

# An empty seat (§ 4.29). A seat guarantees nothing: when its tile is missing its
# place stays visibly EMPTY -- empty space, not a placeholder. So the object carries no
# content at all and is hidden from a screen reader; what it does is hold the gap open,
# so no seat tile moves into it and no ranked tile slides into a seat (R-23-3).
SEAT_TEMPLATE = (
    '<div class="display-seat" data-seat-ord="{{seat_ord}}" aria-hidden="true"></div>'
)

# ---------------------------------------------------------------------------
# Catalogue B. Content -- everything that sits on a window's `.inner` fill.

VALUE_TEMPLATE = (
    '<div class="display-value" data-size="{{size}}" data-trend="{{trend}}">'
    '<p class="display-value-number display-lead">{{value}}'
    '{{#if unit}}<span class="display-value-unit">{{unit}}</span>{{/if}}</p>'
    '{{#if label}}<p class="display-value-label display-line">{{label}}</p>{{/if}}'
    "</div>"
)

TEXT_TEMPLATE = (
    '<p class="display-text display-line{{#if secondary}} display-text--secondary{{/if}}">'
    "{{body}}</p>"
)

# The one italic accent word of a spoken line. One per line, never two: the
# emphasis is the companion's, and two emphases are none.
VOICE_TEMPLATE = (
    '<p class="display-voice display-lead" data-size="{{size}}">{{text}}'
    '{{#if accent}} <em class="display-voice-accent">{{accent}}</em>{{/if}}</p>'
)

KICKER_TEMPLATE = '<p class="display-kicker display-line">{{text}}</p>'

LIST_TEMPLATE = (
    '<div class="display-list{{#if plain}} display-list--plain{{/if}}">'
    '{{#if title}}<h3 class="display-list-title display-lead">{{title}}</h3>{{/if}}'
    '<ul class="display-list-items">{{children}}</ul></div>'
)

ITEM_TEMPLATE = (
    '<li class="display-item display-line{{#if accent}} display-item--accent{{/if}}">'
    '{{#if marker}}<span class="display-item-marker">{{marker}}</span>{{/if}}'
    '{{#if k}}<span class="display-item-k">{{k}}</span>{{/if}}'
    '{{#if v}}<span class="display-item-v">{{v}}</span>{{/if}}</li>'
)

# A table has as many columns as it has, which the template language cannot
# say. So the two rows-of-cells props are raw HTML, composed by whoever sends
# the object -- and everything a model wrote has to be escaped on the way in.
TABLE_TEMPLATE = (
    '<figure class="display-table display-detail"><table class="display-table-grid">'
    '{{#if caption}}<caption class="display-table-caption">{{caption}}</caption>{{/if}}'
    "{{#if head}}<thead>{{&head}}</thead>{{/if}}"
    "<tbody>{{&rows}}</tbody></table></figure>"
)

WEATHER_TEMPLATE = (
    '<div class="display-weather">'
    '<p class="display-weather-now display-lead">'
    '{{#if glyph}}<span class="display-weather-glyph">{{glyph}}</span>{{/if}}'
    '<span class="display-weather-temp">{{temp}}'
    '{{#if unit}}<span class="display-weather-unit">{{unit}}</span>{{/if}}</span></p>'
    '{{#if condition}}<p class="display-weather-condition display-line">{{condition}}</p>{{/if}}'
    '<p class="display-weather-range display-line">'
    '{{#if hi}}<span class="display-weather-hi">{{hi}}</span>{{/if}}'
    '{{#if lo}}<span class="display-weather-lo">{{lo}}</span>{{/if}}'
    '{{#if place}}<span class="display-weather-place">{{place}}</span>{{/if}}</p>'
    '<ul class="display-weather-series display-detail">{{children}}</ul></div>'
)

CLOCK_TEMPLATE = (
    '<div class="display-clock" data-size="{{size}}">'
    '<p class="display-clock-time display-lead">{{time}}</p>'
    '{{#if date}}<p class="display-clock-date display-line">{{date}}</p>{{/if}}'
    '{{#if zone}}<p class="display-clock-zone display-detail">{{zone}}</p>{{/if}}</div>'
)

# `--end-at` is an epoch in milliseconds, `--total` the span it was set for and
# `--now` the moment the page was rendered -- the bar is a CSS animation with a
# negative delay computed from the three, so a page loaded late shows the right
# remainder without a tick. `data-end-at` is the same epoch for the client's
# own second: the sheet draws the bar, the hook writes the digits (D-27).
TIMER_TEMPLATE = (
    '<div class="display-timer{{#if done}} display-timer--done{{/if}}"'
    ' data-end-at="{{end_at}}"'
    ' style="--end-at: {{end_at}}; --total: {{total_ms}}; --now: {{now}}">'
    '<p class="display-timer-remaining display-lead" data-role="remaining">{{remaining}}</p>'
    '{{#if label}}<p class="display-timer-label display-line">{{label}}</p>{{/if}}'
    '<div class="display-timer-bar"><span class="display-timer-fill"></span></div></div>'
)

CHAT_TEMPLATE = (
    '<div class="display-chat">'
    '{{#if title}}<h3 class="display-chat-title display-lead">{{title}}</h3>{{/if}}'
    '<ol class="display-chat-lines">{{children}}</ol></div>'
)

# The line says two things about itself beside its text: WHERE it came from and
# WHEN it arrived (§ 8.4, R-26-1, owner ruling 18.09.). They share one row --
# `display-chat-line-meta` -- because "beside the source" is the sentence, and two
# order-swapped siblings in a column flex box would stand under each other.
#
# `at` travels RAW, as the epoch milliseconds of the line (§ 3.3, § 8.5), and the
# `<time>` element is empty: the screen state has no time zone and the device has
# one, so the client formats HH:MM for whoever is looking (`clocks()` in the scene).
# `{{#if at}}` keeps a line that carries no time from drawing an empty box -- such a
# line renders exactly as it did before the ruling.
CHAT_LINE_TEMPLATE = (
    '<li class="display-chat-line display-line'
    '{{#if partial}} display-chat-line--partial{{/if}}" data-role="{{role}}"'
    ' data-channel="{{channel}}">'
    '<p class="display-chat-line-text">{{text}}</p>'
    '<span class="display-chat-line-meta">'
    '{{#if source}}<span class="display-chat-line-source display-detail">{{source}}</span>{{/if}}'
    '{{#if at}}<time class="display-chat-line-time display-detail" data-at="{{at}}"></time>{{/if}}'
    '</span>'
    "</li>"
)

NOTIFICATION_TEMPLATE = (
    # `data-notice`, not `data-level`: since 2.5.0 `data-level` is the drawing
    # level of a WINDOW (§ 4.24), and one word with two meanings in one sheet
    # is what § 2 forbids. The prop stays `level`, because that is the word an
    # application already sends. Not `data-class` either: the shell carries the
    # whole sheet inline, and a scanner that looks for `class=` reads the value
    # of `data-class` in a selector as a class the catalogue writes.
    '<div class="display-notification" data-notice="{{level}}">'
    '{{#if source}}<p class="display-notification-source display-line">{{source}}</p>{{/if}}'
    '<h3 class="display-notification-title display-lead">{{title}}</h3>'
    '{{#if body}}<p class="display-notification-body display-detail">{{body}}</p>{{/if}}'
    '{{#if time}}<span class="display-notification-time display-detail">{{time}}</span>{{/if}}'
    '<div class="display-notification-actions display-detail">{{children}}</div></div>'
)

# `figure` is the way in for an inline SVG or a frame; `src` is the way in for a
# file the display can already reach. A specimen uses one or the other.
MEDIA_TEMPLATE = (
    '<figure class="display-media" data-ratio="{{ratio}}">'
    '{{#if figure}}<div class="display-media-frame">{{&figure}}</div>{{/if}}'
    '{{#if src}}<img class="display-media-image" src="{{src}}" alt="{{alt}}">{{/if}}'
    '{{#if caption}}<figcaption class="display-media-caption display-line">{{caption}}'
    "</figcaption>{{/if}}</figure>"
)

# A live page inside a window (display-hive.md § 7.9). It is CONTENT and not a
# fifth window object: a page is a `display-pane` with this as its child, so it
# stands beside `display-media` -- the same furniture, a frame with a caption
# under it -- and never beside `display-pane`.
#
# The canvas is the picture and the handles the scene hook reads off it:
# `data-page` is the topic suffix (`page:<page>`, OR-G3) and the one attribute
# the sweep looks for, `data-mount` names the browser cell the frames come from
# (the CURATOR fills it, see `add_tree`), and `data-viewport` is the page's own
# size in CSS pixels, so the frame has its proportions before the first image
# arrives. `tabindex="0"` is what lets a keyboard reach the picture at all.
#
# `role="img"` with the page's title as its label, because that is what a
# screen reader can honestly say about a screencast: a picture, named. The
# caption says the same thing in ink -- the title of the PAGE (the window's own
# `title` hint is the MODEL's title, and the two are different words on purpose,
# OR-G48) and the address under it.
#
# The state rides as `data-page-state` and NOT as `data-state`: § 2 struck
# `state` as a word of the curator (the step is `rung`, where a window is drawn
# is `level`), and a page's own loading state is a different thing entirely. One
# word, one meaning -- so this one gets its own word.
#
# Nothing here is raw and nothing is a link: the bytes on the canvas are an
# image, not markup an application wrote, and an address a viewer could follow
# would take them off this screen to the page's own site.
BROWSER_TEMPLATE = (
    '<figure class="display-browser" data-page-state="{{state}}">'
    '<canvas class="display-browser-view" data-page="{{page}}" data-mount="{{mount}}"'
    ' data-viewport="{{viewport}}" tabindex="0" role="img" aria-label="{{title}}">'
    "</canvas>"
    '<figcaption class="display-browser-bar display-line">'
    '{{#if title}}<span class="display-browser-title">{{title}}</span>{{/if}}'
    '<span class="display-browser-url">{{url}}</span>'
    # The client's own word about the join, and it ships EMPTY: no application
    # writes it, the hook does (contracts § 4 -- a refused join says "no
    # browser mounted"). An attribute the sheet reads is not an answer to a
    # person standing in front of an empty frame. `polite`, because a page
    # that cannot be shown is news, not an alarm.
    '<span class="display-browser-reason" data-role="reason"'
    ' aria-live="polite"></span></figcaption></figure>'
)

DOCUMENT_TEMPLATE = (
    '<article class="display-document">'
    '{{#if title}}<h2 class="display-document-title display-lead">{{title}}</h2>{{/if}}'
    '{{#if source}}<p class="display-document-source display-line">{{source}}</p>{{/if}}'
    '<div class="display-document-body display-detail">{{&body}}</div>'
    '{{#if pages}}<p class="display-document-pages display-detail">'
    "<span>{{page}}</span>"
    '<span class="display-document-total">{{pages}}</span></p>{{/if}}</article>'
)

STATUS_TEMPLATE = (
    '<p class="display-status display-lead" data-kind="{{kind}}">'
    '<span class="display-status-dot"></span>'
    '<span class="display-status-text">{{text}}</span></p>'
)

ACTION_TEMPLATE = (
    '<button class="display-action{{#if primary}} display-action--primary{{/if}}"'
    ' type="button"{{#if event}} phx-click="{{event}}"{{/if}}>{{label}}</button>'
)

CHOICE_TEMPLATE = (
    '<div class="display-choice">'
    '{{#if label}}<p class="display-choice-label display-line">{{label}}</p>{{/if}}'
    '<ul class="display-choice-options">{{children}}</ul></div>'
)

OPTION_TEMPLATE = (
    '<li class="display-option{{#if selected}} display-option--selected{{/if}}">'
    '<button class="display-option-chip" type="button"'
    '{{#if event}} phx-click="{{event}}"{{/if}}>{{label}}</button></li>'
)

# A line a person types into (contract § 8). `keyup` with `phx-key` and NOT a
# form: the LiveView client serialises a form to a URL-encoded string, and this
# scope reads object ids out of `event.value` -- a query string is not one, so
# a submitted form dead-letters (OR-F17). On `keyup` the client sends an object
# instead: every `phx-value-*` plus `value`, the text as it stands. `for` is
# filled by the SCREEN with the object id of the window the field stands in,
# the way a tile's `oid` is: the application cannot know that id, it is the
# index chain the tree walk mints.
#
# `enterkeyhint` is what makes a phone keyboard say "send"; emptying the field
# after Enter is the client's job (`DisplayScene`), because the server does
# not own what a person is in the middle of typing.
INPUT_TEMPLATE = (
    '<div class="display-input">'
    '<input class="display-input-field" type="text" autocomplete="off"'
    ' enterkeyhint="send" placeholder="{{placeholder}}"'
    '{{#if event}} phx-keyup="{{event}}" phx-key="Enter"'
    ' phx-value-for="{{for}}"{{/if}}>'
    "</div>"
)

CHART_TEMPLATE = (
    '<figure class="display-chart display-detail">'
    '<div class="display-chart-figure">{{&figure}}</div>'
    '{{#if caption}}<figcaption class="display-chart-caption display-line">{{caption}}'
    "</figcaption>{{/if}}</figure>"
)

# `scene` is the container the state rules need: the focus rule dims a pane's
# NEIGHBOURS, which is a statement about a group, and `:has()` needs an element
# to hang it on. It is deliberately NOT the way to put windows in a row -- a
# scene is a picture of a screen, with a screen's proportions and a screen's
# neighbour effect, and a row of specimens that each want to be seen on their own
# terms is a row and nothing more.
#
# `ratio` is what tells the sheet which kind of screen: a scene with no ratio
# takes its height from what is in it.
STACK_TEMPLATE = (
    '<div class="display-stack{{#if row}} display-stack--row{{/if}}'
    '{{#if scene}} display-scene{{/if}}" data-gap="{{gap}}"'
    ' data-ratio="{{ratio}}">{{children}}</div>'
)

PROGRESS_TEMPLATE = (
    '<div class="display-progress" data-ratio="{{value}}" style="--value: {{value}}">'
    '<div class="display-progress-track"><span class="display-progress-fill"></span></div>'
    '{{#if label}}<p class="display-progress-label display-line">{{label}}</p>{{/if}}</div>'
)

# The name of the `voice` cell this screen speaks to, from `params.voice_mount`.
# A module-level default, so a caller that says nothing gets the shipped name;
# the dispatcher below replaces it with what this cell was configured with, once
# per message. It is a MOUNT and not a path: the button joins `voice:<call>` on
# this page's own socket, and the `web` cell hands the frames to whichever cell
# holds that name in the process (GH #643).
VOICE_MOUNT = "voice"

# The name of the `browser` cell this screen shows pages from, from
# `params.browser_mount`. The same kind of value as `VOICE_MOUNT` and for the
# same reason: a mount is an operator's arrangement of one member's colony, and
# an application cannot know it. So the SCREEN writes it on every
# `display-browser` (see `add_tree`) and the client joins `page:<page>` with it,
# exactly as the microphone joins `voice:<call>`.
BROWSER_MOUNT = "browser"

# The four words a page may wear, and the ONLY four (wave G contracts 4). The
# cell knows seven states and the application maps them down -- `opening` to
# `loading`, `active`/`background`/`reopened`/`throttled` to `ready`,
# `suspended` to itself, everything else to `error`. Three of the four have a
# rule in the sheet (`KIT_CSS`, `data-page-state`), so the word is the whole
# of the difference between a frame that fills and a frame that failed.
#
# They are NOT `STATE_WORDS`: that pair is what an application may say about a
# WINDOW, and the two lists meet nowhere. Measured on the twin with a real page
# up (reports `g15.md`/`g16.md`): the guard in `add_tree` that keeps a window's
# word down to `urgent`/`hidden` ran on every node of the tree, so every one of
# these four was dropped on the way and the figure came out as
# `data-page-state=""` -- an `error` page looking exactly like a `ready` one.
PAGE_STATES = ("loading", "ready", "error", "suspended")

# Where the two faces are served from, from `params.font_base`. A URL prefix
# RELATIVE to the page's own base (the page carries `<base href>`), so an
# absolute path would break under the next proxy prefix and an `https://` would
# be an external request. Empty is the shipped default and means no
# `@font-face` at all: a set but unserved base sends two requests into nothing.
FONT_BASE = ""


def faces(font_base):
    """The two @font-face rules, or none at all.

    No font file ships in this repository: the faces are an operator asset and
    reach the page from a directory in front of the listener. An empty base is
    the default and means the fallback stacks in the sheet carry the type.
    """
    if not font_base:
        return ""
    # A directory, with or without its trailing slash: `fonts` and `fonts/`
    # both serve `fonts/inter.woff2`, and a base glued to a file name is not.
    if not font_base.endswith("/"):
        font_base += "/"
    return (
        '@font-face { font-family: "Inter"; font-style: normal;'
        " font-weight: 100 900; font-display: swap;"
        ' src: url("%sinter.woff2") format("woff2"); }'
        ' @font-face { font-family: "Fraunces"; font-style: normal;'
        " font-weight: 100 900; font-display: swap;"
        ' font-variation-settings: "SOFT" 55, "WONK" 0;'
        ' src: url("%sfraunces.woff2") format("woff2"); }'
    ) % (font_base, font_base)

# The OS mark (§ 2.6, D-2/D-3). A ring that opens to the right, and a
# stylised S standing in that opening with its outer edge exactly on the ring's
# outer edge, so the two read as »OS« (2.3.2; until 2.3.1 the ring opened at the
# top and the S stood in the gap, then right of the centre). The opening is just
# wide enough to leave a small gap around the S. Drawn
# in the current ink and nothing else: no card, no capsule, no fill -- it
# stands ON the canvas and the canvas shows through it (D-3). `phx-hook`
# keeps the name the client half has always had: the gesture did not change,
# only where the words go. What was heard does NOT stand beside the mark any
# more (D-17); the state line is left for a screen reader and the mark itself
# says its phase in light (`data-phase`) -- with the one exception the sheet
# makes for a refused channel, where § 5.4 asks the mark to SAY the refusal and
# the same line is drawn as a caption (OR-H2.1, GH #722). Detail-level text,
# and it says so itself (§ 7): what the line is does not depend on the rule
# that reads its phase.
OS_TEMPLATE = (
    '<div class="display-os" id="display-os" phx-hook="DisplayMic"'
    ' data-mount="{{mount}}" data-phase="" data-unseen="{{unseen}}">'
    '<button type="button" class="display-os-mark" aria-pressed="false"'
    ' aria-label="hold to talk">'
    '<svg class="display-os-glyph" viewBox="0 0 64 64" aria-hidden="true"'
    ' focusable="false">'
    '<path class="display-os-ring" d="M 47.1 16 A 22 22 0 1 0 47.1 48"></path>'
    '<path class="display-os-s" d="M 52.25 27 C 52.25 24.25 50 22 47.25 22'
    " C 44.5 22 42.25 24.25 42.25 27 C 42.25 29.75 44.5 32 47.25 32"
    " C 50 32 52.25 34.25 52.25 37 C 52.25 39.75 50 42 47.25 42"
    ' C 44.5 42 42.25 39.75 42.25 37"></path>'
    "</svg></button>"
    '<span class="display-os-state display-detail" data-role="state"'
    ' aria-live="polite"></span>'
    "</div><script>{{&client_js}}</script>"
)

# The browser half, in plain browser APIs: no library, no CDN, nothing installed.
#
# It rides in the component as a raw prop, the pattern `colony-view` uses, and
# that is not decoration: the script runs in the DEAD render, before the shell's
# socket constructor reads `window.SurfaceHooks`, and a morph never re-runs it. A
# hook registered any later would never be found.
#
# What it does: takes the socket the page already holds, joins `voice:<call>` on
# it, reads the `hello` push for the two rates it has to adapt to, cuts 20 ms
# PCM16 frames out of the microphone in an `AudioWorklet` and pushes them as
# binary with an empty `ref`, and schedules what comes back gap-free through an
# `AudioContext` at the rate that was declared. The counters on
# `window.__displayMic`, and the text of the last turn beside them, are there
# to be read by a test driving a real browser: since 2.3.0 the words are not
# written into the page any more (D-17), so the counter is where a proof
# reads them.
#
# Three things it does NOT hide. Without a secure context there is no microphone
# at all, and the button says so instead of failing silently; a REFUSED microphone
# says so too, because an unhandled rejection there left the button doing nothing
# and looking fine. And the worklet's decimation is nearest-sample: adequate for
# speech into a recogniser at 16 kHz from a 48 kHz device, and what the built-in
# test page does. It is not a filter.
#
# Three things it undoes. The playback context is built when `hello` names its rate,
# which is not a gesture -- so the first press resumes it, because a context that
# started suspended has a clock that does not run and the gap-free scheduling
# would schedule against it. And `destroyed()` gives every life back: the two
# window listeners, the microphone's tracks, both contexts and the channel.
# LiveView re-mounts a hook after a reconnect, and a wall screen reconnects all
# day.
#
# And the key does not cut the audio. `release` says that no NEW audio belongs to
# this take, not that the old audio is in: the last frames are still in the
# worklet's accumulator, in the message port's queue and in the device when the
# key comes up, and cutting there lost the last 300 to 600 ms of every take. So
# `up()` opens a drain window instead -- the worklet flushes what it has, whatever
# arrives for the next `drainMs` still goes out, and `release` is sent at the end
# of it. The number is measured rather than chosen: the larger of the capture
# latency the track reports and the context's own buffering, one worklet block,
# and the longest delivery gap this take saw, floored at 120 ms and capped at 600.
#
# Since 2.4.0 the press has a threshold. Under `HOLD_MS` it is a tap and means
# the dock (R-23-4); over it, it is a hold and means speech, and the hold says
# so to BOTH halves -- a `hold` frame to the `voice` cell and a `touch` event
# to `compose`, because holding the mark is where a dialogue starts (R-23-5).
# Two ways lead into the tap, and both are measured rather than assumed: Safari
# sends a `click` after `touchend`, and under `touch-action: none` with a
# captured pointer the `pointerup` can go missing entirely.
OS_CLIENT_JS = (
    "(function (root) {\n"
    # A duration out of the sheet's own tokens, so a quantity is the
    # template's decision and not this script's (display-hive.md § 2). The
    # same six lines stand in the scene hook: the two hooks are separate
    # scripts on the page and share no scope.
    "  function dur(el, name, fallback) {\n"
    "    var v = root.getComputedStyle(el).getPropertyValue(name).trim();\n"
    "    var n = parseFloat(v);\n"
    "    return isNaN(n) ? fallback : (v.indexOf(\"ms\") > -1 ? n : n * 1000);\n"
    "  }\n"
    "  var hook = {\n"
    "    mounted: function () {\n"
    "      var el = this.el, mount = el.dataset.mount || \"voice\";\n"
    "      var st = { sent: 0, flushed: 0, drainMs: 0, played: 0, turns: 0, speakEnd: 0, hello: null, code: null,\n"
    "                 taps: 0, touches: 0, holdMs: 0, prebuffered: 0, setupMs: 0, refused: 0,\n"
    "                 holdsRefused: 0, bound: false, audio: false, phase: \"\", said: \"\" };\n"
    "      root.__displayMic = st;\n"
    "      var state = el.querySelector('[data-role=\"state\"]'), btn = el.querySelector(\"button\");\n"
    "      var cols = document.querySelector(\".display-columns\");\n"
    "      // The handle `updated()` reaches for. Taken BEFORE the switch\n"
    "      // returns below: a page that is about to replace itself renders\n"
    "      // patches too, and a hook with no handle answers none of them.\n"
    "      this.__mic = { st: st, repaint: repaint };\n"
    "      // What this output can take (display-hive.md § 6.4). The profile\n"
    "      // says it, the door normalises it, the root carries it -- and the\n"
    "      // client binds NOTHING it does not name. A television is an output\n"
    "      // device (`inputs: []`): a press it cannot receive, a microphone it\n"
    "      // cannot open and an event for a finger that does not exist are\n"
    "      // three promises a screen must not make.\n"
    "      // § 6.5: this page is the switch and is about to replace itself.\n"
    "      // Binding a listener, opening a socket and joining a voice channel\n"
    "      // for the half second before `location.replace` is exactly what the\n"
    "      // scene hook returns early to avoid -- and the mark is a script of\n"
    "      // its own, so it asks the same question again.\n"
    "      if (cols && cols.getAttribute(\"data-switch\") === \"1\") { phase(\"idle\"); return; }\n"
    "      var inputs = ((cols && cols.getAttribute(\"data-inputs\")) || \"\").split(/\\s+/);\n"
    "      var finger = inputs.indexOf(\"pointer\") > -1 || inputs.indexOf(\"touch\") > -1;\n"
    "      var audio = inputs.indexOf(\"audio\") > -1;\n"
    "      st.bound = finger; st.audio = audio;\n"
    "      if (!finger) { phase(\"idle\"); return; }\n"
    "      // The threshold (display-hive.md § 5.5: \"a token, 250 ms\"). It is\n"
    "      // READ, not written here: the sheet is where a quantity lives (§ 2),\n"
    "      // and a script with its own copy is a screen with two thresholds.\n"
    "      var HOLD_MS = dur(cols || el, \"--hold-ms\", 250);\n"
    "      // The dock toggle, and it is the whole of what a tap means (R-23-4,\n"
    "      // OR-F5). Two attributes, two owners: the GROUND state is the\n"
    "      // server's (`data-dock` on the root, out of the profile, in the dead\n"
    "      // render), the OPENING is this line. <html> is outside the LiveView\n"
    "      // container, so no patch overwrites it, and a reload puts the screen\n"
    "      // back into its profile default -- which is exactly \"the toggle is\n"
    "      // not remembered\". Nothing is pushed: a gesture with no semantics has\n"
    "      // nothing to tell the colony, and an iteration that pushed one\n"
    "      // anyway paid a full curation pass per tap, measured on a device.\n"
    "      // Which way the gesture that is running already ended. Safari sends\n"
    "      // a `click` after `touchend`, and under `touch-action: none` with a\n"
    "      // captured pointer it is the `pointerup` that can go missing -- so\n"
    "      // both ways are wired and the SECOND one is dropped. Not by a span\n"
    "      // of time: a 700 ms window was a number outside the sheet (§ 2) and\n"
    "      // a browser state that decides (§ 3.2). Debouncing one physical\n"
    "      // gesture carries no meaning; asking the clock would.\n"
    "      var hookSelf = this, gesture = \"\";\n"
    "      function tapDock() {\n"
    "        var html = document.documentElement;\n"
    "        var open = html.getAttribute(\"data-dock-open\");\n"
    "        // The FIRST tap has no opening to flip, and assuming \"closed\"\n"
    "        // made the mark a one-way switch: on an exit whose profile ships\n"
    "        // the dock open, that first tap wrote the \"1\" it already was and\n"
    "        // nothing moved (GH #705). So it starts from the GROUND state the\n"
    "        // server rendered on the columns, and one tap always means the\n"
    "        // other one -- whatever the profile ships.\n"
    "        if (open !== \"1\" && open !== \"0\") {\n"
    "          open = cols && cols.getAttribute(\"data-dock\") === \"hidden\" ? \"0\" : \"1\";\n"
    "        }\n"
    "        html.setAttribute(\"data-dock-open\", open === \"1\" ? \"0\" : \"1\");\n"
    "        st.taps++;\n"
    "      }\n"
    "      // The mark's two marks: the attribute the sheet draws by, and the\n"
    "      // line a screen reader hears. Both are kept HERE as well, because\n"
    "      // the server renders an empty phase and an empty line and every\n"
    "      // patch would otherwise take them back (GH #720).\n"
    "      function phase(p) { st.phase = p; el.setAttribute(\"data-phase\", p); }\n"
    "      function say(t) { st.said = t; if (state) state.textContent = t; }\n"
    "      // After a morph: put both back, and only what the morph actually\n"
    "      // took. Writing the same words into an `aria-live` region again is\n"
    "      // an announcement a person hears twice. The line is re-queried\n"
    "      // rather than remembered: morphdom may have replaced the span.\n"
    "      function repaint() {\n"
    "        if (el.getAttribute(\"data-phase\") !== st.phase) el.setAttribute(\"data-phase\", st.phase);\n"
    "        var s = el.querySelector('[data-role=\"state\"]');\n"
    "        if (s && s.textContent !== st.said) s.textContent = st.said;\n"
    "      }\n"
    "      // What ends a refusal: a NEW press, and a microphone that opened\n"
    "      // after all. Never a clock -- a dim that times out would be a\n"
    "      // quantity outside the sheet (§ 2). What is left standing is what\n"
    "      // is still true about this screen: a refused CHANNEL is the\n"
    "      // socket's word and outlives the gesture, a refused DEVICE does\n"
    "      // not -- the person may have said yes in the meantime.\n"
    "      function clearMark() { phase(refused ? \"error\" : \"\"); say(refused ? refusal : idleText()); }\n"
    "      var socket = root.SurfaceSocket && root.SurfaceSocket.getSocket && root.SurfaceSocket.getSocket();\n"
    "      var call = \"\", topic = \"\", chan = null, joined = false, joinWait = null;\n"
    "      // A refused join is an answer about SPEECH and about nothing else.\n"
    "      // It used to be kept on the button as its `disabled` property, and a\n"
    "      // disabled control dispatches no pointer events at all -- so the dock\n"
    "      // toggle died with the voice cell, although a toggle is pure\n"
    "      // presentation and owes the colony nothing (R-23-4, OR-F5). On a\n"
    "      // screen that has a voice cell it never showed; after a restart of\n"
    "      // that cell the mark stayed dead until a reload, and on the phone the\n"
    "      // dock is the only way to the tiles. So the refusal lives here, where\n"
    "      // only the half that needs a channel reads it.\n"
    "      var refused = false, refusal = \"\";\n"
    "      // What a refusal does, in one place: it is SAID -- in the live region\n"
    "      // a screen reader hears and in the phase the sheet dims the mark by --\n"
    "      // and it is never announced as `aria-disabled`. The mark still answers\n"
    "      // a finger, and a control that answers must not tell assistive\n"
    "      // technology that it does not.\n"
    "      function refuseFrom(why) { refusal = why; say(why); phase(\"error\"); refused = true; }\n"
    "      // No socket at all -- the same question the refused join asks, one\n"
    "      // layer lower. Leaving here would take every listener below with it,\n"
    "      // including the two that carry the dock toggle, so it is answered the\n"
    "      // same way: the speech half is refused, the screen half is wired.\n"
    "      if (!socket) refuseFrom(\"no socket\");\n"
    "      var ctx = null, mctx = null, worklet = null, stream = null, playAt = 0;\n"
    "      function frame(obj) { if (chan) chan.push(\"frame\", obj); }\n"
    "      function onFrame(f) {\n"
    "        if (f.type === \"hello\") {\n"
    "          st.hello = f;\n"
    "          // A rejoin says hello again. Closing the old context first is what keeps\n"
    "          // a screen that reconnects all day from running into the browser's own\n"
    "          // cap on how many a page may have.\n"
    "          if (ctx) { try { ctx.close(); } catch (e) { /* already closed */ } }\n"
    "          ctx = new (root.AudioContext || root.webkitAudioContext)({ sampleRate: f.audio_out ? f.audio_out.sample_rate : 24000 });\n"
    "          say(idleText());\n"
    "        }\n"
    "        else if (f.type === \"partial\") { /* the words live in the chat application now (D-18) */ }\n"
    "        else if (f.type === \"turn\") { st.turns++; st.text = f.text; phase(\"\"); }\n"
    "        else if (f.type === \"speak_start\") { playAt = 0; phase(\"speaking\"); }\n"
    "        else if (f.type === \"speak_end\") { st.speakEnd++; phase(\"\"); }\n"
    "        else if (f.type === \"error\") { say(f.code); phase(\"error\"); }\n"
    "      }\n"
    "      function onAudio(buf) {\n"
    "        if (!ctx) return;\n"
    "        var pcm = new Int16Array(buf), f32 = new Float32Array(pcm.length);\n"
    "        for (var i = 0; i < pcm.length; i++) f32[i] = pcm[i] / 32768;\n"
    "        var b = ctx.createBuffer(1, f32.length, ctx.sampleRate); b.getChannelData(0).set(f32);\n"
    "        var src = ctx.createBufferSource(); src.buffer = b; src.connect(ctx.destination);\n"
    "        var t = Math.max(ctx.currentTime + 0.02, playAt); src.start(t); playAt = t + b.duration; st.played++;\n"
    "      }\n"
    "      // A close is the end of a CALL, never of the button: a wall screen is\n"
    "      // looked at all day and the next press is a new call. Only a refused\n"
    "      // join is final -- that one is an answer about the screen itself.\n"
    "      function onClose(c) { st.code = c.code; say(\"closed \" + c.code); phase(\"\"); joined = false; }\n"
    "      // The state line when nothing is happening: what the cell said it is\n"
    "      // made of, and an empty line before it has said anything. Said for a\n"
    "      // screen reader; the mark itself shows its phase in light.\n"
    "      function idleText() { return st.hello ? st.hello.stt + (st.hello.tts ? \"/\" + st.hello.tts : \"\") : \"\"; }\n"
    "      // The join is AWAITED, and that is not politeness: a text push is\n"
    "      // buffered by the client until the join is acknowledged, a raw binary\n"
    "      // push is not. Audio sent between `join()` and its `ok` would either\n"
    "      // be dropped or arrive at the cell BEFORE the hold that frames it.\n"
    "      // One join at a time: a press during a join in flight waits for that\n"
    "      // one instead of opening a second channel on the same socket.\n"
    "      function join() {\n"
    "        if (!socket) return Promise.resolve(false);\n"
    "        if (joinWait) return joinWait;\n"
    "        call = \"c\" + Date.now().toString(36) + Math.random().toString(36).slice(2, 8);\n"
    "        topic = \"voice:\" + call;\n"
    "        chan = socket.channel(topic, { mount: mount, mode: \"hold\", sample_rate: 16000 });\n"
    "        chan.on(\"frame\", onFrame); chan.on(\"audio\", onAudio); chan.on(\"close\", onClose);\n"
    "        say(\"joining\\u2026\");\n"
    "        joinWait = new Promise(function (done) {\n"
    "          chan.join()\n"
    "            .receive(\"ok\", function () { joined = true; joinWait = null; say(idleText()); done(true); })\n"
    "            .receive(\"error\", function (e) { refuseFrom((e && e.reason) || \"refused\"); joined = false; joinWait = null; done(false); })\n"
    "            .receive(\"timeout\", function () { say(\"the screen never answered\"); joined = false; joinWait = null; done(false); });\n"
    "        });\n"
    "        return joinWait;\n"
    "      }\n"
    "      if (audio && !refused) join();\n"
    "      function sendAudio(ab) { if (!joined) return; socket.push({ topic: topic, event: \"audio\", payload: ab, ref: \"\", join_ref: chan.joinRef() }); st.sent++; }\n"
    "      var WORKLET = \"class P extends AudioWorkletProcessor{constructor(o){super();this.rate=o.processorOptions.rate;this.acc=[];this.pos=0;var s=this;this.port.onmessage=function(e){if(e.data==='flush')s.f()}}f(){var n=Math.floor(this.rate/50);if(!this.acc.length)return;while(this.acc.length<n)this.acc.push(0);var out=new Int16Array(n);for(var j=0;j<n;j++)out[j]=this.acc[j]*32767;this.acc=this.acc.slice(n);this.port.postMessage(out.buffer,[out.buffer])}process(i){var ch=i[0]&&i[0][0];if(!ch)return true;var r=sampleRate/this.rate;for(var k=0;k<ch.length;k+=r){this.acc.push(Math.max(-1,Math.min(1,ch[Math.floor(k)])))}var n=Math.floor(this.rate/50);while(this.acc.length>=n){var out=new Int16Array(n);for(var j=0;j<n;j++)out[j]=this.acc[j]*32767;this.acc=this.acc.slice(n);this.port.postMessage(out.buffer,[out.buffer])}return true}}registerProcessor('mic',P);\";\n"
    "      async function openMic() {\n"
    "        // `localhost` read as a signpost and pointed at the wrong place; the\n"
    "        // address that WOULD work lives in a proxy this page knows nothing\n"
    "        // about, so the client says only what it knows for certain (#741).\n"
    "        if (!root.isSecureContext) { say(\"the microphone needs https \\u2014 this page is \" + root.location.origin); return false; }\n"
    "        // The permission prompt opens INSIDE the gesture, and a person who is\n"
    "        // being asked is looking at a button that does nothing. Saying so is\n"
    "        // the difference between waiting and a screen that is broken.\n"
    "        say(\"asking for the microphone\\u2026\");\n"
    "        try {\n"
    "          stream = await navigator.mediaDevices.getUserMedia({ audio: true });\n"
    "          // The capture side's own latency, where the browser reports it\n"
    "          // (Chromium does, Firefox does not -- then it stays 0). Seconds.\n"
    "          var tr = stream.getAudioTracks()[0];\n"
    "          var lat = tr && tr.getSettings && tr.getSettings().latency;\n"
    "          micLat = (typeof lat === \"number\" && lat > 0) ? lat : 0;\n"
    "          // The device said yes. Nothing else would ever take an\n"
    "          // earlier refusal off the mark, and a microphone granted a\n"
    "          // minute later must not need a reload (GH #720, OR-H0.15).\n"
    "          clearMark();\n"
    "        } catch (e) {\n"
    "          // A refused or missing microphone is the ordinary case, not a crash:\n"
    "          // an unhandled rejection here left the button doing nothing at all,\n"
    "          // which is the silence this line exists against.\n"
    "          say(\"microphone refused\");\n"
    "          return false;\n"
    "        }\n"
    "        var rate = (st.hello && st.hello.audio_in && st.hello.audio_in.sample_rate) || 16000;\n"
    "        mctx = new (root.AudioContext || root.webkitAudioContext)();\n"
    "        try {\n"
    "          await mctx.audioWorklet.addModule(URL.createObjectURL(new Blob([WORKLET], { type: \"text/javascript\" })));\n"
    "          var src = mctx.createMediaStreamSource(stream);\n"
    "          worklet = new AudioWorkletNode(mctx, \"mic\", { processorOptions: { rate: rate } });\n"
    "        } catch (e) {\n"
    "          say(\"no audio worklet\");\n"
    "          return false;\n"
    "        }\n"
    "        worklet.port.onmessage = function (e) {\n"
    "          // The port itself is the measurement: the gap between two frames is\n"
    "          // what delivery costs on this screen right now, and the drain window\n"
    "          // below is built out of it instead of out of a guess.\n"
    "          var now = Date.now();\n"
    "          if (lastFrameAt) gapMax = Math.max(gapMax, Math.min(now - lastFrameAt, 250));\n"
    "          lastFrameAt = now;\n"
    "          if (holding) { sendAudio(e.data); }\n"
    "          else if (draining) { sendAudio(e.data); st.flushed++; }\n"
    "          // From the touch until the hold: KEPT, not sent. The ring is 2 s\n"
    "          // long, and the oldest frame falls out rather than the ring\n"
    "          // growing into a leak on a screen that is touched all day.\n"
    "          else if (armed) { pre.push(e.data); if (pre.length > PRE_MAX) pre.shift(); }\n"
    "          // And outside a hold, nothing: a frame that left the device\n"
    "          // without one would be the button breaking its own promise.\n"
    "        };\n"
    "        src.connect(worklet); return true;\n"
    "      }\n"
    "      // The drain window: `release` does not mean the audio is in, it means\n"
    "      // no NEW audio belongs to this take. The last frames of every take are\n"
    "      // still in the capture chain when the key comes up -- in the worklet's\n"
    "      // accumulator, in the message port's queue, in the device -- and cutting\n"
    "      // on the key threw 300 to 600 ms away, every time (GH #698). So the key\n"
    "      // opens a window instead: the worklet is told to flush, whatever arrives\n"
    "      // for the next `drainMs` still goes out, and `release` is sent last.\n"
    "      var FLUSH_MIN = 120, FLUSH_MAX = 600;\n"
    "      var pressAt = 0, byPointer = false, holdTimer = null, ready = false;\n"
    "      // The ring behind the threshold (E-3, OR-F25). 100 frames of 20 ms =\n"
    "      // 2 s, which covers the whole setup ever measured here (0,3 to 2 s on\n"
    "      // a phone: permission, device, worklet). It fills from the first\n"
    "      // TOUCH, not from the press, and it is sent behind the `hold` that\n"
    "      // frames it. Without it the threshold would cost the first quarter\n"
    "      // second of every take.\n"
    "      var PRE_MAX = 100;\n"
    "      var pre = [], armed = false, armedAt = 0, micWait = null;\n"
    "      function arm() { armed = true; armedAt = Date.now(); pre.length = 0; }\n"
    "      function disarm() { armed = false; armedAt = 0; pre.length = 0; }\n"
    "      // One microphone at a time: a touch and the press that follows it are\n"
    "      // two callers, and two `getUserMedia` in flight would build two graphs\n"
    "      // of which one is never heard from again.\n"
    "      function mic() {\n"
    "        if (worklet) return Promise.resolve(true);\n"
    "        if (micWait) return micWait;\n"
    "        micWait = openMic().then(function (ok) { micWait = null; return ok; },\n"
    "                                 function () { micWait = null; return false; });\n"
    "        return micWait;\n"
    "      }\n"
    "      // The first touch anywhere on the mark, in the CAPTURE phase, so it is\n"
    "      // ahead of the button's own handler: the device is asked for while the\n"
    "      // finger is still going down, and from there on what it hears is kept.\n"
    "      // A touch is a gesture, so the permission prompt may open.\n"
    "      function warm() {\n"
    "        // § 6.4: without audio the client records nothing -- no ring, no\n"
    "        // permission prompt, no graph. The press below still lives.\n"
    "        if (!audio) return;\n"
    "        if (refused) return;\n"
    "        arm(); if (!worklet) mic();\n"
    "      }\n"
    "      var holding = false, pressed = false, draining = false, drainTimer = null;\n"
    "      var lastFrameAt = 0, gapMax = 0, micLat = 0;\n"
    "      // Three summands, and each says where it comes from. `base` is the\n"
    "      // larger of two numbers the browser reports: `MediaTrackSettings.latency`\n"
    "      // is the capture side's own figure, where a browser gives one (Chromium\n"
    "      // does, Firefox does not, and then it is 0); `AudioContext.baseLatency`\n"
    "      // is the context's own buffering, not the device's. Then one worklet\n"
    "      // block (20 ms), then the longest delivery gap measured during THIS\n"
    "      // hold (capped at 250 in `onmessage`). Floor 120 ms: a desktop chain\n"
    "      // is 10 to 25 + 20 + 20 to 25, about 65, and a single render frame is\n"
    "      // 16 -- below 120 there is no margin, and where no latency is reported\n"
    "      // at all the floor is what stands in for it. Ceiling 600 ms: 300 for\n"
    "      // the device (a slower one is a device to fix, not a window to widen)\n"
    "      // + 20 + 250 + 30 of margin; the window sits IN FRONT of the server's\n"
    "      // own 1500 ms grace and is paid in turn latency.\n"
    "      function flushMs() {\n"
    "        var base = Math.ceil(1000 * Math.max((mctx && mctx.baseLatency) || 0, micLat || 0));\n"
    "        return Math.max(FLUSH_MIN, Math.min(FLUSH_MAX, base + 20 + gapMax));\n"
    "      }\n"
    "      function endDrain() {\n"
    "        if (!draining) return;\n"
    "        draining = false;\n"
    "        if (drainTimer) { clearTimeout(drainTimer); drainTimer = null; }\n"
    "        frame({ type: \"release\" });\n"
    "      }\n"
    "      async function down(e) {\n"
    "        // Pressed again inside the window: the release of the PREVIOUS take\n"
    "        // has to reach the cell before this hold does, or the cell sees a\n"
    "        // `hold` while one is open and refuses it (`already_holding`).\n"
    "        if (draining) endDrain();\n"
    "        if (holding) return;\n"
    "        pressed = true; pressAt = Date.now(); ready = false;\n"
    "        // A new gesture: whatever the last one ended with is spent.\n"
    "        gesture = \"\";\n"
    "        // Only a finger or a mouse can mean the dock. The space key is the\n"
    "        // desk's way of speaking and has no dock to ask for.\n"
    "        byPointer = !!(e && e.pointerId !== undefined);\n"
    "        // The press is recorded; now the mark. This press is not the\n"
    "        // last one (GH #720): the dim left over from a device that said\n"
    "        // no belonged to that gesture, and what the socket has said\n"
    "        // about this screen is put back by the same line. It stands\n"
    "        // AFTER the two lines above on purpose -- everything a tap\n"
    "        // needs is set before anything about the channel is read (GH\n"
    "        // #704).\n"
    "        clearMark();\n"
    "        // The gesture stays on the button whatever moves under the pointer --\n"
    "        // the button itself, when the state line below it grows on a fresh\n"
    "        // screen, or a finger that drifts while holding (GH #684). It is\n"
    "        // taken before anything is opened: it belongs to the PRESS, and\n"
    "        // every press has one, including the one that cannot speak.\n"
    "        if (e && e.pointerId !== undefined && btn.setPointerCapture) { try { btn.setPointerCapture(e.pointerId); } catch (err) { /* no capture, no harm */ } }\n"
    "        // § 6.4: on an output without audio a long press is a PRESS. No\n"
    "        // threshold clock, no join, no microphone, no `hold` -- `up()`\n"
    "        // switches the dock however long the finger stayed down.\n"
    "        if (!audio) {\n"
    "          if (holdTimer) { clearTimeout(holdTimer); holdTimer = null; }\n"
    "          st.holdsRefused++;\n"
    "          return;\n"
    "        }\n"
    "        // Nothing to speak into: no ring, no microphone, no second join.\n"
    "        // The press itself still counts, because a short one means the dock\n"
    "        // and the dock has nothing to do with speech. What is armed instead\n"
    "        // is the same threshold clock, and at the end of it stands a refusal\n"
    "        // a person can read -- a mark that goes quiet is the one failure\n"
    "        // nobody can tell from a broken screen.\n"
    "        if (refused) {\n"
    "          if (holdTimer) { clearTimeout(holdTimer); holdTimer = null; }\n"
    "          holdTimer = setTimeout(refuse, HOLD_MS);\n"
    "          return;\n"
    "        }\n"
    "        // A press with no touch in front of it -- the space key, or a driver\n"
    "        // that calls the handle -- starts the ring here instead. Late by a\n"
    "        // gesture, but never empty.\n"
    "        if (!armed) arm();\n"
    "        if (!joined && !(await join())) return;\n"
    "        // The playback context was built on join, which is not a gesture: under\n"
    "        // an autoplay policy it starts suspended and its clock does not run, so\n"
    "        // the gap-free scheduling would schedule against a stopped clock. This is\n"
    "        // the first gesture there is, so it is where it gets resumed.\n"
    "        if (ctx && ctx.state === \"suspended\") { try { await ctx.resume(); } catch (e) { /* nothing to resume */ } }\n"
    "        if (mctx && mctx.state === \"suspended\") { try { await mctx.resume(); } catch (e) { /* nothing to resume */ } }\n"
    "        if (!worklet && !(await mic())) {\n"
    "          // The device said no; the press did not (GH #719, OR-H0.15). It\n"
    "          // runs off the SAME threshold clock as a take, so a tap stays a tap\n"
    "          // and the dock keeps answering it -- only a real hold opens the chat\n"
    "          // with no voice behind it.\n"
    "          var noMic = HOLD_MS - (Date.now() - pressAt);\n"
    "          if (holdTimer) { clearTimeout(holdTimer); holdTimer = null; }\n"
    "          if (noMic > 0) { holdTimer = setTimeout(refuseMic, noMic); return; }\n"
    "          refuseMic();\n"
    "          return;\n"
    "        }\n"
    "        // Let go while the browser was still asking: the microphone is open\n"
    "        // now and the gesture is over, so the next press is a whole take.\n"
    "        if (!pressed) { say(\"press again\"); return; }\n"
    "        // The gap is measured per take: the worklet posts between holds too,\n"
    "        // and a screen rendering patches in between would otherwise carry\n"
    "        // its worst pause into every window that follows. `lastFrameAt`\n"
    "        // stays, or the first gap of this take would be lost.\n"
    "        gapMax = 0;\n"
    "        ready = true;\n"
    "        // Everything is open; what is left is the threshold. A finger that\n"
    "        // has already been down that long starts its take in this turn.\n"
    "        var wait = HOLD_MS - (Date.now() - pressAt);\n"
    "        if (holdTimer) { clearTimeout(holdTimer); holdTimer = null; }\n"
    "        if (wait > 0) { holdTimer = setTimeout(begin, wait); return; }\n"
    "        begin();\n"
    "      }\n"
    "      // What `begin` is for a screen that can speak. It runs off the same\n"
    "      // threshold clock and refuses the same press: a finger that is already\n"
    "      // up gets nothing, because that press was a tap and the dock already\n"
    "      // answered it.\n"
    "      function refuse() {\n"
    "        holdTimer = null;\n"
    "        if (!pressed || holding) return;\n"
    "        say(refusal); phase(\"error\"); st.refused++; st.holdsRefused++;\n"
    "      }\n"
    # OR-H0.15 (18.09.2026, GH #719): a refused MICROPHONE is not a refused
    # channel. The hold is the event and the audio is best effort.
    "      // The end of a press whose microphone said no. The device is refused,\n"
    "      // the HOLD is not: § 5.4 asks the mark to say the refusal and dim, and\n"
    "      // OR-H0.15 adds that the hold still reaches the screen -- on an output\n"
    "      // with a keyboard the chat's own input line (§ 7.3) then carries what\n"
    "      // the voice cannot. `openMic()` has already said what happened in the\n"
    "      // live region, so this is the other half: `data-phase=\"error\"`, which\n"
    "      // is what the sheet dims on (`--mark-dim`), and a counter that makes it\n"
    "      // measurable from outside. What it does NOT do is set `refused` -- that\n"
    "      // latch is the socket's word, and a sticky device refusal would make a\n"
    "      // microphone granted a minute later need a page reload. Measured before\n"
    "      // this existed (Chromium and WebKit, http:// on a LAN address): a 900 ms\n"
    "      // press did nothing at all -- no hold, no dock, no dim, every counter 0.\n"
    "      function refuseMic() {\n"
    "        holdTimer = null;\n"
    "        if (!pressed || holding) return;\n"
    "        phase(\"error\"); st.holdsRefused++;\n"
    "        if (hookSelf.pushEvent) { hookSelf.pushEvent(\"hold\", {}); st.touches++; }\n"
    "      }\n"
    "      // The take itself. It runs from `down` or from the threshold timer,\n"
    "      // and it refuses both a finger that is already up and a second entry.\n"
    "      function begin() {\n"
    "        if (holdTimer) { clearTimeout(holdTimer); holdTimer = null; }\n"
    "        if (!pressed || holding || !ready) return;\n"
    "        holding = true; btn.setAttribute(\"aria-pressed\", \"true\"); say(\"listening\\u2026\"); phase(\"listening\"); frame({ type: \"hold\" });\n"
    "        // And the other half of a hold (§ 5.4): holding the mark is where a\n"
    "        // dialogue starts, so the curator gets the hold at the same moment\n"
    "        // the voice cell gets the frame. The event carries NO field\n"
    "        // (§ 5.6, S-088): which window it lands on is step 3's business --\n"
    "        // the one with `topic: chat` -- and a topic sent from here would\n"
    "        // be ignored, which makes sending it a promise about the wrong\n"
    "        // thing. The counter keeps its old name; what the wire says is\n"
    "        // `hold`, and `touch` is the curator's word for the effect (§ 2).\n"
    "        st.holdMs = Date.now() - pressAt;\n"
    # § 5.6: the event is `hold` and it carries NOTHING. Which window a hold reaches
    # is the SCREEN's knowledge, not the browser's (§ 5.4, § 8.5); a `topic` a client
    # sent with it would not be read (S-088).
    "        if (hookSelf.pushEvent) { hookSelf.pushEvent(\"hold\", {}); st.touches++; }\n"
    "        // What was said between the touch and this line, in order, ahead of\n"
    "        // the live frames. The cell queues them behind the hold that frames\n"
    "        // them (the text frame opens the session, the binary frames behind\n"
    "        // it go into its channel). The wait for the threshold is INSIDE this\n"
    "        // window: 250 ms of a 2 s ring.\n"
    "        st.setupMs = armedAt ? Date.now() - armedAt : 0;\n"
    "        st.prebuffered = pre.length;\n"
    "        while (pre.length) sendAudio(pre.shift());\n"
    "        disarm();\n"
    "      }\n"
    "      function up() {\n"
    "        if (holdTimer) { clearTimeout(holdTimer); holdTimer = null; }\n"
    "        var was = pressed, held = pressAt ? Date.now() - pressAt : 0;\n"
    "        // The gesture ended HERE, whether it spoke or not: a `click` that\n"
    "        // follows belongs to it and switches nothing.\n"
    "        if (was && byPointer) gesture = \"pointer\";\n"
    "        pressed = false; ready = false;\n"
    "        if (!holding) {\n"
    "          // A touch that never became a hold: what the ring heard belonged\n"
    "          // to a take that did not happen, so it goes with the gesture.\n"
    "          disarm();\n"
    "          // Let go before the threshold: that was a TAP (E-18). Nothing was\n"
    "          // sent, so there is nothing to release, and the one thing it means\n"
    "          // is the dock. `blur` and `visibilitychange` land here too, with\n"
    "          // no press behind them, and must not switch anything.\n"
    "          // § 6.4/§ 5.5: with audio, only a press under the threshold is\n"
    "          // a press -- a longer one spoke. Without audio there is no\n"
    "          // hold to tell it from, so every press means the dock.\n"
    "          if (was && byPointer && (!audio || held < HOLD_MS)) tapDock();\n"
    "          return;\n"
    "        }\n"
    "        holding = false; btn.setAttribute(\"aria-pressed\", \"false\"); say(idleText()); phase(\"sending\");\n"
    "        // The ring belongs to the take that just ended. `begin()` disarmed\n"
    "        // on the way in, but a second finger on the mark re-arms it, and\n"
    "        // leaving it armed here would keep the ring filling BETWEEN takes:\n"
    "        // `down()` would skip its own `arm()`, and the next hold would\n"
    "        // carry up to 2 s of room tone from before this release in front\n"
    "        // of it, with a `setup_ms` that never happened.\n"
    "        disarm();\n"
    "        // The key is up for the person the moment it is up. For the audio it\n"
    "        // is up one window later, and `release` goes at the END of it.\n"
    "        draining = true; st.drainMs = flushMs();\n"
    "        try { if (worklet) worklet.port.postMessage(\"flush\"); } catch (e) { /* no worklet, no flush */ }\n"
    "        if (drainTimer) clearTimeout(drainTimer);\n"
    "        drainTimer = setTimeout(endDrain, st.drainMs);\n"
    "      }\n"
    "      // A display is a surface other people's components render onto, so the\n"
    "      // window-wide key must keep its hands off their controls. It asks\n"
    "      // nothing about the channel: a press with nothing to speak into says\n"
    "      // so at the threshold, in the open.\n"
    "      function typing(e) {\n"
    "        var t = e.target;\n"
    "        if (!t || t === root || t === document.body) return false;\n"
    "        var tag = (t.tagName || \"\").toLowerCase();\n"
    "        return tag === \"input\" || tag === \"textarea\" || tag === \"select\" || t.isContentEditable === true;\n"
    "      }\n"
    "      function keydown(e) { if (e.code !== \"Space\" || e.repeat || typing(e)) return; e.preventDefault(); down(); }\n"
    "      function keyup(e) { if (e.code !== \"Space\" || typing(e)) return; e.preventDefault(); up(); }\n"
    "      // A hold ends when the page does: a key held while switching windows\n"
    "      // would otherwise keep the microphone open with nothing left to release it.\n"
    "      function onBlur() { up(); }\n"
    "      function onHide() { if (document.hidden) up(); }\n"
    "      // The second way in. `pointerup` is the first and answers fastest;\n"
    "      // this one answers where it never came. What it must NOT do is\n"
    "      // switch after a `pointerup` that already did -- nor after a HOLD,\n"
    "      // because a press that spoke also ends in a click. Both are the same\n"
    "      // question: did this gesture already end by pointer?\n"
    "      function onClick() {\n"
    "        if (holding || draining) return;\n"
    "        if (gesture !== \"\") return;\n"
    "        // A click is the END of the gesture it belongs to, and sometimes\n"
    "        // the only end there is: under `touch-action: none` with a captured\n"
    "        // pointer the `pointerup` can go missing entirely. Without these\n"
    "        // two lines the tap would count here and the threshold timer would\n"
    "        // still fire afterwards -- a `hold` frame and a `touch` event for a\n"
    "        // finger that is long gone, and a microphone nothing releases.\n"
    "        if (holdTimer) { clearTimeout(holdTimer); holdTimer = null; }\n"
    "        pressed = false; ready = false;\n"
    "        tapDock();\n"
    "      }\n"
    "      // On the MARK and in the capture phase, ahead of the button's own\n"
    "      // handler: the ring has to be armed before anything else reads the\n"
    "      // gesture, and the element is what a finger lands on.\n"
    "      el.addEventListener(\"pointerdown\", warm, true);\n"
    "      btn.addEventListener(\"pointerdown\", down); btn.addEventListener(\"pointerup\", up); btn.addEventListener(\"pointercancel\", up); btn.addEventListener(\"lostpointercapture\", up); btn.addEventListener(\"click\", onClick);\n"
    "      window.addEventListener(\"blur\", onBlur); document.addEventListener(\"visibilitychange\", onHide);\n"
    "      root.addEventListener(\"keydown\", keydown);\n"
    "      root.addEventListener(\"keyup\", keyup);\n"
    "      st.down = down; st.up = up; st.cancel = function () { frame({ type: \"cancel\" }); };\n"
    "      st.holdThreshold = HOLD_MS; st.tapDock = tapDock; st.warm = warm;\n"
    "      // What a re-mount has to undo. LiveView re-mounts a hook after a reconnect,\n"
    "      // and without this the listeners, the microphone and the contexts of every\n"
    "      // previous life stay open.\n"
    "      this.__displayMicTeardown = function () {\n"
    "        root.removeEventListener(\"keydown\", keydown);\n"
    "        root.removeEventListener(\"keyup\", keyup);\n"
    "        window.removeEventListener(\"blur\", onBlur); document.removeEventListener(\"visibilitychange\", onHide);\n"
    "        btn.removeEventListener(\"pointerdown\", down); btn.removeEventListener(\"pointerup\", up); btn.removeEventListener(\"pointercancel\", up); btn.removeEventListener(\"lostpointercapture\", up);\n"
    "        btn.removeEventListener(\"click\", onClick);\n"
    "        el.removeEventListener(\"pointerdown\", warm, true);\n"
    "        disarm(); micWait = null;\n"
    "        pressAt = 0; byPointer = false; ready = false;\n"
    "        gesture = \"\";\n"
    "        if (holdTimer) { clearTimeout(holdTimer); holdTimer = null; }\n"
    "        holding = false; pressed = false; joined = false; joinWait = null;\n"
    "        refused = false; refusal = \"\";\n"
    "        draining = false; if (drainTimer) { clearTimeout(drainTimer); drainTimer = null; }\n"
    "        lastFrameAt = 0; gapMax = 0; micLat = 0;\n"
    "        phase(\"\"); btn.setAttribute(\"aria-pressed\", \"false\");\n"
    "        if (stream) { stream.getTracks().forEach(function (t) { t.stop(); }); stream = null; }\n"
    "        if (worklet) { try { worklet.port.onmessage = null; worklet.disconnect(); } catch (e) { /* gone */ } worklet = null; }\n"
    "        if (mctx) { try { mctx.close(); } catch (e) { /* gone */ } mctx = null; }\n"
    "        if (ctx) { try { ctx.close(); } catch (e) { /* gone */ } ctx = null; }\n"
    "        if (chan) { try { chan.leave(); } catch (e) { /* the socket may be gone already */ } chan = null; }\n"
    "      };\n"
    "    },\n"
    "    // The patch does not own the mark. The mark rides in the pass's\n"
    "    // render, so every pass morphs the hook's own element, and the\n"
    "    // server's phase is always empty -- because the colony does not\n"
    "    // know, and by § 3.2 must not know, what this browser's microphone\n"
    "    // said (GH #720).\n"
    "    updated: function () {\n"
    "      if (this.__mic) this.__mic.repaint();\n"
    "    },\n"
    "    destroyed: function () {\n"
    "      if (this.__displayMicTeardown) { this.__displayMicTeardown(); this.__displayMicTeardown = null; }\n"
    "      this.__mic = null;\n"
    "    }\n"
    "  };\n"
    "  root.SurfaceHooks = Object.assign(root.SurfaceHooks || {}, { DisplayMic: hook });\n"
    "})(window);\n"
)

# The screen's own motion (§ 2.5). A hook on the ROOT, because the thing it
# has to see is the whole screen: a window that left the canvas and the tile
# it went into are two elements the patch touches in one go, and only somebody
# holding both ends can move one into the other.
#
# It rides as a raw prop, the pattern the OS mark uses, and for the same
# reason: the script runs in the DEAD render, before the shell's socket
# constructor reads `window.SurfaceHooks`, and a morph never re-runs it.
#
# FLIP and not the document's view-transition call: LiveView patches with
# morphdom and cannot be wrapped in one, and `::view-transition-group(*)` moves
# every window at once. The names stay inline on the windows -- they cost nothing
# and they are where a later wave would start.
SCENE_CLIENT_JS = (
    "(function (root) {\n"
    "  var EASE = \"cubic-bezier(.32,.72,0,1)\";\n"
    "  function reduced() {\n"
    "    return !!(root.matchMedia && root.matchMedia(\"(prefers-reduced-motion: reduce)\").matches);\n"
    "  }\n"
    "  // A duration out of the sheet's own tokens, so the motion is the\n"
    "  // template's decision and not this script's.\n"
    "  function dur(el, name, fallback) {\n"
    "    var v = root.getComputedStyle(el).getPropertyValue(name).trim();\n"
    "    var n = parseFloat(v);\n"
    "    return isNaN(n) ? fallback : (v.indexOf(\"ms\") > -1 ? n : n * 1000);\n"
    "  }\n"
    "  // Every tile by the window it stands for, and every window by its id.\n"
    "  // The two halves of one object carry the same name on purpose.\n"
    "  function scan(el) {\n"
    "    var out = { tiles: {}, wins: {} }, i;\n"
    "    var t = el.querySelectorAll(\".display-tile[data-for]\");\n"
    "    for (i = 0; i < t.length; i++) {\n"
    "      out.tiles[t[i].getAttribute(\"data-for\")] = { node: t[i], rect: t[i].getBoundingClientRect() };\n"
    "    }\n"
    "    var w = el.querySelectorAll(\"[data-region] [id][data-level]\");\n"
    "    for (i = 0; i < w.length; i++) {\n"
    "      out.wins[w[i].id] = { node: w[i], rect: w[i].getBoundingClientRect(),\n"
    "        level: w[i].getAttribute(\"data-level\"), age: w[i].getAttribute(\"data-age\") };\n"
    "    }\n"
    "    return out;\n"
    "  }\n"
    "  // Open is a LEVEL, not a rung (display-hive.md § 4.24). The recorded\n"
    "  // VALUE is what is compared, never the node: `flip` runs in `updated`,\n"
    "  // after the patch, so a live node already carries the new attribute --\n"
    "  // the snapshot is the only place the old one still exists.\n"
    "  function open(level) { return !!level && level !== \"0\"; }\n"
    "  // One movement: the element is put back where it was and let go.\n"
    "  function move(node, from, to, ms, fade) {\n"
    "    if (!node.animate || !to.width || !to.height) return false;\n"
    "    var dx = from.left - to.left, dy = from.top - to.top;\n"
    "    var sx = from.width / to.width, sy = from.height / to.height;\n"
    "    node.animate([\n"
    "      { transformOrigin: \"top left\", transform: \"translate(\" + dx + \"px,\" + dy + \"px) scale(\" + sx + \",\" + sy + \")\", opacity: fade },\n"
    "      { transformOrigin: \"top left\", transform: \"none\", opacity: 1 }\n"
    "    ], { duration: ms, easing: EASE });\n"
    "    return true;\n"
    "  }\n"
    "  function away(node, from, to, ms) {\n"
    "    if (!node.animate || !from.width || !from.height) return false;\n"
    "    var dx = to.left - from.left, dy = to.top - from.top;\n"
    "    var sx = to.width / from.width, sy = to.height / from.height;\n"
    "    node.animate([\n"
    "      { transformOrigin: \"top left\", transform: \"none\", opacity: 1 },\n"
    "      { transformOrigin: \"top left\", transform: \"translate(\" + dx + \"px,\" + dy + \"px) scale(\" + sx + \",\" + sy + \")\", opacity: 0 }\n"
    "    ], { duration: ms, easing: EASE, fill: \"forwards\" });\n"
    "    return true;\n"
    "  }\n"
    "  function blink(node, ms) {\n"
    "    node.setAttribute(\"data-zoomed\", \"true\");\n"
    "    root.setTimeout(function () { node.removeAttribute(\"data-zoomed\"); }, ms);\n"
    "  }\n"
    "  // The effect of a tap, drawn before the pass answers (§ 5.7). The\n"
    "  // movement below (`away`) does NOT run for a tap any more, and that is\n"
    "  // the point rather than a loss: this function writes the end state, so\n"
    "  // the snapshot `beforeUpdate` takes already holds it and `flip` finds\n"
    "  // nothing that moved (§ 5.8: the series draws its end state, and ten\n"
    "  // taps in a second are not ten flights). What the FINGER did is\n"
    "  // instant; what the SYSTEM did -- an app withdrawing its view, a judge\n"
    "  // hiding a window, a modal co-closed by a tap on another output --\n"
    "  // still arrives as a patch nobody drew here, and flies into its tile\n"
    "  // exactly as before. Both halves are wanted, and this is where the line\n"
    "  // between them runs (OR-H2.9).\n"
    "  // curator confirms it one pass later; because the finger stands above\n"
    "  // the score (§ 4.19) the pass never disagrees, and a deviation is a\n"
    "  // defect -- Q-20 is the pass this is checked against. The client writes\n"
    "  // exactly the words the curator writes, so the confirming patch changes\n"
    "  // nothing and nothing moves twice.\n"
    "  function optimistic(el, tile, st) {\n"
    "    var id = tile.getAttribute(\"data-for\"), win = id && document.getElementById(id);\n"
    "    if (!win) return;\n"
    "    var open = win.getAttribute(\"data-level\") !== \"0\";\n"
    "    var closed = [];\n"
    "    el.dataset.optimistic = \"1\";\n"
    "    if (open) {\n"
    "      // § 5.2 put away: the window goes back into its tile.\n"
    "      win.setAttribute(\"data-level\", \"0\"); win.setAttribute(\"data-rung\", \"ambient\");\n"
    "      tile.setAttribute(\"data-open\", \"\");\n"
    "    } else {\n"
    "      // § 5.1 open: the window leads its ladder.\n"
    "      var modal = win.getAttribute(\"data-layer\") === \"modal\";\n"
    "      win.setAttribute(\"data-level\", modal ? \"2\" : \"1\"); win.setAttribute(\"data-rung\", \"focus\");\n"
    "      tile.setAttribute(\"data-open\", \"1\");\n"
    "    }\n"
    "    if (win.getAttribute(\"data-layer\") === \"canvas\") {\n"
    "      // § 5.3, on open AND on put-away: the rule reads the tapped\n"
    "      // window's LAYER, never its rung (S-077).\n"
    "      var modals = el.querySelectorAll('[data-region] [data-level=\"2\"]'), i;\n"
    "      for (i = 0; i < modals.length; i++) {\n"
    "        modals[i].setAttribute(\"data-level\", \"0\"); modals[i].setAttribute(\"data-rung\", \"ambient\");\n"
    "        if (modals[i].id) closed.push({ id: modals[i].id,\n"
    "          was: Number(modals[i].getAttribute(\"data-acted\") || 0) });\n"
    "      }\n"
    "    }\n"
    "    st.optimistic++;\n"
    "    // What was drawn, and the stamp the window wore when the finger landed. The\n"
    "    // stamp is the SERVER's (`data-acted` = the last touch or put-away of this\n"
    "    // window, § 4.9/§ 5.2), read out of the DOM rather than taken off this\n"
    "    // device's clock: a comparison between two values of one clock cannot be\n"
    "    // skewed, and a skewed browser would otherwise let go of the drawing early.\n"
    "    st.drawn[id] = { at: Date.now(), was: Number(win.getAttribute(\"data-acted\") || 0),\n"
    "      level: win.getAttribute(\"data-level\"), rung: win.getAttribute(\"data-rung\"),\n"
    "      open: tile.getAttribute(\"data-open\") || \"\", closed: closed };\n"
    "    root.requestAnimationFrame(function () { delete el.dataset.optimistic; });\n"
    "  }\n"
    "  // The other half of § 5.7 (Decision 18.09.): a patch from a pass that started\n"
    "  // BEFORE the tap renders the state before the tap and writes it over the\n"
    "  // drawing. Measured in the owner's own session on the candidate colony -- five\n"
    "  // taps on the chat tile in four seconds, every one of them processed, and a\n"
    "  // window that flashed open and shut each time\n"
    "  // (`plans/welle-h3-2026-09-18/messungen/B-klickserie.md` § 3, 16:38:16). So the\n"
    "  // drawing is put back until a patch carries a state computed after the tap,\n"
    "  // which the curator's stamp says: `data-acted` bigger than the one the window\n"
    "  // wore. It is not a promise the client can keep for ever -- a tap the pass\n"
    "  // absorbs (§ 6.12, S-053) never moves the stamp at all -- so the hold also ends\n"
    "  // by the clock: KEEP_MS is above the longest pass measured on a loaded twin\n"
    "  // (1.6-3.4 s, § 4a of the same report) and far below the time a person would\n"
    "  // spend looking at a window that is wrong.\n"
    "  // A serial curator does NOT make this redundant (GH #765 in 2.6.0, the resident\n"
    "  // curator of GH #809 in 2.7.0), and that was measured rather than argued: the\n"
    "  // flash comes from a pass that ran BEFORE the finger. In a six-run series without\n"
    "  // this hold (2.6.0), one of the twenty-two taps that reached the screen was\n"
    "  // answered first by the pass of the clock's own STROKE -- the order the screen\n"
    "  // placed itself, coming due --\n"
    "  // which started 66 ms before the finger and wrote its row with `rows_affected 1`,\n"
    "  // so its drawing was the truth of that instant, and it rendered the state before\n"
    "  // the tap for 90 ms until the tap's own patch arrived. No order of the writes\n"
    "  // takes that case away; only the client can, and this is the client doing it.\n"
    "  var KEEP_MS = 6000;\n"
    "  function redraw(el, st) {\n"
    "    var now = Date.now(), id, h, win, tile, m, c, i;\n"
    "    for (id in st.drawn) {\n"
    "      h = st.drawn[id]; win = document.getElementById(id);\n"
    "      if (!win || now - h.at > KEEP_MS) { delete st.drawn[id]; continue; }\n"
    "      if (Number(win.getAttribute(\"data-acted\") || 0) > h.was) { delete st.drawn[id]; continue; }\n"
    "      win.setAttribute(\"data-level\", h.level); win.setAttribute(\"data-rung\", h.rung);\n"
    "      tile = el.querySelector('[data-for=\"' + id + '\"]');\n"
    "      if (tile) tile.setAttribute(\"data-open\", h.open);\n"
    "      // A co-closed window is held by its OWN stamp, exactly like the tapped one:\n"
    "      // \u00a7 5.7 runs in both directions. The state is one for all outputs (\u00a7 3.1), and\n"
    "      // this hook only ever sees one of them -- a chat opened by a HOLD (\u00a7 5.4, which\n"
    "      // never passes through the tile handler) or a tap on the phone reaches this\n"
    "      // browser only as a patch. Closing it again from a list drawn seconds ago is\n"
    "      // the same \"the tile does nothing\" from the other side.\n"
    "      for (i = 0; i < h.closed.length; i++) {\n"
    "        c = h.closed[i];\n"
    "        if (st.drawn[c.id]) continue;\n"
    "        m = document.getElementById(c.id);\n"
    "        if (!m || Number(m.getAttribute(\"data-acted\") || 0) > c.was) continue;\n"
    "        m.setAttribute(\"data-level\", \"0\"); m.setAttribute(\"data-rung\", \"ambient\");\n"
    "      }\n"
    "      st.restored++;\n"
    "    }\n"
    "  }\n"
    "  // The zoom, both ways, plus the dock's own reordering. Nothing here\n"
    "  // knows what an object IS -- only that a tile and a window share a name.\n"
    "  function flip(el, before, st) {\n"
    "    // § 5.8: while the client draws, the series draws its END state. An\n"
    "    // animation per patch is the flashing ten taps in a second would be.\n"
    "    if (el.dataset.optimistic === \"1\") return;\n"
    "    if (reduced()) return;\n"
    "    var after = scan(el), id;\n"
    "    var enter = dur(el, \"--t-enter\", 360), leave = dur(el, \"--t-leave\", 240);\n"
    "    for (id in after.wins) {\n"
    "      var now = after.wins[id], was = before.wins[id];\n"
    "      if (!open(now.level) || (was && open(was.level))) continue;\n"
    "      var from = before.tiles[id];\n"
    "      if (!from) continue;\n"
    "      if (move(now.node, from.rect, now.rect, enter, 0.4)) st.flips++;\n"
    "      if (after.tiles[id]) blink(after.tiles[id].node, enter);\n"
    "    }\n"
    "    for (id in before.wins) {\n"
    "      var gone = !after.wins[id] || after.wins[id].age === \"leaving\"\n"
    "        || !open(after.wins[id].level);\n"
    "      if (!gone || !open(before.wins[id].level)) continue;\n"
    "      var tile = after.tiles[id];\n"
    "      var node = after.wins[id] ? after.wins[id].node : null;\n"
    "      if (!tile || !node) continue;\n"
    "      if (away(node, before.wins[id].rect, tile.rect, leave)) st.flips++;\n"
    "    }\n"
    "    for (id in after.tiles) {\n"
    "      var old = before.tiles[id];\n"
    "      if (!old) continue;\n"
    "      var dy = old.rect.top - after.tiles[id].rect.top;\n"
    "      if (Math.abs(dy) < 1) continue;\n"
    "      after.tiles[id].node.animate(\n"
    "        [{ transform: \"translateY(\" + dy + \"px)\" }, { transform: \"none\" }],\n"
    "        { duration: leave, easing: EASE });\n"
    "      st.flips++;\n"
    "    }\n"
    "  }\n"
    "  // The conversation shows its NEWEST line -- the foot of whatever scrolls\n"
    "  // above the lines, found rather than named. Held back only where somebody\n"
    "  // scrolled away from the anchor stamped below, read before the patch:\n"
    "  // distance to the foot would read content that grew by itself as a reader\n"
    "  // and never follow an answer again (OR-F78 argues both halves).\n"
    "  var END_SLACK = 24;\n"
    "  function tails(el) {\n"
    "    var l = el.querySelectorAll(\".display-chat-lines\"), out = [], i, p;\n"
    "    for (i = 0; i < l.length; i++) {\n"
    "      for (p = l[i]; p && p !== document.body; p = p.parentElement) {\n"
    "        if (p.scrollHeight - p.clientHeight > 1) { out.push(p); break; }\n"
    "      }\n"
    "    }\n"
    "    return out;\n"
    "  }\n"
    "  function held(el) {\n"
    "    return tails(el).filter(function (n) { return n.__end > n.scrollTop + END_SLACK; });\n"
    "  }\n"
    "  function toEnd(el, keep, st) {\n"
    "    tails(el).forEach(function (n) {\n"
    "      if (keep.indexOf(n) > -1) return;\n"
    "      n.scrollTop = n.scrollHeight;\n"
    "      n.__end = n.scrollTop;\n"
    "      st.ends++;\n"
    "    });\n"
    "  }\n"
    "  // § 8.4 (R-26-1): the line carries `at` as epoch milliseconds, and nothing\n"
    "  // else. The screen state has no time zone -- the DEVICE in front of a person\n"
    "  // has one -- so the clock is written here, in the browser of whoever is\n"
    "  // looking. HH:MM, never seconds; a line of today carries no date, a line of\n"
    "  // any other day carries DD.MM. in front of it (§ 8.4).\n"
    "  var HHMM = null;\n"
    "  function hhmm(d) {\n"
    "    if (HHMM === null) {\n"
    "      try {\n"
    "        HHMM = new Intl.DateTimeFormat(undefined,\n"
    "          { hour: \"2-digit\", minute: \"2-digit\", hourCycle: \"h23\" });\n"
    "      } catch (e) { HHMM = false; }\n"
    "    }\n"
    "    if (HHMM) return HHMM.format(d);\n"
    "    var h = d.getHours(), m = d.getMinutes();\n"
    "    return (h < 10 ? \"0\" : \"\") + h + \":\" + (m < 10 ? \"0\" : \"\") + m;\n"
    "  }\n"
    "  function daykey(d) {\n"
    "    return d.getFullYear() + \"-\" + (d.getMonth() + 1) + \"-\" + d.getDate();\n"
    "  }\n"
    "  // `data-at` is the truth and the text is derived from it, so a node already\n"
    "  // written for the same `at` ON THE SAME DAY is left alone. Idempotence is not\n"
    "  // a nicety here: this runs on EVERY patch, and rewriting the text of a node\n"
    "  // would drop a selection somebody is making inside the line. The day is part\n"
    "  // of the stamp because a line written before midnight has to grow its date\n"
    "  // when the day turns over, and the first patch after it does that.\n"
    "  function clocks(el, st) {\n"
    "    var nodes = el.querySelectorAll(\"time[data-at]\"), today = daykey(new Date()), i;\n"
    "    for (i = 0; i < nodes.length; i++) {\n"
    "      var n = nodes[i], raw = n.getAttribute(\"data-at\"), at = parseInt(raw, 10);\n"
    "      if (!at) continue;\n"
    "      var stamp = raw + \"@\" + today;\n"
    "      if (n.dataset.clocked === stamp && n.textContent) continue;\n"
    "      var d = new Date(at), text = hhmm(d);\n"
    "      if (daykey(d) !== today) {\n"
    "        var dd = d.getDate(), mm = d.getMonth() + 1;\n"
    "        text = (dd < 10 ? \"0\" : \"\") + dd + \".\"\n"
    "               + (mm < 10 ? \"0\" : \"\") + mm + \". \" + text;\n"
    "      }\n"
    "      n.textContent = text;\n"
    "      n.setAttribute(\"datetime\", d.toISOString());\n"
    "      n.dataset.clocked = stamp;\n"
    "      st.clocks++;\n"
    "    }\n"
    "  }\n"
    "  var CHIME_EVERY_MS = 2000, CHIME_MAX_MS = 60000;\n"
    "  function mmss(ms) {\n"
    "    if (ms < 0) ms = 0;\n"
    "    var s = Math.round(ms / 1000), m = Math.floor(s / 60);\n"
    "    s = s - m * 60;\n"
    "    return (m < 10 ? \"0\" : \"\") + m + \":\" + (s < 10 ? \"0\" : \"\") + s;\n"
    "  }\n"
    "  // The server's resolution is twenty seconds, so the seconds are the\n"
    "  // browser's (D-27). This is real time semantics, not polling: nothing\n"
    "  // is asked of the colony, a clock is read.\n"
    "  function tick(el, st) {\n"
    "    var nodes = el.querySelectorAll(\"[data-end-at]\"), now = Date.now(), i;\n"
    "    for (i = 0; i < nodes.length; i++) {\n"
    "      var end = parseInt(nodes[i].getAttribute(\"data-end-at\"), 10);\n"
    "      if (!end) continue;\n"
    "      // The bar's --now is stamped ONCE per element: the sheet turns it\n"
    "      // into a negative animation delay, and re-stamping would re-seek\n"
    "      // the animation on every second.\n"
    "      if (!nodes[i].dataset.nowStamped) {\n"
    "        nodes[i].style.setProperty(\"--now\", String(now));\n"
    "        nodes[i].dataset.nowStamped = \"1\";\n"
    "      }\n"
    "      // Only where the template marked a slot. Writing into the\n"
    "      // element itself would empty a tile of its glyph and its line.\n"
    "      var slot = nodes[i].querySelector('[data-role=\"remaining\"]');\n"
    "      if (slot) slot.textContent = mmss(end - now);\n"
    "    }\n"
    "    st.ticks++;\n"
    "  }\n"
    "  // Two tones, three times, out of oscillators: no asset ships for this\n"
    "  // and nothing speaks (speaking needs a call, and that is another wave).\n"
    "  function chime(st) {\n"
    "    try {\n"
    "      if (!st.audio) st.audio = new (root.AudioContext || root.webkitAudioContext)();\n"
    "      var ctx = st.audio;\n"
    "      if (ctx.state === \"suspended\") {\n"
    "        if (!st.said && root.console) { st.said = true; root.console.info(\"display: the chime is muted until a gesture or an autoplay flag\"); }\n"
    "        return;\n"
    "      }\n"
    "      var t0 = ctx.currentTime, tones = [880, 1175, 880], i;\n"
    "      for (i = 0; i < tones.length; i++) {\n"
    "        var at = t0 + i * 0.22;\n"
    "        var osc = ctx.createOscillator(), gain = ctx.createGain();\n"
    "        osc.type = \"sine\"; osc.frequency.value = tones[i];\n"
    "        gain.gain.setValueAtTime(0.0001, at);\n"
    "        gain.gain.exponentialRampToValueAtTime(0.25, at + 0.02);\n"
    "        gain.gain.exponentialRampToValueAtTime(0.0001, at + 0.18);\n"
    "        osc.connect(gain); gain.connect(ctx.destination);\n"
    "        osc.start(at); osc.stop(at + 0.2);\n"
    "      }\n"
    "      st.chimes++;\n"
    "    } catch (e) { /* a page with no audio stays a page */ }\n"
    "  }\n"
    "  // The FRONT urgent rings (§ 6.13): on its appearance on level 3, then\n"
    "  // every two seconds, at most one minute. Which window that is, the\n"
    "  // curator says -- `data-front` on exactly one window or on none\n"
    "  // (§ 4.18). Until 2.5.0 this collected every window with the rung\n"
    "  // `urgent`, so two ringing timers rang twice.\n"
    "  function ring(el, st, seen) {\n"
    "    var n = el.querySelector('[data-region] [id][data-front=\"1\"]');\n"
    "    var id = n ? n.id : \"\";\n"
    "    if (!id) { seen.id = \"\"; seen.since = 0; return; }\n"
    "    var t = Date.now();\n"
    "    // Another id is another window: it just appeared up there, whatever\n"
    "    // stood there before.\n"
    "    if (id !== seen.id) { seen.id = id; seen.since = t; seen.last = t; chime(st); return; }\n"
    "    if (t - seen.since < CHIME_MAX_MS && t - seen.last >= CHIME_EVERY_MS) {\n"
    "      seen.last = t; chime(st);\n"
    "    }\n"
    "  }\n"
    "  // An autoplay policy starts the context suspended and its clock does\n"
    "  // not run. The first gesture on the page is where it is resumed -- the\n"
    "  // same trade the OS mark makes, and the kiosk flag is the other half.\n"
    "  function arm(st) {\n"
    "    function go() {\n"
    "      try {\n"
    "        if (!st.audio) st.audio = new (root.AudioContext || root.webkitAudioContext)();\n"
    "        if (st.audio.state === \"suspended\") st.audio.resume();\n"
    "      } catch (e) { /* no audio on this page */ }\n"
    "    }\n"
    "    root.addEventListener(\"pointerdown\", go, true);\n"
    "    root.addEventListener(\"keydown\", go, true);\n"
    "    return function () {\n"
    "      root.removeEventListener(\"pointerdown\", go, true);\n"
    "      root.removeEventListener(\"keydown\", go, true);\n"
    "    };\n"
    "  }\n"
    "  // display-hive.md § 6.5: `/<mount>/` is a switch, and the check is the\n"
    "  // client's because only the client knows the device. It runs ONCE,\n"
    "  // before anything is registered: a page that is about to leave should\n"
    "  // not first boot a socket and pay a curation pass. An explicit exit\n"
    "  // carries no `data-switch`, which is what \"explicit URLs override the\n"
    "  // switch\" means here.\n"
    "  function switchExit(cols) {\n"
    "    if (!cols || cols.getAttribute(\"data-switch\") !== \"1\") return false;\n"
    "    var screens = [], to = cols.getAttribute(\"data-default\") || \"\";\n"
    "    try {\n"
    "      var raw = JSON.parse(cols.getAttribute(\"data-screens\") || \"[]\");\n"
    "      screens = Array.isArray(raw) ? raw : Object.keys(raw);\n"
    "    } catch (e) { screens = []; }\n"
    "    var small = root.matchMedia && root.matchMedia(\"(pointer: coarse) and (max-width: 600px)\").matches;\n"
    "    if (small && screens.indexOf(\"phone\") > -1) to = \"phone\";\n"
    "    if (!to) return false;\n"
    "    var base = root.location.pathname.replace(/\\/+$/, \"\");\n"
    "    root.location.replace(base + \"/\" + to + root.location.search);\n"
    "    return true;\n"
    "  }\n"
    "  if (switchExit(document.querySelector(\".display-columns\"))) return;\n"
    "  // ── Pages (display-hive.md § 7.9) ──────────────────────────────────────\n"
    "  // A page is CONTENT in a window, and what a window is doing is its LEVEL.\n"
    "  // So the sweep below joins a topic while the window STANDS and gives it back\n"
    "  // when it is put away -- never \"while the element exists\". The difference is\n"
    "  // the whole point: a window on level 0 is still in the DOM (the pass computes\n"
    "  // the level, § 4.24, and the sheet hides it), so a hook that counted elements\n"
    "  // would hold a screencast open for every page ever shown until the tab closes.\n"
    "  // The signal is `updated`: LiveView morphs the attribute in place, and neither\n"
    "  // `mounted` nor `destroyed` fires for it.\n"
    "  //\n"
    "  // The level is SYSTEMWIDE (§ 3.1): every output joins or none does. Nothing\n"
    "  // here decides anything about relevance or size, and nothing here sends the\n"
    "  // curator a thing -- a finger in a page is not a gesture of the screen (§ 5.6,\n"
    "  // § 5.11). That the page does not expire under the hand is the CELL's business\n"
    "  // (a throttled `active_at`) together with the APP's (`touched`).\n"
    "  var PAGE_VIEWPORTS = {\n"
    "    tv: { width: 1280, height: 800, dpr: 1, mobile: false },\n"
    "    monitor: { width: 1920, height: 1080, dpr: 1, mobile: false },\n"
    "    phone: { width: 393, height: 852, dpr: 3, mobile: true }\n"
    "  };\n"
    "  // One rejoin, one second later. A `phx_close` on a `page:` topic is the CELL\n"
    "  // letting go -- restarted or replaced -- while the window goes on standing, and\n"
    "  // a screen that gave up here would show a still picture that looks like a live\n"
    "  // page. A topic that turns the second join down IS a refusal, and then the\n"
    "  // frame says so instead of knocking at a dead cell once a second all day. The\n"
    "  // Phoenix client does not do this for us: its rejoin timer is RESET on\n"
    "  // `phx_close`, which only a `phx_error` drives.\n"
    "  var PAGE_REJOIN_MS = 1000;\n"
    "  // The mouse buttons of the wire, in the order the DOM numbers them.\n"
    "  var PAGE_BUTTONS = [\"left\", \"middle\", \"right\", \"back\", \"forward\"];\n"
    "  // The viewport a join declares is the PROFILE of this output (§ 6.6), never a\n"
    "  // measurement: a measured box changes with every patch, the cell would write\n"
    "  // each change into the page as an `Emulation.setDeviceMetricsOverride`, and the\n"
    "  // screen would reflow the streamed page under the reader's hand.\n"
    "  function pageViewport(el) {\n"
    "    return PAGE_VIEWPORTS[el.getAttribute(\"data-exit\") || \"\"] || PAGE_VIEWPORTS.tv;\n"
    "  }\n"
    "  function pageWord(el, word) {\n"
    "    return ((el.getAttribute(\"data-inputs\") || \"\").split(/\\s+/)).indexOf(word) > -1;\n"
    "  }\n"
    "  // What the CLIENT knows, and the only thing it knows: whether its own\n"
    "  // channel stands. It rides `data-page-link` (`up`/`down`) and never\n"
    "  // `data-page-state` -- that one is the curator's word about the CELL, out\n"
    "  // of the seven states the application maps (contracts § 4), and a second\n"
    "  // writer on it made a sleeping page look like a waking one: measured\n"
    "  // 20.09. at the seam, curator `suspended` + a join that worked came out of\n"
    "  // the DOM as `ready`, and the sheet's grayscale rule never drew\n"
    "  // (`display-page-browser.mjs --mode local`, field `asleep`; OR-G.g18.1).\n"
    "  //\n"
    "  // Two voices, two attributes, and the second one carries the words a\n"
    "  // PERSON reads as well: the outline alone left someone in front of an\n"
    "  // empty rectangle with an address in it -- \"no browser mounted\" is a\n"
    "  // different repair from \"unknown page\", and only the line says which one\n"
    "  // it is (contracts § 4). Both are kept on the link, because the server\n"
    "  // renders neither and a morph would take them back (the same trap the\n"
    "  // mark's state line fell into, GH #720).\n"
    "  function pageLink(link, word, why) {\n"
    "    var canvas = link.canvas;\n"
    "    link.word = word;\n"
    "    link.reason = why || \"\";\n"
    "    var fig = canvas.closest ? canvas.closest(\".display-browser\") : null;\n"
    "    if (!fig) return;\n"
    "    fig.setAttribute(\"data-page-link\", word);\n"
    "    if (why) fig.setAttribute(\"data-reason\", why);\n"
    "    else fig.removeAttribute(\"data-reason\");\n"
    "    // Re-said only when the words CHANGED: writing the same sentence into an\n"
    "    // `aria-live` region again is an announcement a person hears twice.\n"
    "    var line = fig.querySelector('[data-role=\"reason\"]');\n"
    "    why = why || \"\";\n"
    "    if (line && line.textContent !== why) line.textContent = why;\n"
    "  }\n"
    "  // The proportions of the frame, set by the HOOK and not by the sheet: the sheet\n"
    "  // cannot know them, and `--browser-ratio` is only the named way in from a\n"
    "  // window. Before the first image they come from `data-viewport` (what the cell\n"
    "  // was asked for), afterwards from every frame head that changes them.\n"
    "  function pageShape(canvas, w, h) {\n"
    "    if (w > 0 && h > 0) canvas.style.aspectRatio = w + \" / \" + h;\n"
    "  }\n"
    "  function pageSaid(canvas) {\n"
    "    var parts = (canvas.getAttribute(\"data-viewport\") || \"\").split(\"x\");\n"
    "    return { w: parseInt(parts[0], 10) || 0, h: parseInt(parts[1], 10) || 0 };\n"
    "  }\n"
    "  // One frame: sixteen bytes of head, then a JPEG.\n"
    "  //\n"
    "  // The head is BIG-endian and carries the PAGE viewport in CSS pixels, not the\n"
    "  // size of the picture -- the cell caps the picture (`screencast.max_*`) and the\n"
    "  // two ends agree on the page's own numbers, which is what a click has to be\n"
    "  // expressed in. `scroll_x`/`scroll_y` are read and deliberately not applied: a\n"
    "  // screencast frame IS the cutout already.\n"
    "  //\n"
    "  // `createImageBitmap` decodes off the main thread and needs neither an `<img>`\n"
    "  // nor an object URL that would have to live until `onload` -- twenty frames a\n"
    "  // second would be twenty URLs a second. The bitmap is closed as soon as it is\n"
    "  // drawn and never outlives its frame.\n"
    "  //\n"
    "  // A frame that arrives while the last one is still decoding is DROPPED, not\n"
    "  // queued: a queue buys latency with memory and ends up showing an old picture.\n"
    "  // The cell is not told; it drops a viewer that cannot keep up itself\n"
    "  // (`client_too_slow`).\n"
    "  //\n"
    "  // `prefers-reduced-motion` does not reach in here. Frames are CONTENT, not the\n"
    "  // screen moving: somebody who wants less motion does not want the page they are\n"
    "  // reading to freeze.\n"
    "  //\n"
    "  // `again` is a REPAINT of the frame already held, not a new one: it neither\n"
    "  // counts nor drops, because the proof counts what the cell sent.\n"
    "  function pageDraw(link, buf, again) {\n"
    "    if (link.busy) { if (!again) link.dropped++; return; }\n"
    "    if (!buf || buf.byteLength < 17) return;\n"
    "    var head = new DataView(buf, 0, 16);\n"
    "    var w = head.getUint32(0), h = head.getUint32(4);\n"
    "    link.scrollX = head.getUint32(8);\n"
    "    link.scrollY = head.getUint32(12);\n"
    "    if (!w || !h) return;\n"
    "    if (!again) { link.last = buf; link.lastW = w; link.lastH = h; }\n"
    "    link.busy = true;\n"
    "    var canvas = link.canvas;\n"
    "    root.createImageBitmap(new Blob([new Uint8Array(buf, 16)])).then(function (bmp) {\n"
    "      // Writing `width` CLEARS the canvas, so it is written only where the shape\n"
    "      // really changed -- otherwise the picture blinks between two frames.\n"
    "      if (canvas.width !== w || canvas.height !== h) {\n"
    "        canvas.width = w;\n"
    "        canvas.height = h;\n"
    "        pageShape(canvas, w, h);\n"
    "      }\n"
    "      link.w = w;\n"
    "      link.h = h;\n"
    "      var g = canvas.getContext(\"2d\");\n"
    "      if (g) g.drawImage(bmp, 0, 0, canvas.width, canvas.height);\n"
    "      if (bmp.close) bmp.close();\n"
    "      if (!again) {\n"
    "        link.frames++;\n"
    "        link.st.pageFrames[link.key] = link.frames;\n"
    "      }\n"
    "      link.busy = false;\n"
    "    }, function () { link.busy = false; });\n"
    "  }\n"
    "  // The picture back onto a canvas that lost it.\n"
    "  //\n"
    "  // Measured in a colony on 2026-09-20 (wave G, g9, finding B-G23): the hook\n"
    "  // writes `canvas.width`, which IS the content attribute `width`, and the\n"
    "  // server renders the `<canvas>` WITHOUT one -- so the next LiveView patch\n"
    "  // reconciles the attribute away. A canvas without `width` is 300x150 again\n"
    "  // and its bitmap is blank. Read off one exit eight seconds after the frame:\n"
    "  // the same node (`link.canvas === the canvas in the document`), `link.w`\n"
    "  // still 1280, `canvas.width` 300, `getAttribute(\"width\")` NULL. An\n"
    "  // animating page hides this, because its next frame sets the size again; a\n"
    "  // page that stands still has no next frame, and its window stays empty for\n"
    "  // good -- which is every PDF, every article, every form.\n"
    "  //\n"
    "  // The repair is the frame already in hand, not a second stream: no roundtrip,\n"
    "  // no keyframe, nothing the cell has to be asked for. It costs one held JPEG\n"
    "  // per page (measured 15.8 kB in wave G) and one decode per patch that\n"
    "  // actually took the size away.\n"
    "  function pageRepaint(link) {\n"
    "    if (!link.last || link.busy) return;\n"
    "    pageDraw(link, link.last, true);\n"
    "  }\n"
    "  function pageMods(e) {\n"
    "    return (e.altKey ? 1 : 0) | (e.ctrlKey ? 2 : 0) | (e.metaKey ? 4 : 0) | (e.shiftKey ? 8 : 0);\n"
    "  }\n"
    "  function pageSend(link, obj) {\n"
    "    if (link.chan) link.chan.push(\"frame\", obj);\n"
    "  }\n"
    "  // Event coordinates into PAGE pixels: x and y scaled on their own, because a\n"
    "  // frame and its box need not have the same proportions for a single patch.\n"
    "  function pageAt(link, e) {\n"
    "    var box = link.canvas.getBoundingClientRect();\n"
    "    var said = pageSaid(link.canvas);\n"
    "    var w = link.w || said.w || box.width;\n"
    "    var h = link.h || said.h || box.height;\n"
    "    return {\n"
    "      x: Math.round((e.clientX - box.left) * (w / (box.width || 1))),\n"
    "      y: Math.round((e.clientY - box.top) * (h / (box.height || 1)))\n"
    "    };\n"
    "  }\n"
    "  function pagePoint(link, touch) {\n"
    "    var box = link.canvas.getBoundingClientRect();\n"
    "    var said = pageSaid(link.canvas);\n"
    "    var w = link.w || said.w || box.width;\n"
    "    var h = link.h || said.h || box.height;\n"
    "    return {\n"
    "      x: Math.round((touch.clientX - box.left) * (w / (box.width || 1))),\n"
    "      y: Math.round((touch.clientY - box.top) * (h / (box.height || 1)))\n"
    "    };\n"
    "  }\n"
    "  // The keyboard goes into a HIDDEN FIELD on `document.body`, and that was\n"
    "  // measured rather than chosen. A canvas is focusable but not editable: the\n"
    "  // engine sends it no `beforeinput`, no `input` and no `composition*`, so every\n"
    "  // dead-key sequence, every emoji out of a picker and every on-screen keyboard\n"
    "  // produces text and no usable `keydown`; and CDP's `Input.insertText` reaches\n"
    "  // nothing on a canvas at all. A field in the TEMPLATE was the other way and is\n"
    "  // worse: it would be patched, read by `phx-change` and reported to an\n"
    "  // application that never asked. So the field hangs outside what LiveView\n"
    "  // patches, for the same reason wave F writes `data-dock-open` on `<html>`.\n"
    "  //\n"
    "  // It is `position: fixed` far off-screen with `opacity: 0` and NOT\n"
    "  // `display: none`: a box that is not rendered takes no focus and opens no\n"
    "  // keyboard on a telephone.\n"
    "  function pageKeys() {\n"
    "    var field = document.createElement(\"input\");\n"
    "    field.className = \"display-browser-keys\";\n"
    "    field.setAttribute(\"type\", \"text\");\n"
    "    field.setAttribute(\"autocomplete\", \"off\");\n"
    "    field.setAttribute(\"autocorrect\", \"off\");\n"
    "    field.setAttribute(\"aria-hidden\", \"true\");\n"
    "    document.body.appendChild(field);\n"
    "    return field;\n"
    "  }\n"
    "  function pageMark(canvas, on) {\n"
    "    var fig = canvas.closest ? canvas.closest(\".display-browser\") : null;\n"
    "    if (!fig) return;\n"
    "    if (on) fig.setAttribute(\"data-keys\", \"true\");\n"
    "    else fig.removeAttribute(\"data-keys\");\n"
    "  }\n"
    "  // What this output can do decides what is wired. A television declares\n"
    "  // `[\"audio\"]` and sends nothing -- the honest answer for a thing that only\n"
    "  // shows. An exit with neither pointer nor touch nor keyboard wires NOTHING: no\n"
    "  // listener, no field, no work.\n"
    "  //\n"
    "  // Every listener hangs on the CANVAS or on the hidden field. None on the root,\n"
    "  // none on `document` in the capture phase, and not one of them calls\n"
    "  // `stopPropagation` -- otherwise the page would swallow the tap of the tile\n"
    "  // beside it, and on a telephone the dock is the only way to a window\n"
    "  // (§ 5.1, § 5.2, § 5.7).\n"
    "  function pageWire(link, el) {\n"
    "    var canvas = link.canvas, offs = [];\n"
    "    function on(node, type, fn) {\n"
    "      node.addEventListener(type, fn);\n"
    "      offs.push(function () { node.removeEventListener(type, fn); });\n"
    "    }\n"
    "    if (pageWord(el, \"pointer\")) {\n"
    "      var send = function (kind) {\n"
    "        return function (e) {\n"
    "          var at = pageAt(link, e);\n"
    "          pageSend(link, { type: \"pointer\", kind: kind, x: at.x, y: at.y,\n"
    "            button: PAGE_BUTTONS[e.button] || \"left\", clicks: e.detail || 1 });\n"
    "        };\n"
    "      };\n"
    "      var up = send(\"up\");\n"
    "      on(canvas, \"pointerdown\", function (e) {\n"
    "        send(\"down\")(e);\n"
    "        if (link.field) link.field.focus();\n"
    "      });\n"
    "      on(canvas, \"pointerup\", up);\n"
    "      // A cancel travels as an `up` and is NOT a sixth type: the system takes the\n"
    "      // pointer away (a system gesture, a menu), and a streamed page that never\n"
    "      // heard the release keeps the button down.\n"
    "      on(canvas, \"pointercancel\", up);\n"
    "      on(canvas, \"pointermove\", send(\"move\"));\n"
    "      on(canvas, \"wheel\", function (e) {\n"
    "        var at = pageAt(link, e);\n"
    "        pageSend(link, { type: \"wheel\", x: at.x, y: at.y, dx: e.deltaX, dy: e.deltaY });\n"
    "      });\n"
    "      // Swallowed, nothing else: a right click over a picture would otherwise\n"
    "      // open the VIEWER's menu over a page rendered somewhere else entirely.\n"
    "      on(canvas, \"contextmenu\", function (e) { e.preventDefault(); });\n"
    "    }\n"
    "    if (pageWord(el, \"touch\")) {\n"
    "      var touches = function (kind) {\n"
    "        return function (e) {\n"
    "          var list = e.changedTouches || [], points = [], i;\n"
    "          for (i = 0; i < list.length; i++) points.push(pagePoint(link, list[i]));\n"
    "          pageSend(link, { type: \"touch\", kind: kind, points: points });\n"
    "        };\n"
    "      };\n"
    "      on(canvas, \"touchstart\", touches(\"start\"));\n"
    "      on(canvas, \"touchmove\", touches(\"move\"));\n"
    "      on(canvas, \"touchend\", touches(\"end\"));\n"
    "    }\n"
    "    if (pageWord(el, \"keyboard\")) {\n"
    "      link.field = pageKeys();\n"
    "      on(link.field, \"keydown\", function (e) {\n"
    "        pageSend(link, { type: \"key\", kind: \"down\", key: e.key, code: e.code,\n"
    "          text: \"\", mods: pageMods(e) });\n"
    "      });\n"
    "      on(link.field, \"keyup\", function (e) {\n"
    "        pageSend(link, { type: \"key\", kind: \"up\", key: e.key, code: e.code,\n"
    "          text: \"\", mods: pageMods(e) });\n"
    "      });\n"
    "      // Whatever the field ends up holding is what was typed -- a dead-key\n"
    "      // sequence, an emoji from a picker, a whole line from a phone's keyboard.\n"
    "      // Read and emptied at once, so the next one starts from nothing.\n"
    "      on(link.field, \"input\", function () {\n"
    "        var text = link.field.value;\n"
    "        link.field.value = \"\";\n"
    "        if (text) pageSend(link, { type: \"text\", text: text });\n"
    "      });\n"
    "      on(link.field, \"focus\", function () { pageMark(canvas, true); });\n"
    "      on(link.field, \"blur\", function () { pageMark(canvas, false); });\n"
    "      on(canvas, \"focus\", function () { if (link.field) link.field.focus(); });\n"
    "    }\n"
    "    return function () {\n"
    "      for (var i = 0; i < offs.length; i++) offs[i]();\n"
    "    };\n"
    "  }\n"
    "  // A topic, opened. EVERY join of this hook goes through here, and a rejoin\n"
    "  // builds a NEW channel rather than joining the old one again -- the shipped\n"
    "  // client throws on the second one (`phoenix.min.js`: `if (this.joinedOnce)\n"
    "  // throw new Error(\"tried to join multiple times\")`), silently, because the\n"
    "  // call sits in a `setTimeout`. And even without the throw the old channel is\n"
    "  // finished: its own `onClose` has already run `socket.remove(this)`, so it\n"
    "  // hangs on nothing and would never see an `image` again. The microphone in\n"
    "  // this same script builds a channel per call for the same reason.\n"
    "  function pageOpen(link, socket, el) {\n"
    "    var vp = pageViewport(el);\n"
    "    // FLAT, one level (OR-G32). The `web` cell puts everything but `mount` into\n"
    "    // `LinkRequest.params`, and the cell reads `params.viewport` -- a payload\n"
    "    // nested one deeper would arrive as `params.params.viewport`, which nobody\n"
    "    // reads, and every page would stand at the shipped default.\n"
    "    var chan = socket.channel(\"page:\" + link.key, {\n"
    "      mount: link.canvas.getAttribute(\"data-mount\") || \"browser\",\n"
    "      viewport: { width: vp.width, height: vp.height, dpr: vp.dpr, mobile: vp.mobile }\n"
    "    });\n"
    "    link.chan = chan;\n"
    "    chan.on(\"image\", function (buf) { pageDraw(link, buf); });\n"
    "    chan.onClose(function () {\n"
    "      // Only the channel that is CURRENT may ask for a successor: a close\n"
    "      // arriving late from one already replaced would open a second stream on\n"
    "      // the same topic, and the cell would send two screencasts for one frame.\n"
    "      if (link.gone || link.chan !== chan || !link.rejoin) return;\n"
    "      link.rejoin = 0;\n"
    "      root.setTimeout(function () {\n"
    "        if (!link.gone && link.chan === chan) pageOpen(link, socket, el);\n"
    "      }, PAGE_REJOIN_MS);\n"
    "    });\n"
    "    chan.join()\n"
    "      .receive(\"ok\", function () {\n"
    "        // A join that WORKED earns the next rejoin, and that is the whole\n"
    "        // budget: a cell restarting again hours later is picked up again,\n"
    "        // while a topic that keeps closing without ever answering is not\n"
    "        // knocked at once a second all day.\n"
    "        link.rejoin = 1;\n"
    "        pageLink(link, \"up\", \"\");\n"
    "      })\n"
    "      .receive(\"error\", function (e) {\n"
    "        // The cell's own word, verbatim: \"no browser mounted\" is a different\n"
    "        // repair from \"unknown page\".\n"
    "        pageLink(link, \"down\", (e && e.reason) || \"refused\");\n"
    "      })\n"
    "      .receive(\"timeout\", function () {\n"
    "        pageLink(link, \"down\", \"the browser never answered\");\n"
    "      });\n"
    "    return chan;\n"
    "  }\n"
    "  function pageJoin(el, st, canvas, key) {\n"
    "    var socket = root.SurfaceSocket && root.SurfaceSocket.getSocket\n"
    "      && root.SurfaceSocket.getSocket();\n"
    "    if (!socket) { pageLink({ canvas: canvas }, \"down\", \"no socket\"); return null; }\n"
    "    var link = { key: key, canvas: canvas, st: st, chan: null, field: null, unwire: null,\n"
    "      frames: st.pageFrames[key] || 0, dropped: 0, busy: false, w: 0, h: 0,\n"
    "      last: null, lastW: 0, lastH: 0,\n"
    "      scrollX: 0, scrollY: 0, rejoin: 1, word: \"\", reason: \"\", gone: false };\n"
    "    var said = pageSaid(canvas);\n"
    "    pageShape(canvas, said.w, said.h);\n"
    "    link.unwire = pageWire(link, el);\n"
    "    pageOpen(link, socket, el);\n"
    "    st.pages[key] = link;\n"
    "    st.pageLinks++;\n"
    "    return link;\n"
    "  }\n"
    "  // The ONE door out of a topic, and all four ways out go through it: level 0,\n"
    "  // the object gone, the hook destroyed, and the driver's own hand. Channel,\n"
    "  // field and listeners are given back in this order, and LETTING THE SENDER GO\n"
    "  // is what the cell reads as \"the viewer left\" -- the last one stops the\n"
    "  // screencast. There is no second message for it.\n"
    "  function pageLeave(st, key) {\n"
    "    var link = st.pages[key];\n"
    "    if (!link) return;\n"
    "    link.gone = true;\n"
    "    link.last = null;\n"
    "    delete st.pages[key];\n"
    "    st.pageLinks--;\n"
    "    if (link.unwire) link.unwire();\n"
    "    // The field hangs on `document.body`, outside everything LiveView clears\n"
    "    // away: one per reconnect would be a leak with a clock on it.\n"
    "    if (link.field && link.field.parentNode) link.field.parentNode.removeChild(link.field);\n"
    "    try { if (link.chan) link.chan.leave(); } catch (e) { /* the socket may be down */ }\n"
    "    st.pageLeft++;\n"
    "  }\n"
    "  function pageSweep(el, st) {\n"
    "    var list = el.querySelectorAll(\"canvas.display-browser-view[data-page]\");\n"
    "    var seen = {}, keys = [], i, key, link, said;\n"
    "    for (i = 0; i < list.length; i++) {\n"
    "      var canvas = list[i];\n"
    "      key = canvas.getAttribute(\"data-page\") || \"\";\n"
    "      // A frame with no stream behind it: the window stands, and that is all\n"
    "      // this one is (§ 6.14).\n"
    "      if (!key) continue;\n"
    "      var win = canvas.closest ? canvas.closest(\"[data-level]\") : null;\n"
    "      // Text, always: the template language reads an `int 0` as empty, so the\n"
    "      // number is parsed here and compared as a number.\n"
    "      var level = win ? parseInt(win.getAttribute(\"data-level\"), 10) : 0;\n"
    "      if (!(level >= 1)) continue;\n"
    "      seen[key] = true;\n"
    "      link = st.pages[key];\n"
    "      if (link && link.canvas === canvas) {\n"
    "        // The node survived the patch and its SIZE did not (B-G23).\n"
    "        if (link.last\n"
    "            && (canvas.width !== link.lastW || canvas.height !== link.lastH)) {\n"
    "          pageShape(canvas, link.lastW, link.lastH);\n"
    "          pageRepaint(link);\n"
    "        }\n"
    "        continue;\n"
    "      }\n"
    "      if (link) {\n"
    "        // The same page on a NEW element: morphdom replaced the canvas. The\n"
    "        // channel stays -- re-joining would cost the cell a whole page.\n"
    "        if (link.unwire) link.unwire();\n"
    "        if (link.field && link.field.parentNode) link.field.parentNode.removeChild(link.field);\n"
    "        link.field = null;\n"
    "        link.canvas = canvas;\n"
    "        link.w = 0;\n"
    "        link.h = 0;\n"
    "        said = pageSaid(canvas);\n"
    "        pageShape(canvas, said.w, said.h);\n"
    "        // The new node came from the SERVER, which renders the curator's\n"
    "        // `data-page-state` and an empty line -- what the CLIENT found out\n"
    "        // about this join is said again, or a refusal would vanish on the\n"
    "        // next unrelated patch. Only the client's own word: the server's is\n"
    "        // freshly rendered and already right (OR-G.g18.1).\n"
    "        if (link.word) pageLink(link, link.word, link.reason);\n"
    "        link.unwire = pageWire(link, el);\n"
    "        // A fresh node is blank, and the channel deliberately stays -- so the\n"
    "        // only picture there will ever be for a page that stands still is the\n"
    "        // one already held (B-G23).\n"
    "        pageRepaint(link);\n"
    "        continue;\n"
    "      }\n"
    "      pageJoin(el, st, canvas, key);\n"
    "    }\n"
    "    for (key in st.pages) keys.push(key);\n"
    "    for (i = 0; i < keys.length; i++) {\n"
    "      if (!seen[keys[i]]) pageLeave(st, keys[i]);\n"
    "    }\n"
    "  }\n"
    "  function pageAll(st) {\n"
    "    var keys = [], key, i;\n"
    "    for (key in st.pages) keys.push(key);\n"
    "    for (i = 0; i < keys.length; i++) pageLeave(st, keys[i]);\n"
    "  }\n"
    "  var hook = {\n"
    "    mounted: function () {\n"
    "      var el = this.el;\n"
    "      var st = { flips: 0, ticks: 0, chimes: 0, ends: 0, clocks: 0, optimistic: 0,\n"
    "                 restored: 0, drawn: {}, audio: null, said: false,\n"
    "                 pages: {}, pageFrames: {}, pageLinks: 0, pageLeft: 0 };\n"
    "      root.__displayScene = st;\n"
    "      // The beat of the ring, and nothing more: which window is up there\n"
    "      // now, since when, when it last rang. It belongs to THIS mount\n"
    "      // (§ 3.2: the dock is the one browser state with meaning), and a\n"
    "      // reconnect starts it afresh -- a memory of what was heard is the\n"
    "      // \"seen\" memory § 4.33 does without.\n"
    "      var seen = { id: \"\", since: 0, last: 0 };\n"
    "      var disarm = arm(st);\n"
    "      // The optimistic half of a tile tap (§ 5.7). Capture phase, so it is\n"
    "      // ahead of LiveView's own click path and the ring is on the tile in\n"
    "      // the same frame the finger lands. The ring is the press; the window\n"
    "      // beside it is the EFFECT, drawn here and confirmed by the pass.\n"
    "      var press = function (e) {\n"
    "        var t = e.target && e.target.closest && e.target.closest(\".display-tile[phx-click]\");\n"
    "        if (!t) return;\n"
    "        t.setAttribute(\"data-zoomed\", \"true\");\n"
    "        root.setTimeout(function () { t.removeAttribute(\"data-zoomed\"); }, dur(el, \"--t-enter\", 360));\n"
    "        optimistic(el, t, st);\n"
    "      };\n"
    "      el.addEventListener(\"pointerdown\", press, true);\n"
    "      // Enter in a `display-input` sends the line; emptying it afterwards is\n"
    "      // the screen's job, because the server never holds the field's value\n"
    "      // as state. It happens in a MACROTASK of its own, and that shape is\n"
    "      // measured rather than argued: the vendored LiveView binds `keyup` on\n"
    "      // `window` and not on this container, and in the bubble phase\n"
    "      // `document` runs BEFORE `window` -- while LiveView reads the value\n"
    "      // synchronously as it pushes. Emptying the field here and now would\n"
    "      // hand the server an empty string, and a sentence somebody typed\n"
    "      // would never become a turn. A macrotask runs after EVERY synchronous\n"
    "      // handler, wherever LiveView hangs its own (OR-F17, OR-F48).\n"
    "      var typed = function (e) {\n"
    "        if (e.key !== \"Enter\") return;\n"
    "        var f = e.target;\n"
    "        if (!f || !f.classList || !f.classList.contains(\"display-input-field\")) return;\n"
    "        root.setTimeout(function () { f.value = \"\"; }, 0);\n"
    "      };\n"
    "      document.addEventListener(\"keyup\", typed);\n"
    "      var iv = root.setInterval(function () { tick(el, st); ring(el, st, seen); }, 1000);\n"
    "      st.tick = function () { tick(el, st); };\n"
    "      st.ring = function () { ring(el, st, seen); };\n"
    "      st.clock = function () { clocks(el, st); };\n"
    "      st.pageSweep = function () { pageSweep(el, st); };\n"
    "      this.__scene = { st: st, before: scan(el), held: [], seen: seen, iv: iv, disarm: disarm,\n"
    "                       press: press, typed: typed };\n"
    "      tick(el, st); ring(el, st, seen); clocks(el, st);\n"
    "      pageSweep(el, st);\n"
    "      toEnd(el, [], st);\n"
    "    },\n"
    "    beforeUpdate: function () {\n"
    "      if (!this.__scene) return;\n"
    "      this.__scene.before = scan(this.el);\n"
    "      this.__scene.held = held(this.el);\n"
    "    },\n"
    "    updated: function () {\n"
    "      if (!this.__scene) return;\n"
    "      // Before the movement, always: `flip` compares the snapshot with what stands\n"
    "      // NOW, so a drawing put back after it would be a movement nobody asked for.\n"
    "      redraw(this.el, this.__scene.st);\n"
    "      flip(this.el, this.__scene.before, this.__scene.st);\n"
    "      tick(this.el, this.__scene.st);\n"
    "      ring(this.el, this.__scene.st, this.__scene.seen);\n"
    "      clocks(this.el, this.__scene.st);\n"
    "      toEnd(this.el, this.__scene.held || [], this.__scene.st);\n"
    "      // Last, and after the patch: a page joins while its window STANDS,\n"
    "      // and the level it stands on is what the patch has just written.\n"
    "      pageSweep(this.el, this.__scene.st);\n"
    "    },\n"
    "    destroyed: function () {\n"
    "      if (!this.__scene) return;\n"
    "      root.clearInterval(this.__scene.iv);\n"
    "      // Both handles out of the bag first: what is given back has to read\n"
    "      // as the same two names it was registered with.\n"
    "      var press = this.__scene.press, typed = this.__scene.typed;\n"
    "      this.el.removeEventListener(\"pointerdown\", press, true);\n"
    "      document.removeEventListener(\"keyup\", typed);\n"
    "      this.__scene.disarm();\n"
    "      // Every topic this hook still holds, through the one door.\n"
    "      pageAll(this.__scene.st);\n"
    "      if (this.__scene.st.audio) { try { this.__scene.st.audio.close(); } catch (e) { /* gone */ } }\n"
    "      this.__scene = null;\n"
    "    }\n"
    "  };\n"
    "  root.SurfaceHooks = Object.assign(root.SurfaceHooks || {}, { DisplayScene: hook });\n"
    "})(window);\n"
)


def _c(name, template, props, layer="content"):
    """One `component.define` argument bag of the catalogue.

    `editable` is empty on every component of the catalogue: a prop a browser
    may write is an authorisation, and nothing on these screens is dragged.
    """
    return {
        "name": name,
        "template": template,
        "prop_schema": props,
        "editable": [],
        "layer": layer,
    }


# What the curator writes on every window, and the hints an application may
# send about one (GH #679). `since`, `score` and `relevance` travel as text:
# the template language reads an `int 0` as empty, and `as_unit()` reads a
# number out of a string. The `web` cell refuses an undeclared prop, so a
# hint has to stand here before an application may say it.
CURATED = {
    # The app's hints (§ 3): what an application may say about its own window. Numbers
    # travel as TEXT, because the template language reads an `int 0` as empty and § 3.3
    # says so in as many words; the `web` cell refuses an undeclared prop, so a hint has
    # to stand here before an application may send it.
    "context": "text", "relevance": "text", "class": "text", "pinned": "boolean",
    "relevant_until": "text", "touched": "text", "topic": "text", "layer": "text",
    "seat": "text", "seat_ord": "text", "linger": "text", "state": "text",
    # § 8.3: the turn a window came out of. The chat closes on it (§ 4.13).
    "turn_id": "text",
    # The curator's rendering values (§ 3.1): systemwide, the same on every output.
    # `level` is text for the same reason `since` is -- `data-level=""` matches no rule.
    "rung": "text", "level": "text", "front": "text", "age": "text", "led": "text",
    "since": "text", "score": "text", "region": "text",
    # The stamp the client reads to tell a patch of its own tap's pass from a patch of
    # one that started earlier (GH #744, § 5.7).
    "acted": "text",
}


def windows():
    """Catalogue A: the four glass components, all `layer: "navigation"`.

    The ornament is a thing, not a place: a component an application hangs
    into its view, which the sheet fixes to the bottom edge. The two regions
    stay the only places on this screen.
    """
    return [
        _c("display-pane", PANE_TEMPLATE, dict({
            "pane_id": "text", "kicker": "text", "title": "text",
            "thin": "boolean", "tone": "text",
        }, **CURATED), "navigation"),
        _c("display-panel", PANEL_TEMPLATE, dict({
            "pane_id": "text", "title": "text", "scroll": "boolean", "tone": "text",
        }, **CURATED), "navigation"),
        _c("display-overlay", OVERLAY_TEMPLATE, dict({
            "pane_id": "text", "title": "text",
            "body": "text", "ttl_ms": "int", "position": "text",
        }, **CURATED), "navigation"),
        _c("display-ornament", ORNAMENT_TEMPLATE, {
            "text": "text", "dot": "boolean", "count": "int",
        }, "navigation"),
    ]


def contents():
    """Catalogue B: the twenty-seven content components, all `layer: "content"`.

    None of them writes glass: a content component sits on a window's
    `.inner` fill, and the `web` cell refuses the material on this layer.
    """
    return [
        _c("display-value", VALUE_TEMPLATE, {
            "value": "text", "unit": "text", "label": "text",
            "trend": "text", "size": "text"}),
        _c("display-text", TEXT_TEMPLATE, {"body": "text", "secondary": "boolean"}),
        _c("display-voice", VOICE_TEMPLATE, {
            "text": "text", "accent": "text", "size": "text"}),
        _c("display-kicker", KICKER_TEMPLATE, {"text": "text"}),
        _c("display-list", LIST_TEMPLATE, {"title": "text", "plain": "boolean"}),
        _c("display-item", ITEM_TEMPLATE, {
            "k": "text", "v": "text", "marker": "text", "accent": "boolean"}),
        _c("display-table", TABLE_TEMPLATE, {
            "caption": "text", "head": "html", "rows": "html"}),
        _c("display-weather", WEATHER_TEMPLATE, {
            "temp": "text", "unit": "text", "condition": "text",
            "glyph": "text", "hi": "text", "lo": "text", "place": "text"}),
        _c("display-clock", CLOCK_TEMPLATE, {
            "time": "text", "date": "text", "zone": "text", "size": "text"}),
        _c("display-timer", TIMER_TEMPLATE, {
            "label": "text", "remaining": "text", "end_at": "int",
            "total_ms": "int", "now": "int", "done": "boolean"}),
        _c("display-chat", CHAT_TEMPLATE, {"title": "text"}),
        _c("display-chat-line", CHAT_LINE_TEMPLATE, {
            "role": "text", "text": "text", "channel": "text", "source": "text",
            "at": "int", "partial": "boolean"}),
        _c("display-notification", NOTIFICATION_TEMPLATE, {
            "source": "text", "title": "text", "body": "text",
            "time": "text", "level": "text"}),
        _c("display-media", MEDIA_TEMPLATE, {
            "src": "text", "alt": "text", "caption": "text",
            "figure": "html", "ratio": "text"}),
        # New in 2.6.0: a live page, drawn into the canvas by the scene hook at
        # the root (R-G8) rather than by a script of its own. `mount` is the one
        # prop an application leaves empty -- the screen writes it in `add_tree`
        # out of `params.browser_mount`, the way it writes `for` on a field.
        _c("display-browser", BROWSER_TEMPLATE, {
            "page": "text", "mount": "text", "url": "text",
            "title": "text", "viewport": "text", "state": "text"}),
        _c("display-document", DOCUMENT_TEMPLATE, {
            "title": "text", "body": "html", "page": "int",
            "pages": "int", "source": "text"}),
        _c("display-status", STATUS_TEMPLATE, {"kind": "text", "text": "text"}),
        _c("display-action", ACTION_TEMPLATE, {
            "label": "text", "event": "text", "primary": "boolean"}),
        _c("display-choice", CHOICE_TEMPLATE, {"label": "text"}),
        _c("display-option", OPTION_TEMPLATE, {
            "label": "text", "event": "text", "selected": "boolean"}),
        _c("display-input", INPUT_TEMPLATE, {
            "placeholder": "text", "event": "text", "for": "text"}),
        _c("display-chart", CHART_TEMPLATE, {"figure": "html", "caption": "text"}),
        _c("display-stack", STACK_TEMPLATE, {
            "row": "boolean", "gap": "text", "scene": "boolean",
            "ratio": "text"}),
        _c("display-progress", PROGRESS_TEMPLATE, {
            "value": "int", "label": "text"}),
        _c("display-dock", DOCK_TEMPLATE, {"count": "int"}),
        _c("display-seat", SEAT_TEMPLATE, {"seat_ord": "text"}),
        _c("display-tile", TILE_TEMPLATE, {
            "glyph": "text", "line": "text", "value": "text", "topic": "text",
            "for": "text", "rung": "text", "open": "text",
            "pinned": "text", "rank": "text", "end_at": "int",
            # New in 2.4.0. `oid` is the OBJECT id of the window and the target of
            # a tap (`for` stays the `pane_id`, which is what the client's
            # zoom matches on -- two names, two jobs, and merging them would
            # break one of them). `tap` says whether this exit has a finger at
            # all (OR-F18); `seat` and `unread` are selectors for the sheet,
            # and `unit` is the degree sign the weather sets beside its value.
            "oid": "text", "tap": "boolean", "seat": "text",
            "unit": "text", "unread": "text"}),
    ]


def components():
    """The display's own components, in the order they are defined.

    First the five the screen is made of, then the catalogue: the windows and
    the content an application may name without defining them. A component
    has to exist before an object names it, and the legs of a bundle run in
    call order.

    None of them is `editable`. A prop a browser may write is an authorisation
    an application grants over its OWN component; the frame around it is not a
    thing anybody drags.
    """
    return own() + windows() + contents()


def own():
    """The five the screen is made of: shell, regions, two wrappers, the OS mark."""
    return [
        {
            # `faces` is typed `"html"` because that is what makes a prop RAW:
            # an `@font-face` block rendered escaped is not an `@font-face`.
            "name": "display-shell",
            "template": SHELL_TEMPLATE,
            # `vocab` is never rendered: the template does not name it. It is a
            # note the root carries about which vocabulary it was built against
            # (see VOCAB), so a later pass can read it back.
            "prop_schema": {"stylesheet": "boolean", "faces": "html", "vocab": "text",
                            # The screen's state (GH #679): the bar, the
                            # per-context weights, when the judge last spoke,
                            # and the schedule the clock holds. None rendered.
                            "due": "text",
                            # The operator's ground: `day` or `night`.
                            "ground": "text",
                            # The screen this tree is rendered for (§ 2.8):
                            # which exit of the one screen state it is, which
                            # kind of display, which inputs that display has,
                            # and the one number every size derives from. The
                            # floor computes them from the `screens` setting;
                            # the sheet reads them off the root.
                            "exit": "text", "screen_name": "text",
                            "inputs": "text", "scale": "text",
                            "default_screen": "text",
                            # § 6.5: the root of `/<mount>/` is the switch and says
                            # so; an explicit output carries an empty word. § 6.4:
                            # whether a tap and the input line are bound here.
                            "switch": "text", "tap": "boolean",
                            "input_line": "boolean",
                            # What did not fit in the dock, and the exits as
                            # JSON: the sheet may read the first (since 2.4.0
                            # the judge does not, R-23-6), and the floor
                            # compares the second to know whether the routes
                            # have to be written again.
                            "screens": "text",
                            # The exit's own dials (contract § 6): whether the
                            # dock is shown at all on this kind of screen, how
                            # many tiles it carries and how many windows may
                            # stand on plane 1 here. One state, and each exit
                            # renders as much of it as it can carry (R-23-6).
                            "dock": "text", "dock_max": "int",
                            # The screen's own motion, once the scene hook
                            # ships; empty until then. `"html"` is what makes
                            # a prop RAW, and a script rendered escaped is a
                            # script that does nothing.
                            "client_js": "html"},
            "editable": [],
            "layer": "content",
        },
        {
            # One per entry in REGIONS, all of them direct children of the
            # root. That USED to be impossible: a materialised page carried two
            # statics whatever the child count, so the closing static landed
            # between the first and the second child and everything from the
            # second on rendered outside the element meant to contain it. GH
            # #394 replaced that with n+1 statics for n slots, and the `web`
            # README says so in as many words -- "a composition CHOICE now
            # rather than a constraint". So the one-child rule is retracted
            # here too, and the two regions stand side by side (GH #609).
            "name": "display-region",
            "template": REGION_TEMPLATE,
            "prop_schema": {"region": "text"},
            "editable": [],
            "layer": "content",
        },
        {
            # Navigation, because it writes `glass--thin`, and glass is a
            # navigation-layer material. A `layer: "content"` component that
            # names one of the three glass classes is refused by the `web` cell
            # at `component.define`.
            "name": "display-view-prose",
            "template": PROSE_TEMPLATE,
            # A prose view IS a window (§ 2 Window), so it declares the same contract as
            # the other three: the hints of § 3 and the curator's rendering values. One
            # list, one place -- a schema that drifted from `CURATED` refused exactly the
            # props the pass had just written.
            "prop_schema": dict({
                "view_id": "text", "owner": "text", "title": "text", "body": "text",
            }, **CURATED),
            "editable": [],
            "layer": "navigation",
        },
        {
            # Content, and that is the load-bearing half: an application's own
            # glass card sits INSIDE this wrapper, and glass on glass is refused
            # where the edge is made. Glass on a content parent is allowed, so a
            # content wrapper is what lets an app bring its own pane.
            "name": "display-view-custom",
            "template": CUSTOM_TEMPLATE,
            "prop_schema": {"view_id": "text", "owner": "text"},
            "editable": [],
            "layer": "content",
        },
        {
            # The OS mark (§ 2.6). `client_js` is typed `"html"` because that
            # is what makes a prop RAW: a script rendered escaped is a script
            # that does nothing. Everything else stays escaped, and `mount`
            # with it -- a mount name is configuration and must not be able to
            # close a tag. The name changed with 2.3.0 and the id with it; the
            # HOOK kept its name, because the gesture is the same one.
            "name": "display-os",
            "template": OS_TEMPLATE,
            "prop_schema": {"mount": "text", "client_js": "html",
                            # The light on the mark (OR-F4): how many present
                            # windows want attention and are not on a plane.
                            # Text, because the template language reads an
                            # `int 0` as empty and the sheet selects on the
                            # value.
                            "unseen": "text"},
            "editable": [],
            "layer": "content",
        },
    ]


# The fingerprint of the vocabulary: `components()` as canonical JSON, hashed,
# cut to twelve hex characters. Computed ONCE, at import, because the list is
# a constant of the code that runs -- a screen whose root carries another
# fingerprint was built by other code, and gets every definition again
# (`patches()`). Twelve characters are 48 bits: enough that two vocabularies
# never collide by accident, short enough to read in a prop (GH #670).
VOCAB = hashlib.sha256(
    json.dumps(components(), sort_keys=True).encode("utf-8")
).hexdigest()[:12]


# ---------------------------------------------------------------------------
# Wire helpers


def now_ms():
    """Epoch milliseconds. `updated_at` is an int and never a formatted date.

    `_TEST_NOW` is the scenario driver's clock (`compose/scenarios/curator_driver.py`,
    OR-D10): it sets the name in the resident globals dict before a message. In a colony
    nobody can reach that dict, so the wall clock is all there is.
    """
    held = globals().get("_TEST_NOW")
    return int(held) if held is not None else int(time.time() * 1000)


def tool_call(args, tid):
    """One operation, as a UBF `tool_call` turn."""
    return {
        "origin": "assistant",
        "type": "tool_call",
        "id": tid,
        "text": json.dumps(args, sort_keys=True),
    }


def text_turn(text):
    """One plain turn. `messages` is mandatory on every body that crosses the
    substrate: a body without it is refused as `invalid_ubf_body` before it
    reaches an edge, which shows up as a dead letter rather than as an answer.
    """
    return {"origin": "assistant", "type": "text", "text": text}


def emission(route, body, **header):
    """One message on `route`, carrying `body`'s slots."""
    head = {"route": route}
    head.update(header)
    out = {"header": head}
    out.update(body)
    return out


def canon(value):
    """One byte form for a JSON value, whatever side it came from.

    A `json` column is stored as TEXT and read back as a string, while a value
    this cell just built is still a Python object. Comparing the two directly
    would report a difference that is only a serialisation, which is exactly
    the comparison the component dedup rests on.
    """
    if isinstance(value, str):
        try:
            value = json.loads(value)
        except (TypeError, ValueError):
            return value
    return json.dumps(value, sort_keys=True)


def refuse(code, detail, view_id, owner):
    """ONE emission on the `receipt` lane, and nothing else.

    All four keys are always present, empty where unknown: the member that
    routes a receipt back reads `owner`, and a key that is sometimes missing is
    a router branch nobody tests.

    The same two keys ride on the HOP as well, and that is not a duplication for
    convenience: an edge condition in this substrate sees `context.*` and
    `hop.*` and nothing else (`crates/meclaw-colony/src/cel_eval.rs`, `bind_ctx`),
    so a receipt whose owner lives only in the body cannot be routed back to
    that owner at all. The body copy is what the receiving cell reads; the hop
    copy is what the member's graph reads (GH #459).
    """
    return [
        emission(
            "receipt",
            {
                "messages": [text_turn("%s: %s" % (code, detail))],
                "receipt": {
                    "error_code": code,
                    "view_id": view_id or "",
                    "owner": owner or "",
                    "detail": detail,
                },
            },
            owner=owner or "",
            view_id=view_id or "",
        )
    ]


# ---------------------------------------------------------------------------
# Pass 1: a request


def is_view_id(value):
    """`[a-z0-9-]{1,64}`, and nothing looser.

    The id ends up inside an object id and inside a `data-view` attribute, so
    the set is closed at the door rather than escaped at every use site.
    """
    return (
        isinstance(value, str)
        and 1 <= len(value) <= ID_MAX
        and all(c in ID_CHARS for c in value)
    )


def is_node_key(value):
    """Whether a component-tree node may name its own identity with this.

    A key stands where the child index would stand, so it must not carry the
    separator the index chain is written with, and it must not be empty -- an
    id with an empty segment is two different ids depending on who splits it.
    Everything else a path can hold is allowed, because the keys that matter
    are paths: `parse_object_id` only ever reads the FIRST segment of an id, so
    a `~` or a `.` further along is already in the language.
    """
    return (
        isinstance(value, str)
        and 1 <= len(value) <= KEY_MAX
        and "/" not in value
    )


def check_node(node, depth):
    """A component-tree node, or the reason it is not one."""
    if depth > MAX_DEPTH:
        return "the component tree is deeper than %d levels" % MAX_DEPTH
    if not isinstance(node, dict):
        return "a component tree node is not an object"
    name = node.get("component")
    if not isinstance(name, str) or not name:
        return 'a component tree node carries no "component" name'
    if not isinstance(node.get("props", {}), dict):
        return '"props" is not an object'
    keep = node.get("keep", [])
    if not isinstance(keep, list) or not all(isinstance(k, str) for k in keep):
        return '"keep" is not a list of prop names'
    key = node.get("key")
    if key is not None and not is_node_key(key):
        return '"key" is not a usable object key'
    kids = node.get("children", [])
    if not isinstance(kids, list):
        return '"children" is not a list'
    # A key stands where the index would stand, so two siblings naming the same
    # one mint the same object id and the second would simply overwrite the
    # first -- an accepted view with a node missing from the screen and nothing
    # in the receipt to act on (GH #568). A key that is a plain number is the
    # same collision one door along: `"3"` names exactly what the unkeyed
    # fourth child beside it names. Both are refused here, where the sender is
    # still being told why.
    seen = set()
    for kid in kids:
        why = check_node(kid, depth + 1)
        if why:
            return why
        kid_key = kid.get("key") if isinstance(kid, dict) else None
        if not is_node_key(kid_key):
            continue
        if kid_key.isdigit():
            return (
                '"key" %r is a number and would collide with the index language'
                % (kid_key,)
            )
        if kid_key in seen:
            return '"key" %r collides with a sibling under %s' % (kid_key, name)
        seen.add(kid_key)
    return None


def check_components(declared, view_id):
    """The `component.define` arguments a view brings, or the reason they fail.

    Every name must start with `<view_id>-`. The component library of a display
    is ONE namespace shared by every application writing to that screen, so a
    prefix is what keeps two apps from redefining each other's vocabulary out
    from under a page that is already rendered.
    """
    if not isinstance(declared, list):
        return None, "invalid_view", '"components" is not a list'
    out = []
    for item in declared:
        if not isinstance(item, dict):
            return None, "invalid_view", "a component definition is not an object"
        name = item.get("name")
        if not isinstance(name, str) or not name:
            return None, "invalid_view", 'a component definition carries no "name"'
        if not name.startswith(view_id + "-"):
            return (
                None,
                "component_prefix",
                "the component %r does not start with %r" % (name, view_id + "-"),
            )
        if not isinstance(item.get("template"), str):
            return None, "invalid_view", 'the component %r has no "template"' % name
        if not isinstance(item.get("prop_schema"), dict):
            return None, "invalid_view", 'the component %r has no "prop_schema"' % name
        out.append(
            dict(
                (k, item[k])
                for k in ("name", "template", "prop_schema", "editable", "layer")
                if k in item
            )
        )
    return out, None, None


# The wire of § 3.3, for the hints the PASS reads and for no other prop. `pinned` is the
# one boolean; everything else a hint says is TEXT, the five numbers included -- the
# template language reads an `int 0` as empty, so a number on the wire would arrive as
# "nothing said" somewhere down the line. Whatever else a window component declares
# (`thin`, `scroll`, `position`, an overlay's own `ttl_ms`) is that component's business
# and is typed by the `web` cell against its `prop_schema`, not here.
BOOL_HINTS = ("pinned",)
TEXT_HINTS = ("context", "class", "topic", "layer", "seat", "state", "turn_id")


def hint_shape(hints):
    """Why a hint does not arrive in the shape § 3.3 names, or None.

    The WIRE, not the vocabulary: which words a hint may carry is `step2_door`'s question
    (§ 4.6) and is asked right after this. This one exists because the pass is the
    reference model: a list where it expects a word does not come back as a refusal
    there, it raises -- and the whole write pass dies with it instead of answering the
    sender a receipt they can read.
    """
    for key in BOOL_HINTS:
        if key in hints and not isinstance(hints[key], bool):
            return 'hint "%s" is not a boolean (§ 3.3)' % (key,)
    for key in TEXT_HINTS:
        if key in hints and not isinstance(hints[key], str):
            return 'hint "%s" is not text (§ 3.3)' % (key,)
    for key in NUMERIC_HINTS:
        if key in hints and not isinstance(hints[key], str):
            return 'hint "%s" travels as text, not as a number (§ 3.3)' % (key,)
    return None


def door_refusal(row):
    """Why the door would not take this row, or None. ONE door, two questions.

    First the wire of § 3.3 (`hint_shape`), then the words of § 4.6: the write lane asks
    exactly the `step2_door` the pass asks, on a throwaway state. The words it knows
    (`state`, `seat`, `ttl_ms`) live in the pass section, which is byte-identical with
    the reference model, so a change to § 4.6 in the document moves ONE implementation
    instead of two that drift apart. The throwaway state carries the shipped settings and
    no screens -- the door's profile half has nothing to say about a view, and its
    `default_screen` error lands in `errors`, which this never reads.
    """
    hints = hints_of_row(row)
    why = hint_shape(hints)
    if why:
        return why
    oid = object_id(row.get("owner"), row.get("view_id"))
    state = {"settings": dict(DEFAULT_SETTINGS), "screens": {}, "views": {},
             "pass": {"refused": [], "errors": [], "new": [], "touched": {}}}
    step2_door(state, {"kind": "app_write", "oid": oid, "view": hints}, 0)
    for entry in state["pass"]["refused"]:
        if str(entry[0]) == oid:
            return 'the door does not take "%s" %r (§ 4.6)' % (entry[1], entry[2])
    return None


def validate(body, owner, withdraw):
    """`(row, error_code, detail)` -- exactly one of the first and the second."""
    view_id = body.get("view_id")
    if not is_view_id(view_id):
        return None, "invalid_view", '"view_id" must match [a-z0-9-]{1,64}'

    # The body may repeat the owner. It may not disagree with the envelope: a
    # sender that could name somebody else's owner could withdraw their views.
    claimed = body.get("owner")
    if claimed is not None and claimed != owner:
        return (
            None,
            "not_owner",
            "the body claims owner %r, the sender is %r" % (claimed, owner),
        )

    if withdraw:
        return {"owner": owner, "view_id": view_id}, None, None

    region = body.get("region")
    if region is None:
        region = REGIONS[0]
    if region not in REGIONS:
        return None, "invalid_view", "unknown region %r" % (region,)

    kind = body.get("kind")
    if kind not in ("prose", "component"):
        return None, "invalid_view", 'unknown kind %r ("prose" or "component")' % (kind,)

    content = body.get("content")
    if not isinstance(content, dict):
        return None, "invalid_view", '"content" is not an object'

    declared = body.get("components")
    if kind == "prose":
        if not isinstance(content.get("body"), str):
            return None, "invalid_view", 'a prose view needs a "body" string'
        title = content.get("title")
        if title is not None and not isinstance(title, str):
            return None, "invalid_view", 'a prose "title" is not a string'
        if declared:
            return None, "invalid_view", "a prose view brings no components"
        clean = []
    else:
        why = check_node(content, 0)
        if why:
            return None, "invalid_view", why
        clean, code, detail = check_components(
            declared if declared is not None else [], view_id
        )
        if code:
            return None, code, detail

    ttl_ms = body.get("ttl_ms")
    if ttl_ms is None:
        ttl_ms = 0
    if isinstance(ttl_ms, bool) or not isinstance(ttl_ms, int) or ttl_ms < 0:
        return None, "invalid_view", '"ttl_ms" is not a non-negative integer'

    # Signed and unbounded on purpose: it is compared, never used as an index,
    # and a widget that wants to stand above everything says so with a negative
    # number instead of asking every other sender to move down.
    view_ord = body.get("ord")
    if view_ord is None:
        view_ord = 0
    if isinstance(view_ord, bool) or not isinstance(view_ord, int):
        return None, "invalid_view", '"ord" is not an integer'

    row = {
        "owner": owner,
        "view_id": view_id,
        "region": region,
        "ord": view_ord,
        "kind": kind,
        # The two `json` columns are written as canonical text so the value
        # that comes back out of the store compares byte for byte against
        # the value that went in.
        "content": canon(content),
        "components": canon(clean),
        "ttl_ms": ttl_ms,
        "updated_at": now_ms(),
    }
    # And last, the one door (§ 4.6): a word the pass would refuse is refused here, so
    # the row never reaches the store (S-039).
    why = door_refusal(row)
    if why:
        return None, "view_refused", why
    return row, None, None


def pass_request(body, envelope, withdraw):
    """A write lane: validate, then ONE store bundle."""
    owner = envelope.get("reply_to")
    if not isinstance(owner, str) or not owner:
        return refuse(
            "owner_unknown",
            "the message carries no envelope.reply_to, so it has no owner",
            body.get("view_id") if isinstance(body.get("view_id"), str) else "",
            "",
        )

    row, code, detail = validate(body, owner, withdraw)
    if code:
        vid = body.get("view_id")
        return refuse(code, detail, vid if isinstance(vid, str) else "", owner)
    return accept_row(owner, row, withdraw)


def write_row(owner, row, withdraw=False):
    """ONE store bundle that puts `row` up under `(owner, view_id)`, or takes it down.

    The same bundle for a view and for a notice: delete what stood under the name, then
    insert the row. No select any more (GH #809): the cell holds the rows in memory
    (`ram()["rows"]`), so the before-state is already known. The mark on the hop is
    small and says only whose write this was -- enough for a refused leg to become a
    receipt to that app, and nothing the screen is drawn from.
    """
    view_id = row["view_id"]
    legs = [
        tool_call(
            {
                "operation": "delete",
                "table": TABLE,
                "where": {"owner": owner, "view_id": view_id},
            },
            "d-delete",
        ),
    ]
    if not withdraw:
        legs.append(
            tool_call({"operation": "insert", "table": TABLE, "row": row}, "d-insert")
        )
    mark = {"write": {"owner": owner, "view_id": view_id, "withdraw": bool(withdraw)}}
    return [
        emission(
            "views",
            {"messages": legs},
            display_request=json.dumps(mark, sort_keys=True),
        )
    ]


def accept_row(owner, row, withdraw):
    """A write the door took: the store bundle now, the row in memory, the pass of it.

    The store bundle leaves in this turn whatever phase the cell is in -- even while it
    is still booting, a write is the app's and belongs in the store at once (plan D1a
    § 2.2). `define` is the app's vocabulary, and it only travels when it CHANGED: a
    `component.define` re-renders every route in the display, so an app that ticks once
    a second and re-sends the same definitions would re-render the whole screen once a
    second for no difference at all.
    """
    r = ram()
    oid = object_id(owner, row["view_id"])
    prior = r["rows"].get(oid)
    ops = write_row(owner, row, withdraw)
    if withdraw:
        r["rows"].pop(oid, None)
        return pass_or_queue({"kind": "app_withdraw", "oid": oid}, now_ms(), ops)
    r["rows"][oid] = row
    define = []
    if prior is None or canon(prior.get("components")) != row["components"]:
        parsed = json.loads(row["components"])
        define = parsed if isinstance(parsed, list) else []
    event = {"kind": "app_write", "oid": oid, "view": hints_of_row(row)}
    return pass_or_queue(event, int(row["updated_at"]), ops, define)


# A channel's failure, translated. The codes are the substrate's public error_code
# strings; a code this table does not know is still shown, with the code in it.
NOTICE_TEXT = {
    "stt_failed": "The microphone did not catch that.",
    "speak_failed": "The voice could not speak just now.",
    "busy": "The call did not go through.",
    "no_answer": "The call did not go through.",
    "call_refused": "The call did not go through.",
    "failed": "The telephone line failed.",
    "line_write_failed": "The telephone line failed.",
}
NOTICE_FALLBACK = "A part of the colony failed: %s"


def owner_slug(owner):
    """The context a sender's own windows stand in when they name none."""
    return str(owner or "").replace("/", "~")


def notice_row(body, hop, owner, now, knobs=None):
    """The view row an `in_notice` becomes, or (None, code, detail)."""
    code = str(hop.get("error_code") or "")
    klass = str(body.get("class") or ("system_error" if code else ""))
    if klass not in NOTICE_DEFAULTS:
        return None, "invalid_notice", 'unknown "class" %r' % (klass,)
    text = body.get("text")
    if not isinstance(text, str) or not text:
        # A channel's failure says nothing but its code; the table speaks for
        # it, and `meta.detail` is never read.
        text = NOTICE_TEXT.get(code, NOTICE_FALLBACK % code) if code else None
    if not text:
        return None, "invalid_notice", 'a notice needs a "text" string'
    rel, ttl = NOTICE_DEFAULTS.get(klass, (DEFAULT_WEIGHT, 0))
    context = str(body.get("context") or ("system" if klass == "system_error" else owner_slug(owner)))
    digest = hashlib.sha256(text.encode("utf-8")).hexdigest()[:8]
    view_id = str(body.get("view_id") or ("notice-%s-%s" % (klass.replace("_", "-"), digest)))
    if not is_view_id(view_id):
        return None, "invalid_notice", '"view_id" must match [a-z0-9-]{1,64}'
    content = {"title": klass.replace("_", " "), "body": text, "context": context,
               # § 3.3: a numeric hint travels as TEXT. A notice is a view like any
               # other and goes on the same wire, so the door takes it.
               "relevance": str(as_unit(body.get("relevance"), rel)), "class": klass}
    ttl_ms = body.get("ttl_ms")
    if isinstance(ttl_ms, bool) or not isinstance(ttl_ms, int) or ttl_ms < 0:
        ttl_ms = ttl
    return {"owner": owner, "view_id": view_id, "region": "main", "ord": 0, "kind": "prose",
            "content": canon(content), "components": "[]", "ttl_ms": ttl_ms,
            "updated_at": now}, None, None


def pass_notice(body, envelope, hop):
    """A classified message becomes a prose view of its sender: the same bundle as `in_view`."""
    owner = envelope.get("reply_to")
    if not isinstance(owner, str) or not owner:
        return refuse("owner_unknown", "the message carries no envelope.reply_to, so it has no owner", "", "")
    row, code, detail = notice_row(body, hop, owner, now_ms(), None)
    if code:
        vid = body.get("view_id")
        return refuse(code, detail, vid if isinstance(vid, str) else "", owner)
    return accept_row(owner, row, False)


def parse_object_id(oid):
    """`(owner, view_id)` out of an object id, or `(None, None)`.

    The inverse of how an id is built: `view.<slug>.<view_id>/<i>/<j>`, where
    the slug is the owner path with `/` written as `~` -- a path segment inside
    an id would otherwise be indistinguishable from the child index chain.
    """
    if not isinstance(oid, str) or not oid.startswith(VIEW_PREFIX):
        return None, None
    wrapper = oid.split("/")[0]
    rest = wrapper[len(VIEW_PREFIX) :]
    if "." not in rest:
        return None, None
    slug, view_id = rest.rsplit(".", 1)
    if not slug or not is_view_id(view_id):
        return None, None
    return slug.replace("~", "/"), view_id


# The keys of `event.value` that may carry an object id, in the order they are
# asked. The catalogue writes exactly one `phx-value-*` -- `for`, on a tile and
# on the input line -- and a window's own button may name `id` beside it to say
# WHICH action it is; those two are the whole set, and a name outside it is
# something a person did rather than something the screen wrote.
#
# Asking every string of the payload instead read the typed sentence as a
# candidate: `{"for": "", "key": "Enter", "value": "view.mallory.evil/0"}` came
# back as owner `mallory`, view `evil`, so a person could route their own line
# to somebody else's view by typing its id. A filled `for` won that race by
# alphabet alone, which is not a rule -- it is an accident of two key names.
ID_KEYS = ("id", "for")


def event_object_id(event):
    """The object id a browser event names, preferring the key `id`."""
    value = event.get("value")
    if isinstance(value, str):
        return value if value.startswith(VIEW_PREFIX) else None
    if not isinstance(value, dict):
        return None
    for key in ID_KEYS:
        candidate = value.get(key)
        if isinstance(candidate, str) and candidate.startswith(VIEW_PREFIX):
            return candidate
    return None


def pass_tap(event):
    """A finger on a tile (\u00a7 5.6). The value carries the id of its window."""
    value = event.get("value")
    oid = value.get("for") if isinstance(value, dict) else None
    if not isinstance(oid, str) or parse_object_id(oid)[0] is None:
        # An absorbed gesture has no receipt and no dead letter, so a payload
        # the screen cannot read would vanish without a trace -- and a client
        # defect with it.
        sys.stderr.write("%s: the value carries no object id\n" % TAP_EVENT)
        return []
    return pass_or_queue({"kind": "tap", "for": wrapper_of(oid)}, now_ms(), [])


def pass_hold(event):
    """The OS mark was held (\u00a7 5.4, \u00a7 5.6). The event carries NOTHING.

    The recording itself rides the voice channel and never passes through here
    (`frame({"type": "hold"})`, OS_CLIENT_JS); this is the second half the hook sends
    over the LiveView so the screen can open the window the turn belongs in. Which
    window that is, is the SCREEN's knowledge: the curator picks the one with
    `topic: chat` (\u00a7 8.5). A `topic` a client sends with the hold is not read (S-088).
    """
    return pass_or_queue({"kind": "hold"}, now_ms(), [])


def pass_event(body):
    """A semantic browser event, handed out of the hive with its addressee.

    An event whose id will not parse still leaves. A view the display holds and
    this cell cannot attribute is a defect somebody has to see; dropping the
    event would make it invisible, while a message with no `owner` dead-letters
    where a person can read it.

    `owner` and `view_id` leave on the HOP as well as in the body, always
    present and empty where the id would not parse: the member routes the event
    back to whoever owns the view, and an edge condition can only read
    `context.*` and `hop.*` (GH #459). An empty `owner` therefore fails every
    owner guard by construction and the event dead-letters, which is the
    behaviour this scope wanted in the first place.
    """
    event = body.get("event")
    if not isinstance(event, dict):
        return []
    name = str(event.get("name") or "")
    if name == TAP_EVENT:
        return pass_tap(event)
    if name == HOLD_EVENT:
        return pass_hold(event)
    out = {
        "messages": body.get("messages") or [text_turn(str(event.get("name") or ""))],
        "event": event,
    }
    owner, view_id = parse_object_id(event_object_id(event))
    if owner is not None:
        out["owner"] = owner
        out["view_id"] = view_id
    return [
        emission(
            "event",
            out,
            owner=owner or "",
            view_id=view_id or "",
        )
    ]


# ---------------------------------------------------------------------------
# The store's rows and the display's tree, as this cell reads them


def bundle_failed(body, hop):
    """The leg that failed, or None.

    Every leg is checked, leg 0 included: a select that could not run leaves
    this cell with no before-state, and computing an after-state out of nothing
    would silently blank the screen.
    """
    if hop.get("error_code"):
        return str(hop["error_code"])
    for entry in body.get("results") or []:
        if isinstance(entry, dict) and entry.get("error_code"):
            return "%s on %s" % (entry["error_code"], entry.get("operation") or "?")
    return None


def read_rows(body):
    """The rows of leg 0, or None when the reply does not carry any."""
    msgs = body.get("messages") or []
    if not msgs:
        return None
    try:
        doc = json.loads(str(msgs[0].get("text") or ""))
    except (TypeError, ValueError, AttributeError):
        return None
    if not isinstance(doc, list):
        return None
    return [r for r in doc if isinstance(r, dict)]


def read_objects(body):
    """What the display holds, or None when the answer was not a `query` one.

    `{id: {"props": …, "parent": …, "ord": …}}` -- the place an object sits is
    read back as well as its props, because the order of the screen lives in
    `ord` and an `object.update` cannot move anything.
    """
    msgs = body.get("messages") or []
    if not msgs:
        return None
    try:
        doc = json.loads(str(msgs[-1].get("text") or ""))
    except (TypeError, ValueError, AttributeError):
        return None
    if not isinstance(doc, dict) or not isinstance(doc.get("objects"), list):
        return None
    out = {}
    for obj in doc["objects"]:
        if isinstance(obj, dict) and obj.get("id"):
            props = obj.get("props")
            try:
                ord_ = int(obj.get("ord") or 0)
            except (TypeError, ValueError):
                ord_ = 0
            out[str(obj["id"])] = {
                "props": props if isinstance(props, dict) else {},
                "parent": obj.get("parent"),
                "ord": ord_,
                # The component too: a window the table no longer has is
                # laid back for one more frame (`ghosts`), and only a window
                # is -- a wrapper with nothing under it is not.
                "component": str(obj.get("component") or ""),
            }
    return out


def add_tree(want, parent, node, index, tiles=None, window="", attrs=None, region=""):
    """One component-tree node and everything under it, as objects.

    The id is the index chain in `children` order, which makes it a function of
    the tree alone: the same tree sent twice patches the same objects, and a
    node that moved is an update rather than a delete plus a create.

    Unless the node names its own `key`, and then the id is that instead of the
    index -- while `ord`, the drawing order, still comes from the index. The
    difference is what a `keep` prop is worth. An index is a SLOT: insert one
    sibling ahead of a node and every id behind it now belongs to a different
    thing, so the props the sender asked to keep are handed to the new occupant
    of the slot. Measured on a running colony under GH #544: a picture whose
    edge count had grown by three had 103 of 104 boxes standing at a position
    that had been computed for some OTHER cell, and three cells held two objects
    each. A key that says WHAT the node is -- a cell path, a row id -- makes the
    kept prop follow the thing, which is the only reading under which `keep`
    means anything at all.
    """
    key = node.get("key")
    oid = "%s/%s" % (parent, key if is_node_key(key) else index)
    props = dict(node.get("props") or {})
    component = str(node.get("component") or "")
    # The state is the curator's word. An application may say `urgent` or
    # `hidden` about a window; any other word is dropped before the curator
    # looks, so a `focus` an app claims never reaches the screen (GH #679).
    #
    # A PAGE is the one node with a state of its OWN, and the two lists meet
    # nowhere: `loading|ready|error|suspended` is what the cell is doing with
    # the page, not what the application wants of the window (contracts 4).
    # Measured on the twin (`g15.md`, second finding at the edge): the window
    # guard ran here on every node, so the word the application had mapped was
    # dropped one step before the markup and every page stood as
    # `data-page-state=""` -- the sheet's `error` rule had never once fired.
    # Still fail-closed, and for the same reason: a word outside the four has
    # no rule behind it, so wearing it would be a lie in the markup.
    words = PAGE_STATES if component == "display-browser" else STATE_WORDS
    if props.get("state") not in words:
        props.pop("state", None)
    # `age` belongs to the channel: it is how the screen tells a window that
    # arrived from one that is on its way out, and an application's word for
    # it would fly a window in twice.
    props.pop("age", None)
    # The ladder is resolved at the door, once, on every window (R-23-2,
    # OR-F19): the sheet reads `data-layer` and a half-empty attribute is a
    # rule that never fires.
    if component in WINDOWS:
        # The curator's values, systemwide (§ 3.1): the same numbers on every output.
        # `layer` is resolved here once so the sheet always reads one of the two words.
        props["layer"] = layer_of(props)
        props["region"] = region
        props.update(attrs or {})
        # Taken, once. The values belong to the ONE window of a view (§ 7.1), which is
        # the one `unwrap_window` reads the hints off -- the first in the tree. They are
        # handed DOWN past everything that is not a window (below), because an app may
        # wrap its window in a `display-stack` and `unwrap_window` says so.
        attrs = None
        window = oid
    # The screen names the window a typed line belongs to (contract § 8): the
    # application cannot, because the id is the index chain this walk mints,
    # and a field that named the wrong object would send a person's sentence
    # to somebody else's application.
    # Written even when nothing encloses the field: an empty `for` makes the
    # event nameless and it dead-letters where a person reads it, while a value
    # the application invented would send the sentence to a foreign view.
    if component == "display-input":
        props["for"] = window
    # And the screen names the cell a page is streamed from (§ 7.9), out of
    # `params.browser_mount` -- the application cannot know that name either.
    # Written on EVERY pass and not only into an empty slot: `object.update`
    # merges per key, so a mount an application once invented would stand on
    # that object for ever and the page would go on asking a cell that is not
    # there.
    if component == "display-browser":
        props["mount"] = BROWSER_MOUNT
    want[oid] = {
        "component": component,
        "parent": parent,
        "ord": index * ORD_STEP,
        "props": props,
        "keep": [k for k in (node.get("keep") or []) if isinstance(k, str)],
    }
    kids = [k for k in (node.get("children") or []) if isinstance(k, dict)]
    # The tile is TAKEN OUT (R-D1): a window's child keyed `tile` is what the
    # application says about itself in one line, and it belongs in the dock,
    # not a second time inside the big window. Exactly one per window; a
    # second one keeps its place in the tree and is drawn like any child.
    if tiles is not None and component in WINDOWS:
        rest = []
        for kid in kids:
            if (oid not in tiles and kid.get("key") == TILE_KEY
                    and str(kid.get("component") or "") == "display-tile"):
                tiles[oid] = dict(kid.get("props") or {})
            else:
                rest.append(kid)
        kids = rest
    # The curator's values travel DOWN until a window takes them (above). Handing the
    # children `None` was the same statement for a window at the root of a view and a
    # falsehood for one inside a wrapper: `unwrap_window` lets an app put its window in a
    # `display-stack`, the pass reads the hints through it, and the RENDER then drew the
    # window with nothing but the app's own props. Measured on e25, whose `chat@0.3.0`
    # wraps its pane: `<section id="chat" data-rung="" data-level="" data-pinned="true">`
    # beside its own tile from the same pass, `data-rung="ambient" data-pinned="1"`. No
    # level rule of § 7d reaches such a window -- not even the one that hides level 0 --
    # so a conversation the curator had put away stood open and 4948 px tall on a 852 px
    # phone (B-09), every proof that asks about levels saw a stage with none (B-20), and
    # the pass answered every tap by wiping the level the client had just drawn (B-06).
    for j, kid in enumerate(kids):
        add_tree(want, oid, kid, j, tiles, window, attrs, region)
        # And once only. § 7.1: a view is ONE window, and the one the pass computed a
        # rung for is the one `unwrap_window` reads -- the first in the tree. A second
        # window beside it is not this view's, and wearing the first one's level would
        # draw it at a depth nobody decided.
        if attrs is not None and unwrap_window(kid):
            attrs = None


def drawable(views):
    """The views that can be drawn at all, each with its wrapper id.

    A row the screen cannot address -- no owner, an id that is not one, a
    region this version does not have, content that will not parse -- is
    skipped rather than refused. It was refused at the door; a row that got
    past that is a defect somebody has to be able to see the REST of the
    screen through.
    """
    out = []
    for view in views:
        region = str(view.get("region") or REGIONS[0])
        if region not in REGIONS:
            continue
        owner = str(view.get("owner") or "")
        view_id = str(view.get("view_id") or "")
        if not owner or not is_view_id(view_id):
            continue
        content = view.get("content")
        if isinstance(content, str):
            try:
                content = json.loads(content)
            except (TypeError, ValueError):
                continue
        if not isinstance(content, dict):
            continue
        wrapper = "%s%s.%s" % (VIEW_PREFIX, owner.replace("/", "~"), view_id)
        out.append((region, owner, view_id, wrapper, content, view))
    return out


def as_unit(value, default):
    """A number clamped to 0..1, or the default when it is not a number."""
    try:
        return min(1.0, max(0.0, float(value)))
    except (TypeError, ValueError):
        return default


# ---------------------------------------------------------------------------
# The memory of the cell (GH #809)
#
# `resident` runs this module again for EVERY message into one globals dict, so every
# assignment on module level is made again each time -- the memory must not be one. It
# lives under one name that nothing on module level assigns (`_RAM`), reached only through
# `ram()`, and a version guard: a dict of another shape is dropped rather than read.
#
# RAM is a cache (docs/cell-types.md § code): what it holds is rebuilt from the store's
# rows, the rest row and the tree `web` holds whenever the child is new -- after a start,
# a kill, a timeout, a replaced node. That rebuild is the boot below, and it costs ONE
# select and ONE read.

RAM_V = 1
# How long a repair read may stay unanswered before the next event asks again. Its answer
# comes back in milliseconds (a 540 kB tree read in 8 ms, wave Display integration pass);
# one that has not come after five seconds died on the way -- its chain ran out of ttl or
# the child was killed under it -- and without a second question the cell would wait in
# `repair` for good and draw nothing ever again.
REPAIR_RETRY_MS = 5000


def ram():
    """The cell's memory across messages. `resident` re-executes this module per message into
    ONE globals dict (crates/meclaw-cells/src/code/harness.py), so the dict survives only when
    nothing on module level re-assigns it. Killed child = fresh dict = phase "cold".

    `phase`: cold | booting-rows | booting-tree | live | repair. `pending`: the events that
    arrived before the boot finished, `(at, event)` in order. `state`: the curator's state.
    `rows`: `{oid: row}`, exactly the store rows of the applications. `have`: the tree this
    cell last sent (`read_objects` form), None while unknown. `rest`: the rest row's content
    as last written. `said`: what the pass before said out loud. `dials`: the knobs the state
    was last given. `define`: application vocabulary not yet sent to the display. `seen`: the
    windows whose queued writes left before the running boot select (a retry after a failed
    one), so the select already reads them. `due`: the clock's order this cell last gave,
    None until it gave one. `asked`: when the repair read left (REPAIR_RETRY_MS).
    `repaired`: the patch a repair drew is still unanswered -- its refusal is not a mirror
    fault, and one read cannot cure it.
    """
    held = globals().get("_RAM")
    if isinstance(held, dict) and held.get("v") == RAM_V:
        return held
    held = {"v": RAM_V, "phase": "cold", "pending": [], "state": None, "rows": {},
            "have": None, "rest": None, "said": [], "dials": None, "define": [], "seen": [], "due": None,
            "asked": None, "repaired": False}
    globals()["_RAM"] = held
    return held


def select_rows(mark):
    """ONE bundle that reads the whole table: the boot, and nothing else asks for it."""
    legs = [tool_call({"operation": "select", "table": TABLE, "columns": COLUMNS}, "d-select")]
    return emission("views", {"messages": legs}, display_request=json.dumps(mark, sort_keys=True))


def read_tree(mark):
    """ONE query of the display's tree: at the boot and after a refused patch (OR-D7)."""
    return emission("read", {"messages": [tool_call({"op": "query", "route": PAGE_ROUTE},
                                                    "d-query")]},
                    display_request=json.dumps(mark, sort_keys=True))


def pass_or_queue(event, now, ops, define=None):
    """The ONE event of § 4.1 reached the cell: run its pass, or keep it for the boot.

    `ops` is what has to leave in this turn anyway (a write's store bundle). A cold cell
    has no rows and no tree, so the first event queues and starts the boot (§ 2.2); while
    it boots, events queue behind it. A live cell -- or one repairing its mirror -- runs
    the pass now and renders; a pass the model does not run (§ 4.1: nothing else triggers
    one) renders nothing.
    """
    r = ram()
    r["define"] += list(define or [])
    if r["phase"] == "cold":
        # The select goes out BEFORE this write's bundle: the store serves one sender in
        # order, so it reads the rows as they stood before the restart, and the queued
        # write is then a real pass over the real prior state. Were it after the write, the
        # boot would read the NEW row and lose what the curator remembered of that window
        # (since, dismissed_at, verdict, kept hints): measured 0/112 equal screens for a
        # killed cell woken by the same write, 58/112 with a new `touched` (REBUILD-WRITE).
        # Writes queued before a select that failed are in the store already; only those
        # the boot leaves out of the replay (`seen`).
        r["seen"] = sorted(set(str(ev.get("oid") or "") for _, ev in r["pending"]
                               if ev.get("kind") in ("app_write", "app_withdraw")))
        r["pending"].append((now, event))
        r["phase"] = "booting-rows"
        return [select_rows({"boot": "rows"})] + ops
    if r["phase"] in ("booting-rows", "booting-tree"):
        r["pending"].append((now, event))
        return ops
    if r["phase"] == "repair" and now - int(r.get("asked") or 0) >= REPAIR_RETRY_MS:
        r["asked"] = now
        ops = ops + [read_tree({"repair": True})]
    said = advance(event, now)
    if said is None:
        return ops
    return with_rest(ops + said + render(now, str(event.get("struck") or "")), now)


def advance(event, now):
    """One pass of § 4 over the state in memory. None when the model did not run it.

    What the pass says out loud (refusals, errors) and the question to the judge leave
    per pass: a boot runs several passes in one turn, and each one is its own event.
    The dials are the member's params and come with every message; when they differ from
    the ones the state was given, the RAW values face the door of § 4.7 again, which
    normalises them in the next pass and says once what it would not take.
    """
    r = ram()
    dials = [copy.deepcopy(KNOB_SETTINGS), copy.deepcopy(KNOB_SCREENS)]
    if r["dials"] != dials:
        if r["dials"] is not None:
            settings = dict(DEFAULT_SETTINGS)
            settings.update(KNOB_SETTINGS)
            r["state"]["settings"] = settings
            r["state"]["screens"] = copy.deepcopy(KNOB_SCREENS)
        r["dials"] = dials
    state = run_pass(r["state"], event, now)             # § 4, verbatim
    r["state"] = state
    if not state["pass"].get("runs"):
        return None
    spoken = spoken_of(state)
    out = refusals_of(state, spoken, r["said"])
    r["said"] = spoken
    if state["judge"]["called"]:
        out += judge_ops_from_state(state, now)
    return out


def sorted_rows(rows):
    """The app rows in one deterministic order. What a person SEES is the pass's word
    (`canvas_order`, `dock_order`), so this order decides nothing on the screen."""
    return sorted(rows.values(), key=lambda x: (
        REGION_INDEX.get(str(x.get("region") or REGIONS[0]), 0),
        str(x.get("owner") or ""), str(x.get("view_id") or "")))


def draw(r, now, struck):
    """The tree the state in memory says the screen is: `(want, pages, due ops)`.

    The order the clock holds is the one this cell last gave (`due`), not the one the
    mirror names: a refused patch leaves the display on an older order while the clock
    already took the newer one, and during a repair there is no mirror at all. Read off
    the mirror, the repair took an order back twice and left the refused pass's order
    standing -- a stroke for nothing (REPAIR "the clock through a repair"). Only a fresh
    cell, which gave no order yet, reads it off the tree.
    """
    state = r["state"]
    have = r["have"] if r["have"] is not None else {}
    default = str(state["settings"].get("default_screen") or "")
    want = objects_from_state(state, sorted_rows(r["rows"]), now, default, have)
    clock = have if r.get("due") is None else {ROOT_ID: {"props": {"due": r["due"]}}}
    ops = due_ops_from_strokes(state, want, clock, now, struck)
    r["due"] = str(want[ROOT_ID]["props"].get("due") or "")
    pages = mirror_screens(state, want, now)
    return want, pages, ops


def row_components(rows):
    """The vocabulary of every app row (OR-D8): what a fresh display has to learn."""
    out = []
    for row in sorted_rows(rows):
        try:
            parsed = json.loads(str(row.get("components") or "[]"))
        except (TypeError, ValueError):
            continue
        out += [c for c in parsed if isinstance(c, dict)] if isinstance(parsed, list) else []
    return out


def snapshot(want):
    """What the display holds once a patch of `want` has landed, in `read_objects` form.

    The mirror the next pass diffs against: a pass whose drawing did not change sends
    nothing, and a pass that changed one window sends that window (GH #412).
    """
    return {oid: {"props": copy.deepcopy(spec["props"]), "parent": spec["parent"],
                  "ord": spec["ord"], "component": spec["component"]}
            for oid, spec in want.items()}


def render(now, struck="", bootstrap=False, everything=False):
    """ONE patch of what changed since the last one, and the timer's order.

    Only a live cell draws: while the mirror is being repaired (OR-D7) the passes run on
    and the patch waits for the tree. `define` is the app vocabulary still owed to the
    display -- all of it on a bootstrap and after a repair (OR-D8), otherwise what changed.
    """
    r = ram()
    have = r["have"] if r["have"] is not None else {}
    want, pages, ops = draw(r, now, struck)
    define = row_components(r["rows"]) if (bootstrap or everything) else list(r["define"])
    calls = patches(want, have, define, bootstrap) + page_ops(want, have, pages, bootstrap)
    if calls and r["phase"] == "live":
        ops.append(emission("patch", {"messages": [tool_call(c, "d-%d" % i)
                                                   for i, c in enumerate(calls)]}))
        r["have"] = snapshot(want)
        r["define"] = []
    return ops


# ---------------------------------------------------------------------------
# The rest row (OR-D3, OR-D4, OR-D22)
#
# The store holds what the apps said; the curator's own memory of them is what cannot be
# read back out of those rows: when a window was touched, put away, led; what the judge
# said and when it was last asked; what the screen already said out loud. That is ONE
# small row beside the app rows, written only when its content changes. Everything else
# of the state -- presence, rungs, levels, the dock, `unseen`, the chat's last turn --
# every pass computes anew.

STATE_OWNER = "display"
# The row of display@2.5.0-2.6.x. Not written any more and never deleted: a boot ignores
# it (`is_state_row`), and a fresh store never has one.
STATE_VIEW_ID = "screen-state"
STATE_KIND = "state"
REST_OWNER = STATE_OWNER
REST_VIEW_ID = "screen-rest"
REST_CURATOR = ("since", "dismissed_at", "led_until", "verdict_cleared", "topic_dupe")


def is_state_row(row):
    """Whether a store row is the state row rather than an app's view."""
    return (isinstance(row, dict)
            and str(row.get("owner") or "") == STATE_OWNER
            and str(row.get("view_id") or "") == STATE_VIEW_ID)


def is_rest_row(row):
    return (isinstance(row, dict)
            and str(row.get("owner") or "") == REST_OWNER
            and str(row.get("view_id") or "") == REST_VIEW_ID)


# What a view of the model holds beside its hints: the curator's own values and the
# bookkeeping of the door. Everything else is a hint an application wrote.
NOT_HINTS = ("children", "curator", "verdict", "owner", "ttl_ms", "written_at", "withdrawn")


def kept_of(view, row):
    """The hints the model still holds that the view's last row no longer says.

    § 4.6 merges a write per key (Decision 16.09. 5: a key left out stands), while the
    store holds the last write whole. A write that leaves a hint out therefore leaves the
    model with more than any row says, and a rebuild out of the rows alone would lose it
    -- measured: 44 of 114 scenarios rebuilt a different screen without it (OR-D.D1.4).
    An application that always sends its whole view keeps this empty.
    """
    said = hints_of_row(row)
    kept = {k: v for k, v in view.items() if k not in NOT_HINTS and k not in said}
    kids = view.get("children") or {}
    gone = {k: v for k, v in kids.items() if k not in (said.get("children") or {})}
    return kept, gone


def rest_content(state, said, rows=None):
    """The rest row's content: the curator memory no app row carries.

    Two cuts against the plan's first form, both so that a stroke alone writes nothing
    (#809 acceptance: the row is written on app writes, and only when it changes):
    `judge.called` is left out (OR-D22) -- `step1_judge_call` sets it anew in every pass,
    True only in the pass that asks, so the first stroke after every question would
    rewrite the row -- and `said` keeps only the ERRORS, the words § 4.7 repeats in every
    pass. A refusal is said once, in the pass that replaces the value, so it would flip
    the list on the very next stroke; after a restart the raw dials face the door again
    and say it once more, which is the same thing a changed dial does (OR-D.D1.3).
    """
    views = {}
    for oid, v in state["views"].items():
        c = v["curator"]
        views[oid] = {"since": c.get("since"), "dismissed_at": c.get("dismissed_at") or 0,
                      "led_until": c.get("led_until") or 0,
                      "verdict_cleared": bool(c.get("verdict_cleared")),
                      "topic_dupe": bool(c.get("topic_dupe")),
                      "verdict": v.get("verdict"), "withdrawn": bool(v.get("withdrawn"))}
        row = (rows or {}).get(oid)
        if row is not None:
            kept, kids = kept_of(v, row)
            if kept:
                views[oid]["kept"] = kept
            if kids:
                views[oid]["kept_children"] = kids
    judge = {k: v for k, v in state["judge"].items() if k != "called"}
    errors = [list(row) for row in said or [] if row and row[0] == "error"]
    return json.dumps({"v": 1, "judge": judge, "bar": state["bar"],
                       "weights": state["weights"], "said": errors, "views": views},
                      sort_keys=True)


def restore_rest(state, text):
    """Put the rest row's memory back into a state rebuilt from the app rows.

    A window the rows did not bring back is not made up here: the rows are the truth
    about what exists, the rest row only about what the curator knew of it.
    """
    if not text:
        return
    try:
        doc = json.loads(text)
    except (TypeError, ValueError):
        doc = None
    if not isinstance(doc, dict):
        sys.stderr.write("compose: the rest row is unreadable and was ignored\n")
        return
    for oid, held in (doc.get("views") or {}).items():
        view = state["views"].get(oid)
        if view is None or not isinstance(held, dict):
            continue
        for key in REST_CURATOR:
            if key in held:
                view["curator"][key] = held[key]
        if isinstance(held.get("verdict"), dict):
            view["verdict"] = copy.deepcopy(held["verdict"])
        if "withdrawn" in held:
            view["withdrawn"] = bool(held["withdrawn"])
        for key, value in (held.get("kept") or {}).items():
            if key not in NOT_HINTS:
                view[key] = copy.deepcopy(value)
        if held.get("kept_children"):
            view["children"] = dict(view.get("children") or {})
            view["children"].update(copy.deepcopy(held["kept_children"]))
    if isinstance(doc.get("judge"), dict):
        judge = copy.deepcopy(doc["judge"])
        judge["called"] = False
        state["judge"] = judge
    if "bar" in doc:
        state["bar"] = doc["bar"]
    if isinstance(doc.get("weights"), dict):
        state["weights"] = copy.deepcopy(doc["weights"])
    said = doc.get("said")
    ram()["said"] = [list(row) for row in said] if isinstance(said, list) else []


def rest_legs(content, now):
    row = {"owner": REST_OWNER, "view_id": REST_VIEW_ID, "region": REGIONS[0], "ord": 0,
           "kind": STATE_KIND, "content": content, "components": "[]", "ttl_ms": 0,
           "updated_at": now}
    return [tool_call({"operation": "delete", "table": TABLE,
                       "where": {"owner": REST_OWNER, "view_id": REST_VIEW_ID}}, "r-delete"),
            tool_call({"operation": "insert", "table": TABLE, "row": row}, "r-insert")]


def with_rest(ops, now):
    """`ops`, plus the rest row when its content changed (OR-D4).

    Two more legs IN the store bundle of the app write that caused them -- one bundle, one
    round trip -- or, when this turn wrote nothing (a tap, a verdict, a stroke), a bundle
    of their own.
    """
    r = ram()
    if r["state"] is None:
        return ops
    content = rest_content(r["state"], r["said"], r["rows"])
    if content == r["rest"]:
        return ops
    r["rest"] = content
    legs = rest_legs(content, now)
    for em in ops:
        head = em.get("header") or {}
        if head.get("route") != "views":
            continue
        try:
            mark = json.loads(str(head.get("display_request") or ""))
        except (TypeError, ValueError):
            continue
        if isinstance(mark, dict) and "write" in mark:
            em["messages"] = list(em.get("messages") or []) + legs
            mark["rest"] = True
            head["display_request"] = json.dumps(mark, sort_keys=True)
            return ops
    return ops + [emission("views", {"messages": legs},
                           display_request=json.dumps({"rest": True}, sort_keys=True))]


def spoken_of(state):
    """Every refusal and every error of this pass, as comparable rows."""
    return ([["refused"] + [str(x) for x in entry]
             for entry in state["pass"].get("refused") or []]
            + [["error"] + [str(x) for x in entry]
               for entry in state["pass"].get("errors") or []])


# ---------------------------------------------------------------------------
# The replies: the store, the display


def failed_legs(body, hop):
    """`[(tool_call_id, why)]` of every refused leg; the id is "" for a whole refusal."""
    if hop.get("error_code"):
        return [("", str(hop["error_code"]))]
    return [(str(e.get("tool_call_id") or ""),
             "%s on %s" % (e["error_code"], e.get("operation") or "?"))
            for e in body.get("results") or []
            if isinstance(e, dict) and e.get("error_code")]


def pass_views(body, ctx, hop):
    """The store answered: the boot's rows, or the acknowledgement of a write."""
    try:
        request = json.loads(str(ctx.get("display_request") or ""))
    except (TypeError, ValueError):
        request = None
    if not isinstance(request, dict):
        return []
    if request.get("boot") == "rows":
        return boot_rows(body, hop)
    out = []
    failed = failed_legs(body, hop)
    write = request.get("write")
    if isinstance(write, dict):
        # The row is in memory already; a refused store write is a receipt to the app
        # and nothing else. After a restart that view is missing -- the store is the
        # truth, and the app has heard it (plan D1a § 2.3).
        mine = [why for tid, why in failed if not tid.startswith("r-")]
        if mine:
            out += refuse("store_failed", mine[0], str(write.get("view_id") or ""),
                          str(write.get("owner") or ""))
    if request.get("rest"):
        theirs = [why for tid, why in failed if tid.startswith("r-") or tid == ""]
        if theirs:
            sys.stderr.write("compose: the rest row was not written: %s\n" % theirs[0])
            ram()["rest"] = None            # the next change writes it again
    return out


def boot_rows(body, hop):
    """The boot's select answered: the rows, the rest row, and the state rebuilt from both.

    The state is rebuilt the way `reconcile` catches a state up with the store (OR-H0.9):
    every row replayed as the `app_write` it was, at the moment the store wrote it; then the
    rest row puts back what no row says. The select left before the queued writes, so every
    row it read is the state before the restart and is replayed; the queued passes run next
    over that state, and the rows in memory take the queued writes' word. Only a window
    whose queued write left BEFORE this select (a retry after a failed one, `seen`) is left
    out of the replay: the store already holds the queued row, and replaying it would make
    its own pass a repeat.
    """
    r = ram()
    if r["phase"] != "booting-rows":
        return []
    why = bundle_failed(body, hop)
    got = None if why else read_rows(body)
    if got is None:
        sys.stderr.write("compose: the boot select failed: %s\n" % (why or "no rows"))
        r["phase"] = "cold"                 # the next message asks again; pending stays
        return []
    now = now_ms()
    queued = set(str(ev.get("oid") or "") for _, ev in r["pending"]
                 if ev.get("kind") in ("app_write", "app_withdraw"))
    seen = set(r.get("seen") or [])
    rows, rest = {}, None
    for row in got:
        if is_rest_row(row):
            rest = str(row.get("content") or "")
            continue
        if is_state_row(row):
            continue
        oid = object_id(row.get("owner"), row.get("view_id"))
        if oid not in seen:
            rows[oid] = row
    state = empty_state(KNOB_SETTINGS, KNOB_SCREENS)
    state = reconcile(state, sorted_rows(rows), {"kind": "stroke"}, now)
    restore_rest(state, rest)
    for oid in queued:
        if oid in r["rows"]:
            rows[oid] = r["rows"][oid]
        else:
            rows.pop(oid, None)
    r["rows"] = rows
    r["seen"] = []
    r["state"] = state
    r["rest"] = rest
    r["dials"] = [copy.deepcopy(KNOB_SETTINGS), copy.deepcopy(KNOB_SCREENS)]
    r["phase"] = "booting-tree"
    return [read_tree({"boot": "tree"})]


def boot_tree(body):
    """The boot's read answered: the mirror, then every queued event, then ONE patch.

    First a stroke when the rows brought windows back, so presence, rungs and the dock
    stand as the clock says before any queued tap asks whether its window is open; it
    runs at the earliest queued moment so no pass runs earlier than the one before it
    (OR-D.D1.2). Then each queued event is its
    own pass at its own moment (§ 4.1: one event, one pass).
    """
    r = ram()
    have = read_objects(body)
    bootstrap = have is None or ROOT_ID not in have
    r["have"] = {} if bootstrap else have
    r["phase"] = "live"
    pending, r["pending"] = r["pending"], []
    now = now_ms()
    start = min([at for at, _ in pending] + [now])
    # Only a rebuilt state needs the stroke: with no row to bring back there is nothing
    # it could set, and the first queued event is then the first pass, which is where
    # § 4.7 says a refused dial is said (OR-D.D1.2).
    ops = (advance({"kind": "stroke"}, start) or []) if r["state"]["views"] else []
    last, struck = start, ""
    for at, event in pending:
        ops += advance(event, at) or []
        last = max(last, at)
        if event.get("kind") == "stroke" and event.get("struck"):
            struck = str(event["struck"])
    return with_rest(ops + render(last, struck, bootstrap), last)


def pass_read(body):
    """The display's tree: the boot's, or the repair's after a refused patch (OR-D7)."""
    r = ram()
    if r["phase"] == "booting-tree":
        return boot_tree(body)
    if r["phase"] != "repair":
        return []
    have = read_objects(body)
    bootstrap = have is None or ROOT_ID not in have
    r["have"] = {} if bootstrap else have
    r["phase"] = "live"
    now = now_ms()
    ops = render(now, "", bootstrap, everything=True)
    r["repaired"] = any((em.get("header") or {}).get("route") == "patch" for em in ops)
    return with_rest(ops, now)


def pass_patched(body, hop):
    """The display's acknowledgement: nothing -- unless it refused a leg (OR-D7).

    Then the mirror no longer says what the display holds, and a diff against it would be
    a guess: `have` is dropped and ONE read asks. Until it answers, passes run on and draw
    nothing; the answer draws the difference once.
    """
    why = bundle_failed(body, hop)
    if not why and (int_or_zero(hop.get("bundle_errors")) > 0):
        why = "%s legs refused" % hop.get("bundle_errors")
    r = ram()
    # The display answers patches in the order they left, and none leaves during a
    # repair: the first answer after the repair read is the answer to the patch that
    # repair drew.
    repaired, r["repaired"] = r.get("repaired"), False
    if not why:
        return []
    sys.stderr.write("compose: the display refused a patch: %s\n" % (why,))
    if r["phase"] != "live":
        return []
    if repaired:
        # Drawn against the tree the display itself just reported and refused all the
        # same: the leg is refused on every try (an app nesting glass in glass), not a
        # mirror that lies. Another read would draw the same leg again -- measured: six
        # repairs in one chain until its ttl ran out, the last read dead with it, the cell
        # in `repair` for good (`710_the_colony_holds_in_both_engines_browser`, 6/6 red).
        # The mirror keeps what was drawn; the refused leg stays refused.
        sys.stderr.write("compose: refused again after a repair; the leg stays refused\n")
        return []
    r["have"] = None
    r["phase"] = "repair"
    r["asked"] = now_ms()
    return [read_tree({"repair": True})]


def int_or_zero(value):
    try:
        return int(value)
    except (TypeError, ValueError):
        return 0


# ---------------------------------------------------------------------------
# Between the store row and the model's `view`: the door's own wire format
#
# The model knows a window as a flat dict of hints plus `children`; the store knows it
# as the component tree the app sent. These two functions are the translation, and they
# are inverses of each other -- `window_node(hints_of_row(row))` is the same tree again.

CHAT_COMPONENT = "display-chat"
CHAT_LINE_COMPONENT = "display-chat-line"
CHILD_COMPONENT = "display-value"
# The role a chat line wears for the sheet, per kind of line (§ 8.5).
LINE_ROLES = {"turn": "you", "answer": "companion"}


def object_id(owner, view_id):
    """The id of a window in the screen state: `view.<owner slug>.<view_id>` (§ 2 Id).

    A path segment inside an id would otherwise be indistinguishable from the child
    index chain, so a `/` in the owner is written `~` -- the same spelling `drawable`
    and `parse_object_id` use.
    """
    return "%s%s.%s" % (VIEW_PREFIX, str(owner or "").replace("/", "~"), view_id)


def wrapper_of(oid):
    """The window id out of any id under it: a tap names a child, the state names the window."""
    return str(oid or "").split("/")[0]


def unwrap_window(node):
    """The window node of a content tree: the first node whose component is a window.

    An app may wrap its window in a `display-stack`; the pass reads the window.
    """
    if not isinstance(node, dict):
        return {}
    if str(node.get("component") or "") in WINDOWS:
        return node
    for kid in node.get("children") or []:
        found = unwrap_window(kid)
        if found:
            return found
    return {}


def hints_of_row(row):
    """The model's `view` out of a store row: the window's own props plus `children`.

    `children` carries only what the pass may read (§ 4.8: a child change is no touch):
    the `tile`, the chat's lines for the judge (§ 4.4), and every other keyed child as
    its text -- enough for the merge of § 4.6 and for nothing else.

    A `prose` row is FLAT: its content IS the hints (`notice_row`, `validate`), so there
    is no window node to unwrap. Reading it like a component tree handed the pass an
    empty view, and a `system_error` notice scored like a silent one.
    """
    try:
        root = json.loads(str(row.get("content") or "{}"))
    except (TypeError, ValueError):
        root = {}
    if not isinstance(root, dict):
        root = {}
    if str(row.get("kind") or "") == "prose":
        props = dict(root)
        props["children"] = {}
        props["ttl_ms"] = row.get("ttl_ms")
        return props
    win = unwrap_window(root) or root
    props = dict(win.get("props") or {})
    kids = {}
    for child in win.get("children") or []:
        if not isinstance(child, dict):
            continue
        component = str(child.get("component") or "")
        key = child.get("key")
        if key == TILE_KEY:
            kids["tile"] = dict(child.get("props") or {})
        elif component == CHAT_COMPONENT:
            kids["lines"] = [
                {"kind": "turn"
                 if (c.get("props") or {}).get("role") == LINE_ROLES["turn"] else "answer",
                 "text": (c.get("props") or {}).get("text"),
                 "channel": (c.get("props") or {}).get("channel")}
                for c in child.get("children") or [] if isinstance(c, dict)]
        elif isinstance(key, str) and key:
            kids[key] = (child.get("props") or {}).get("text")
    props["children"] = kids
    # `ttl_ms` stands at the VIEW, beside the window (§ 3.3), and the model reads it off
    # the same dict as the hints.
    props["ttl_ms"] = row.get("ttl_ms")
    return props


def window_node(props, children=None):
    """A store row's content out of the model's `view`: the inverse of `hints_of_row`."""
    props = dict(props or {})
    props.pop("children", None)
    props.pop("ttl_ms", None)
    kids = []
    children = children if isinstance(children, dict) else {}
    if isinstance(children.get("tile"), dict):
        kids.append({"component": "display-tile", "key": TILE_KEY,
                     "props": dict(children["tile"])})
    if isinstance(children.get("lines"), list):
        kids.append({"component": CHAT_COMPONENT, "props": {}, "children": [
            {"component": CHAT_LINE_COMPONENT,
             "props": {"role": LINE_ROLES.get(str(line.get("kind") or ""), "companion"),
                       "text": line.get("text") or "",
                       "channel": line.get("channel") or ""}}
            for line in children["lines"] if isinstance(line, dict)]})
    for key in sorted(children):
        if key in ("tile", "lines"):
            continue
        kids.append({"component": CHILD_COMPONENT, "key": key,
                     "props": {"text": children[key]}})
    return {"component": "display-pane", "props": props, "children": kids}


# ---------------------------------------------------------------------------
# The patch and the tree it builds, for the driver and the tests


def calls_of(em):
    """The `object.*` calls of a patch bundle. For the driver and the tests."""
    out = []
    for leg in em.get("messages") or []:
        try:
            call = json.loads(str(leg.get("text") or ""))
        except (TypeError, ValueError):
            continue
        if isinstance(call, dict):
            out.append(call)
    return out


def apply_call(have, call):
    """One call of a patch bundle on the tree a display holds. For the driver and the tests."""
    op = str(call.get("op") or "")
    oid = str(call.get("id") or "")
    if op == "object.create":
        have[oid] = {"id": oid, "parent": call.get("parent"), "ord": call.get("ord") or 0,
                     "component": call.get("component") or "",
                     "props": dict(call.get("props") or {})}
    elif op == "object.update" and oid in have:
        have[oid]["props"].update(call.get("props") or {})
        if call.get("parent") is not None:
            have[oid]["parent"] = call["parent"]
    elif op == "object.move" and oid in have:
        have[oid]["parent"] = call.get("parent")
        have[oid]["ord"] = call.get("ord") or 0
    elif op == "object.delete":
        have.pop(oid, None)


# ---------------------------------------------------------------------------
# What the curator says out loud about what it would not take (§ 4.6, § 4.7)

# Which refusal of the pass leaves on which `error_code`. A refusal of the door is a
# receipt and never a silent default: a member who sets a value the screen will not
# take has to be able to read that.
REFUSAL_CODES = {"settings": "setting_refused", "screen": "profile_refused"}
ERROR_CODES = {"settings": "setting_error", "screen": "profile_error"}


def refusals_of(state, spoken, before):
    """One receipt per refusal and per error THIS pass says for the first time.

    Two rules beyond the list itself:

      * An absorbed gesture is not a refusal to anybody. A tap on a window with no tile
        and a hold with no chat view are "absorbed and recorded" (§ 5.4, § 6.12, S-017,
        S-053): the pass writes them down so a scenario can read them, and the screen
        says nothing out loud -- a receipt there is noise on the lane of an app that did
        nothing wrong.
      * A value is said ONCE. Settings and profile dials replace themselves at the door
        and so speak once by themselves (§ 4.7); the two errors replace nothing and would
        otherwise repeat in every pass, so the pass before's list (`said`) silences them.
        § 4.7 stays true where it is written -- in the state (`errors()`), every pass.
    """
    out = []
    fresh = [row for row in spoken if row not in before]
    for row in fresh:
        kind, first, rest = row[0], row[1], row[2:]
        detail = " ".join(rest)
        if kind == "error":
            out += refuse(ERROR_CODES.get(first, "profile_error"), detail, "", "")
            continue
        if first in REFUSAL_CODES:
            out += refuse(REFUSAL_CODES[first], detail, "", "")
            continue
        if first == "hold" or (rest and rest[0] == "tap"):
            continue                       # absorbed, not refused (§ 5.4, S-053)
        if first == "verdict":
            out += refuse("view_refused", "verdict: %s" % (detail,), "", "")
            continue
        owner, view_id = parse_object_id(first)
        out += refuse("view_refused", detail, view_id or "", owner or "")
    return out


# ---------------------------------------------------------------------------
# The rendering: the object tree is a rendering of ONE state (§ 3.1, § 6)

# The names the sheet and the hooks read. One list, one place (OR-H3).
ATTRS = {
    "rung": "data-rung",        # hidden | ambient | relevant | focus | urgent   (§ 4.17)
    "level": "data-level",      # 0 | 1 | 2 | 3                                  (§ 4.24)
    "front": "data-front",      # "1" on the front urgent only                   (§ 4.18)
    "age": "data-age",          # fresh | settled | leaving                      (§ 4.35)
    "layer": "data-layer",      # canvas | modal                                 (§ 4.17)
    "led": "data-led",          # "1" while led_until > now                      (§ 4.19)
    # The moment the window's own state last MOVED under a touch or a put-away
    # (`max(since, dismissed_at)`, § 4.9/§ 5.2). The client compares against the value it
    # read when the finger landed and holds its optimistic drawing until a patch carries
    # a bigger one -- a patch of a pass that started before the tap renders the state
    # before it (GH #744, § 5.7 Decision 18.09.). A server stamp and not a browser clock:
    # the comparison is between two values of the SAME clock, so a skewed device cannot
    # release the hold early.
    "acted": "data-acted",      # epoch ms, the last touch or put-away of this window
    "open": "data-open",        # "1" on the tile of an open window              (§ 6.10)
    "pinned": "data-pinned",    # "1"                                            (§ 7.6)
    "unread": "data-unread",    # "1"                                            (§ 7.2)
    "seat": "data-seat",        # "1" on a seat tile; an empty seat is a `display-seat`
    "unseen": "data-unseen",    # the count, on the OS mark                      (§ 4.33)
    "exit": "data-exit",        # tv | monitor | phone, on the root              (§ 6.1)
    "inputs": "data-inputs",    # space-joined words, on the root                (§ 6.4)
    "dock": "data-dock",        # shown | hidden, on the columns                 (§ 6.1)
    "switch": "data-switch",    # "1" on the root of /<mount>/ only              (§ 6.5)
    "scale": "--scale",         # the token, on the root style                   (§ 6.6)
}
# The props a window wears for the sheet, out of the curator's values. Everything else
# on the object is the app's own (§ 3 hints, title, kicker).
WINDOW_ATTRS = ("rung", "level", "front", "age", "layer", "led", "pinned", "topic",
                "since", "score", "acted")


def window_attrs(view, now):
    """The rendering values of one window, systemwide (§ 3.1): the same on every output."""
    c = view["curator"]
    return {"rung": str(c["rung"] or ""),
            "level": str(c["level"]),
            "front": "1" if c.get("front") else "",
            "age": str(c["age"] or ""),
            "layer": layer_of(view),
            "led": "1" if (c.get("led_until") or 0) > now else "",
            "pinned": "1" if view.get("pinned") is True else "",
            "topic": str(view.get("topic") or ""),
            "since": str(c.get("since") or ""),
            "score": str(c.get("score") or 0),
            # `max(since, dismissed_at)` and nothing else: both are written by the pass
            # that acted, so the value only moves when this window's own state did. It
            # therefore survives the dedup of GH #412 -- a pass that changed nothing
            # about this window sends no update for it.
            "acted": str(max(int(c.get("since") or 0), int(c.get("dismissed_at") or 0)))}


def profile_broken(state, name):
    """Whether this profile never passed the door: `display_type` is mandatory (§ 4.7).

    Such a profile stays RAW and is reported in every pass; it carries no normalised
    `inputs` and no `dock_max`, so nothing that reads one may be asked about it.
    """
    return "error" in ((state.get("screens") or {}).get(name) or {})


def profile_of(state, name):
    """This output's profile after the door (§ 4.7), plus the scale of § 6.6."""
    prof = dict((state.get("screens") or {}).get(name) or {})
    kind = str(prof.get("display_type") or "")
    if kind not in SCREEN_BASE:
        kind = "tv"
    try:
        distance = float(prof.get("viewing_distance_m"))
    except (TypeError, ValueError):
        distance = SCREEN_REFERENCE[kind]
    if distance <= 0:
        distance = SCREEN_REFERENCE[kind]
    scale = SCREEN_BASE[kind] * (distance / SCREEN_REFERENCE[kind])
    # A profile that never passed the door keeps its raw words in the state (§ 4.7), but
    # nothing is BOUND on it: the kind is unknown, so the screen promises no finger and no
    # keyboard here. Half a television -- the tv fallback with the raw `inputs` -- would be
    # exactly the silent default § 4.7 forbids.
    said = prof.get("inputs") if isinstance(prof.get("inputs"), list) else []
    words = [] if "error" in prof else [w for w in said if w in INPUT_WORDS]
    return {"type": kind,
            "scale": scale_text(min(SCALE_MAX, max(SCALE_MIN, scale))),
            "inputs": " ".join(words),
            "dock": prof.get("dock_default") or PROFILE_DEFAULTS[kind][0],
            "dock_max": prof.get("dock_max") or PROFILE_DEFAULTS[kind][1]}


def root_objects(state, now, name):
    """The structure every output has: shell, regions, the OS mark, the dock."""
    prof = profile_of(state, name)
    want = {
        ROOT_ID: {
            "component": "display-shell",
            "parent": None,
            "ord": 0,
            "props": {"stylesheet": True, "faces": faces(FONT_BASE), "vocab": VOCAB,
                      "ground": GROUND, "client_js": SCENE_CLIENT_JS,
                      # § 6.5: the switch reads the outputs by NAME and which of them
                      # is the default; the sheet reads the TYPE (§ 6.3, § 6.6). So the
                      # root wears both, under two names.
                      "screens": json.dumps(sorted(state.get("screens") or {})),
                      "default_screen": str(state["settings"].get("default_screen") or ""),
                      "exit": prof["type"], "screen_name": name,
                      "inputs": prof["inputs"],
                      "scale": prof["scale"], "dock": prof["dock"],
                      "dock_max": prof["dock_max"], "switch": "",
                      "tap": False, "input_line": False, "due": ""},
            "keep": [],
        }
    }
    for i, region in enumerate(REGIONS):
        want[REGION_PREFIX + region] = {
            "component": "display-region", "parent": ROOT_ID, "ord": i * ORD_STEP,
            "props": {"region": region}, "keep": [],
        }
    want[DOCK_ID] = {
        "component": "display-dock", "parent": ROOT_ID, "ord": len(REGIONS) * ORD_STEP,
        "props": {"count": 0}, "keep": [],
    }
    want[OS_ID] = {
        "component": "display-os", "parent": ROOT_ID,
        "ord": (len(REGIONS) + 1) * ORD_STEP,
        "props": {"mount": VOICE_MOUNT, "client_js": OS_CLIENT_JS,
                  "unseen": str(state.get("unseen") or 0)},
        "keep": [],
    }
    return want


def ghost(want, have, oid, attrs):
    """Lay a leaving window back from what the display is already holding.

    § 4.35 gives a window one last pass so the sheet can fade it out, and a withdrawal is
    one of the three ways to leave -- but a withdrawal takes the ROW with it, and the row
    is where the content lives. Without this, every withdrawn window vanished between two
    renders instead of leaving: the state said `leaving`, and there was nothing to draw it
    from. So the objects the display holds stand in for the row for exactly that one pass;
    the next pass has no such view in the state any more and `patches` sweeps them.
    """
    for held_id, held in (have or {}).items():
        if held_id != oid and not held_id.startswith(oid + "/"):
            continue
        spec = {"component": str(held.get("component") or ""),
                "parent": held.get("parent"),
                "ord": held.get("ord") or 0,
                "props": dict(held.get("props") or {}),
                "keep": []}
        if spec["component"] in WINDOWS:
            spec["props"].update(attrs)
        want[held_id] = spec


def objects_from_state(state, rows, now, name, have=None):
    """Every object the screen holds, keyed by id -- a rendering of ONE state (§ 3.1).

    Windows: every view the pass computes (present, or in its leaving pass), with the
    curator's values as attributes; tiles: `state["dock_order"]` bottom -> top, an empty
    seat as a `display-seat`; the OS mark with `unseen`. `name` is the output this first
    tree is drawn for -- the switch's, which draws the `default_screen` (§ 6.5) -- and it
    reaches nothing but the root's own profile words: every per-output DECISION, the dock
    cut of § 4.30 included, is `apply_exit`, once per copy.
    """
    want = root_objects(state, now, name)
    tiles = {}            # window id -> the props of the `tile` child the app handed in
    faces = {}            # window id -> the DOM id its tile points at (§ 7.2)
    by_oid = {}
    for row in rows:
        if is_state_row(row):
            continue
        for region, owner, view_id, wrapper, content, view in drawable([row]):
            by_oid[wrapper] = (region, owner, view_id, content, view)
    # Every window that is not open sits at the same `ord`: it is a tile, and its
    # wrapper is not drawn (§ 6.3). Numbering them would move every window of a region
    # whenever one arrives whose id sorts ahead -- an `object.move` per window for a
    # difference nobody sees. What IS ordered stands in the band below zero, further down.
    for oid in sorted(state["views"]):
        view = state["views"][oid]
        if not in_state(view):
            continue
        if oid not in by_oid:
            # A window the pass still computes whose row is gone: its leaving pass
            # (§ 4.35). The display's own objects stand in for the content it no longer
            # has -- see `ghost`.
            ghost(want, have, oid, window_attrs(view, now))
            continue
        region, owner, view_id, content, row = by_oid[oid]
        parent = REGION_PREFIX + region
        attrs = window_attrs(view, now)
        if str(row.get("kind") or "") == "prose":
            want[oid] = {
                "component": "display-view-prose", "parent": parent, "ord": 0,
                "props": dict({"view_id": view_id, "owner": owner,
                               "title": str(content.get("title") or ""),
                               "body": str(content.get("body") or ""),
                               "region": region}, **attrs),
                "keep": [],
            }
            continue
        want[oid] = {
            "component": "display-view-custom", "parent": parent, "ord": 0,
            "props": {"view_id": view_id, "owner": owner}, "keep": [],
        }
        # The tile comes out of the tree under the WINDOW's id, not under the node id
        # `add_tree` mints: the dock is keyed by window, and the two were two maps.
        mine = {}
        add_tree(want, oid, content, 0, mine, attrs=attrs, region=region)
        if mine:
            tiles[oid] = list(mine.values())[0]
        # `data-for` is the DOM id of the window element, which is its `pane_id`: that is
        # what the client matches the two halves of one object on. A prose window has no
        # `pane_id` and no id in the DOM; its tile names the object instead.
        win = unwrap_window(content)
        faces[oid] = str((win.get("props") or {}).get("pane_id") or "") or oid
    # § 6.3: the open canvas windows stand in the canvas in `canvas_order`, the leading
    # one first. A band below zero, so a window that is drawn at all stands ahead of
    # everything that is only a tile.
    lane = canvas_order(state)
    for i, oid in enumerate(lane):
        if oid in want:
            want[oid]["ord"] = -(len(lane) - i) * ORD_STEP
    # The dock, bottom -> top. `ord` runs the other way: the column is anchored at the
    # bottom edge, so what stands lowest needs the highest `ord` (OR-F29).
    entries = state["dock_order"]
    n = len(entries)
    for i, entry in enumerate(entries):
        ord_ = (n - 1 - i) * ORD_STEP
        oid = entry["oid"]
        if entry["empty"]:
            # § 4.29: an empty seat is empty SPACE, not a placeholder -- an object with
            # no content, so nothing slides into the gap and nothing is drawn in it.
            want[tile_id(oid) + "~seat"] = {
                "component": "display-seat", "parent": DOCK_ID, "ord": ord_,
                "props": {"seat_ord": str(entry["seat_ord"] or 0)}, "keep": [],
            }
            continue
        view = state["views"].get(oid)
        if view is None:
            continue
        _tile(want, oid, view, ord_, entry, tiles.get(oid) or {}, faces.get(oid) or oid)
    want[DOCK_ID]["props"]["count"] = len([e for e in entries if not e["empty"]])
    return want


def mirror_screens(state, want, now):
    """One copy of the whole tree per output, and the switch at `/<mount>/`.

    The curator ran once; an output is the same objects under a prefix, rendered by its
    own profile (§ 6). The unprefixed tree is the SWITCH: it draws the `default_screen`
    and carries `data-switch="1"`, and the client leads from there to the output that
    matches (§ 6.5).
    """
    names = sorted(state.get("screens") or {})
    default = str(state["settings"].get("default_screen") or "")
    pages = [{"route": PAGE_ROUTE, "root": ROOT_ID, "title": PAGE_TITLE}]
    originals = sorted(want)
    for name in names:
        for oid in originals:
            spec = want[oid]
            want[name + "." + oid] = {
                "component": spec["component"],
                "parent": None if spec["parent"] is None else name + "." + spec["parent"],
                "ord": spec["ord"],
                "props": dict(spec["props"]),
                "keep": list(spec.get("keep") or []),
            }
        pages.append({"route": "/" + name, "root": name + "." + ROOT_ID, "title": PAGE_TITLE})
    for name in names:
        apply_exit(state, want, name + ".", name, "")
    if default in names:
        apply_exit(state, want, "", default, "1")
    return pages


def cut_of(state, name, prof):
    """`dock(state, screen)` for an output whose profile may never have passed the door.

    A raw profile has no `dock_max` (§ 4.7 leaves it raw and reports the error), and
    `dock` reads one. Skipping the cut instead would make `dock_max` an exception on
    exactly the output whose configuration is broken -- and § 4.30 says "without
    exception". So the type default of the fallback kind stands in for the number, and
    the output is cut like any other (OR-H1.16).
    """
    screens = state["screens"]
    if "dock_max" in (screens.get(name) or {}):
        return dock(state, name)
    patched = dict(state)
    patched["screens"] = dict(screens)
    patched["screens"][name] = dict(screens.get(name) or {}, dock_max=prof["dock_max"])
    return dock(patched, name)


def apply_exit(state, want, prefix, name, switch):
    """Render ONE output out of the one state: the profile's words and the dock's cut.

    This is the ONLY place an output decides anything (§ 4.30): `dock(state, screen)`
    says which tiles it draws. A tile that does not fit is ABSENT here -- the app stays
    present, its window stays open, and on every other output with a tile it can be put
    away (R-23-6, R-24-2).
    """
    root = want.get(prefix + ROOT_ID)
    if root is None:
        return
    prof = profile_of(state, name)
    broken = profile_broken(state, name)
    root["props"].update({"exit": prof["type"], "screen_name": name,
                          "inputs": prof["inputs"],
                          "scale": prof["scale"], "dock": prof["dock"],
                          "dock_max": prof["dock_max"], "switch": switch,
                          "tap": not broken and tap_bound(state, name),
                          "input_line": not broken and input_line(state, name)})
    finger = not broken and tap_bound(state, name)
    kept = set(o for o in cut_of(state, name, prof) if o)
    seats = {e["oid"]: e for e in state["dock_order"] if e["seat"] and not e["empty"]}
    for key in [k for k in want if k.startswith(prefix + DOCK_PREFIX)]:
        spec = want[key]
        if spec["component"] != "display-tile":
            continue
        oid = str(spec["props"].get("oid") or "")
        if oid in kept:
            # § 6.4: only an output with a finger gets the binding.
            spec["props"]["tap"] = finger
            continue
        del want[key]
        if oid in seats:
            # § 4.30: a cut seat tile leaves an EMPTY seat (§ 4.29) -- the place stays,
            # so nothing below it climbs into the gap on this output alone.
            want[prefix + tile_id(oid) + "~seat"] = {
                "component": "display-seat", "parent": prefix + DOCK_ID, "ord": spec["ord"],
                "props": {"seat_ord": str(seats[oid]["seat_ord"] or 0)}, "keep": [],
            }
    dockobj = want.get(prefix + DOCK_ID)
    if dockobj is not None:
        dockobj["props"]["count"] = len(kept)
    mark = want.get(prefix + OS_ID)
    if mark is not None:
        mark["props"]["unseen"] = str(state.get("unseen") or 0)


def _tile(want, oid, view, ord_, entry, said, face_id):
    """One tile of the dock, at the `ord` its caller decided (§ 7.2, § 4.31).

    `tap` starts false and is written per output (§ 6.4): a `phx-click` on an output with
    no finger is a promise the screen cannot keep.
    """
    face = fallback_tile(oid, view)
    for key in ("glyph", "line", "value", "end_at"):
        value = said.get(key)
        if value not in (None, ""):
            face[key] = value
    c = view["curator"]
    want[tile_id(oid)] = {
        "component": "display-tile",
        "parent": DOCK_ID,
        "ord": ord_,
        "props": {
            "glyph": str(face["glyph"] or TILE_FALLBACK_GLYPH),
            "line": cut(face["line"]),
            "value": str(face["value"] or ""),
            "unit": str(said.get("unit") or ""),
            "unread": "1" if said.get("unread") is True else "",
            "end_at": face["end_at"] if isinstance(face["end_at"], int)
                      and not isinstance(face["end_at"], bool) else 0,
            "topic": str(view.get("topic") or ""),
            # Two names, two jobs: `for` is the DOM id the client matches the window on,
            # `oid` is the window id a tap carries back (§ 5.6).
            "for": face_id,
            "oid": oid,
            "tap": False,
            "rung": str(c["rung"] or ""),
            # § 6.10: the tile of an open window says so, and the sheet dims it.
            "open": "1" if c.get("open") else "",
            "pinned": "1" if entry["pinned"] else "",
            "seat": "1" if entry["seat"] else "",
            "rank": str(entry["rank"]),
        },
        "keep": [],
    }


def tile_id(oid):
    """The dock child of a window, derived from the window's own id.

    A window id carries slashes (`view.<slug>.<view_id>/c.a`), and a slash in
    an object id is the child chain: written as a tilde it is one segment
    again, and the tile is a child of the dock rather than of the window.
    """
    return "%s%s.%s" % (DOCK_PREFIX, TILE_KEY, oid.replace("/", "~"))


def cut(text, limit=TILE_LINE_MAX):
    """One line, at most `limit` characters, an ellipsis where it was cut."""
    text = " ".join(str(text if text is not None else "").split())
    return text if len(text) <= limit else text[:limit - 1] + "\u2026"


def fallback_tile(oid, props):
    """The tile of a window that brought none: a glyph, a line, no value (R-D1)."""
    owner, _ = parse_object_id(oid)
    glyph = (TILE_GLYPHS.get(str(props.get("context") or ""))
             or TILE_GLYPHS.get(str(props.get("class") or ""))
             or TILE_FALLBACK_GLYPH)
    line = props.get("title") or props.get("kicker") or owner_slug(owner)
    return {"glyph": glyph, "line": line, "value": "", "end_at": 0}


def scale_text(scale):
    """The scale as the root wears it: `1.6`, `1.0`, `1.25` -- one value, one spelling.

    Two places write it (the root and every mirrored exit) and one sheet reads
    it, so the spelling is a function and not a format string in two hands.
    """
    text = ("%.2f" % scale).rstrip("0")
    return text + "0" if text.endswith(".") else text


def page_ops(want, have, pages, bootstrap):
    """The routes of this screen: written on the bootstrap, and again when the exits change.

    `page.set` refuses a root that does not exist yet, so these calls go LAST
    in the bundle -- after every `object.create` the patch carries.
    """
    held = (have.get(ROOT_ID) or {}).get("props") or {}
    mine = want[ROOT_ID]["props"]
    # The exits AND which of them is the default: a new default moves `/` and
    # the two names between one root and a prefixed one, with the same
    # `screens` text on both sides.
    if (not bootstrap and held.get("screens") == mine.get("screens")
            and held.get("default_screen") == mine.get("default_screen")):
        return []
    return [dict({"op": "page.set"}, **page) for page in pages]


def update_props(spec):
    """The props of an update, with the kept ones left out.

    `object.update` merges per key, so a prop this cell does not name keeps the
    value the display holds -- which is the value a browser wrote. That is what
    makes `keep` the counterpart of the component's own `editable`: the
    component says what a browser MAY write, and `keep` says that this cell will
    not write over it on the next tick. A create writes everything, because
    there is nothing to preserve yet.
    """
    props = dict(spec["props"])
    for key in spec.get("keep") or []:
        props.pop(key, None)
    return props


def patches(want, have, define, bootstrap):
    """The calls that turn `have` into `want`, in an order the display accepts."""
    calls = []
    # A running screen whose root was built against another vocabulary is not
    # a bootstrap and still needs every definition again: new code, old
    # language, and the definitions only ever travelled on the bootstrap pass.
    # Once per change of vocabulary, not per tick -- the root's `vocab` is
    # brought up to date by the same `object.update` that compares the root's
    # props below, so the next pass sees them agree (GH #670).
    stale = (not bootstrap) and ROOT_ID in have \
        and have[ROOT_ID]["props"].get("vocab") != VOCAB
    if bootstrap or stale:
        for component in components():
            calls.append(dict({"op": "component.define"}, **component))
    # The application's own vocabulary, after this cell's own: a view component
    # may only be created once the component it names exists, and the legs of a
    # bundle run in call order.
    for component in define:
        if isinstance(component, dict):
            calls.append(dict({"op": "component.define"}, **component))

    if bootstrap:
        root = want[ROOT_ID]
        calls.append(
            {
                "op": "object.create",
                "id": ROOT_ID,
                "component": root["component"],
                "ord": root["ord"],
                "props": root["props"],
            }
        )
    elif ROOT_ID in have:
        props = update_props(want[ROOT_ID])
        held = have[ROOT_ID]["props"]
        if any(held.get(k) != v for k, v in props.items()):
            calls.append({"op": "object.update", "id": ROOT_ID, "props": props})

    # The roots of the further exits first (`mirror_screens`): a root sorts
    # AFTER its own regions and dock (`display.region` < `display.root`), so
    # it cannot take its place in the sorted walk below. Then the rest,
    # sorted, because sorted IS parent-before-child: a region sorts before
    # every `view.` id, and a wrapper sorts before its own index chain.
    roots = sorted(k for k in want if k != ROOT_ID and want[k]["parent"] is None)
    rest = sorted(k for k in want if k != ROOT_ID and want[k]["parent"] is not None)
    for oid in roots + rest:
        spec = want[oid]
        if oid in have:
            held = have[oid]
            props = update_props(spec)
            if any(held["props"].get(k) != v for k, v in props.items()):
                calls.append({"op": "object.update", "id": oid, "props": props})
            # A move is its own operation: `object.update` writes props and
            # nothing else, so the order of the screen -- which lives in `ord`
            # -- would never actually change without this. A rewrite alone
            # moves nothing now; what moves a view is a view above it going
            # away, a region change, or a different `ord`.
            if held["parent"] != spec["parent"] or held["ord"] != spec["ord"]:
                calls.append(
                    {
                        "op": "object.move",
                        "id": oid,
                        "parent": spec["parent"],
                        "ord": spec["ord"],
                    }
                )
        else:
            calls.append(
                {
                    "op": "object.create",
                    "id": oid,
                    "component": spec["component"],
                    "parent": spec["parent"],
                    "ord": spec["ord"],
                    "props": spec["props"],
                }
            )

    # What no view claims any more. Leaf first, because `object.delete` does not
    # cascade and names the children standing in the way. Only ids this cell
    # mints are swept: a foreign object on an adopted page is not ours.
    orphans = [k for k in have if k not in want and is_ours(k)]
    # Deepest first: an exit taken out of `screens` leaves a whole tree
    # behind, root included, and a root sorts BEFORE its regions -- so the
    # order is the depth in the held tree, not the spelling of the id.
    for oid in sorted(orphans, key=lambda k: (-depth_of(k, have), k)):
        calls.append({"op": "object.delete", "id": oid})
    return calls


def depth_of(oid, have):
    """How many parents an object has on the held screen."""
    n, seen = 0, set()
    parent = (have.get(oid) or {}).get("parent")
    while parent and parent in have and parent not in seen:
        seen.add(parent)
        n += 1
        parent = have[parent].get("parent")
    return n


def is_ours(oid):
    """Whether this cell minted the id -- on this screen or on one of its exits.

    An exit's objects carry the exit's name as a prefix (`desk.view.`), and a
    screen that is taken out of `screens` leaves its whole tree behind. So the
    sweep reads the id after the first dot as well, and only accepts what this
    cell mints on either side of it.
    """
    if oid.startswith((VIEW_PREFIX, REGION_PREFIX, DOCK_PREFIX)) or oid == OLD_MIC_ID:
        return True
    rest = oid.split(".", 1)[1] if "." in oid else ""
    return (rest.startswith((VIEW_PREFIX, REGION_PREFIX, DOCK_PREFIX))
            or rest in (ROOT_ID, OS_ID, DOCK_ID, OLD_MIC_ID))


# ---------------------------------------------------------------------------
# The clock: what is due, and the one strike ordered for it (GH #679)


def iso_z(ms):
    """An epoch in milliseconds as the timer's `at`: RFC 3339, UTC, rounded UP to the second.

    The timer is exact to the second and refuses an `at` that is already past
    when the order arrives; a moment cut DOWN to its second could be, a moment
    rounded up never is.
    """
    return datetime.fromtimestamp(-(-int(ms) // 1000), timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def pass_tick(hop):
    """The clock struck: a pass, and nothing read for it.

    The strike's own `schedule_id` rides the event as `struck`, so the render does not
    ask the timer to remove an order it has already fired.
    """
    event = {"kind": "stroke"}
    if hop.get("schedule_id"):
        event["struck"] = str(hop["schedule_id"])
    return pass_or_queue(event, now_ms(), [])


def due_ops_from_strokes(state, want, have, now, struck=""):
    """Zero, one or two timer ops: remove the last order, add the next (§ 4.34).

    The curator ORDERS; the clock cell keeps the order. `state["strokes"]` is the whole
    ordered list of § 4.34 and the earliest of them is what the clock is told -- `due`
    on the root is the order the clock holds and no curator value.

    Each pass replaces the previous order, so the clock holds at most one schedule for
    this screen. The order that just struck (`struck`) is gone from the timer and is not
    removed. The order's id is derived from the SECOND it is due (`DUE_NAMESPACE`), not
    drawn at random: two passes that run close together order the same second, and the
    clock takes the second as the same order (GH #681, GH #690).
    """
    old = str(((have.get(ROOT_ID) or {}).get("props") or {}).get("due") or "")
    strokes = [t for t in (state.get("strokes") or []) if isinstance(t, (int, float))]
    at = int(strokes[0]) if strokes else None
    ops = []
    if at is None:
        if old and old != struck:
            ops.append(emission("due", {"messages": [], "op": "remove", "schedule_id": old}))
        want[ROOT_ID]["props"]["due"] = ""
        return ops
    at_ms = max(at, now + 1000)
    sec = -(-int(at_ms) // 1000)
    sid = str(uuid.uuid5(DUE_NAMESPACE, "due:%d" % sec))
    want[ROOT_ID]["props"]["due"] = sid
    if old and old not in (struck, sid):
        ops.append(emission("due", {"messages": [], "op": "remove", "schedule_id": old}))
    ops.append(emission("due", {"messages": [], "op": "add", "schedule_id": sid,
                                "schedule_name": "due", "at": iso_z(at_ms),
                                "emit_to": ".", "emit_body": {"messages": []}}))
    return ops


def judge_ops_from_state(state, now):
    """One message to the judge: what `judge_sees` shows (§ 4.4), and nothing else.

    WHETHER it is asked is the pass's own decision (§ 4.3, `step1_judge_call`): the
    interval lives in the state, not on the root, so two outputs cannot ask twice.
    """
    return [{"header": {"route": "judge"},
             "system": {"instructions": {"text": JUDGE_INSTRUCTIONS}},
             "messages": [{"origin": "user", "type": "text",
                           "text": json.dumps(judge_sees(state, now), sort_keys=True)}]}]


def _written_at(value, fallback):
    """A store row's `updated_at` as a moment, never later than the pass's own."""
    try:
        at = int(value)
    except (TypeError, ValueError):
        return fallback
    return min(at, fallback)


def reconcile(state, rows, event, now):
    """Catch a state up with the store's rows (OR-H0.9): the boot's rebuild, and only that.

    Since display 2.7.0 (GH #809) the state lives in the resident cell's memory and every
    write is its own pass on it, so no pass has to catch up any more -- the race this was
    built for (two writes handed the same state row of display 2.5/2.6) is gone with the
    row. What is left is the rebuild of a fresh cell: `boot_rows` hands it an EMPTY state,
    every app row the boot select read, and a stroke as the event. Every row is replayed as
    the `app_write` it was (§ 4.8 a/b/c), at the moment the store wrote it, so `ttl_ms` and
    `since` land where they landed before the restart; a row whose `ttl_ms` has run out is
    skipped, replaying it would only delete it again. The rest row then puts back what no
    row says (`restore_rest`).

    Nothing is marked `withdrawn` here: the state is empty, so there is no window the rows
    could have left behind. A window whose row left the store while the cell was down comes
    back neither from the rows nor from the rest row, and the boot takes it off the display
    without a `leaving` frame (OR-D.D1.12, BOOT "a row gone while the cell was down"). The
    `own` and `written_at` checks keep the function a general catch-up for a caller that
    hands it a state and an app event; the boot passes neither.

    One thing a replay cannot give back: `age`. The pass it repairs is the pass in which
    the window was `fresh`, and that pass is over -- the window arrives `settled` and
    misses its fly-in (OR-D19). Making it `fresh` again would mean writing into the `new`
    list inside the pass section, which is byte-identical with the reference model and
    not ours to reach into; and `since` would then be the boot's moment instead of the one
    the store recorded, which is what `ttl_ms` and the decay are measured from.

    The judge's bookkeeping is put back afterwards: a replayed write is a pass that ran
    before the restart, and it does not ask the judge a second time. What the judge said
    and when it was last asked comes back from the rest row.
    """
    kind = str(event.get("kind") or "")
    own = str(event.get("oid") or "") if kind in ("app_write", "app_withdraw") else ""
    seen = dict(state["views"])
    replays = []
    for row in rows:
        oid = object_id(row.get("owner"), row.get("view_id"))
        if oid == own:
            continue
        at = _written_at(row.get("updated_at"), now)
        held = seen.get(oid)
        if held is not None and _written_at(held.get("written_at"), now) >= at:
            continue
        try:
            ttl = int(row.get("ttl_ms") or 0)
        except (TypeError, ValueError):
            ttl = 0
        if ttl > 0 and at + ttl <= now:
            continue        # already expired: replaying it would only delete it again
        replays.append((at, oid, {"kind": "app_write", "oid": oid,
                                  "view": hints_of_row(row)}))
    if not replays:
        return state
    judge = copy.deepcopy(state["judge"])
    for at, _oid, replay in sorted(replays, key=lambda r: (r[0], r[1])):
        state = run_pass(state, replay, at)
    state["judge"] = judge
    return state


# ---------------------------------------------------------------------------
# The judge (§ 4.3-4.5): what it sees is `judge_sees`, what it answers is a verdict

JUDGE_INSTRUCTIONS = """You are the judge of one person's screen. The guideline you judge by, word for word:
Focus is the state of the whole screen, not one highlighted element. Visible is only what is of use now; everything else is closed, small or gone. At every real event the situation is judged anew: relevant? what priority? does it support the activity? does it distract? can it go entirely?
The display has no agenda of its own. The clock is an application like any other and its window follows score and bar. No sender has a bonus. No rule says "X always stands".
Weigh the current context, the person's activity, time relevance, urgency, importance, running interactions, the cost of an interruption and the person's preferences. Decide not only how something is shown but whether, when and ahead of what.
Show as little as possible at every moment -- and everything that truly matters at that moment.
You are given the screen state as JSON: every present window with its owner, context, class, topic, relevance hint, its own last verdict, rung, age, whether it leads its ladder and a glimpse of its text; beside them the bar, the weights, the last turn and the last answer.
The dock shows everything that is present; your verdict decides only what stands LARGE. A window you hide keeps its tile, so hiding costs the person nothing but the space.
Your numbers: the screen computes score = weight x relevance x decay for every window -- the weight of its context, your `judged_relevance`, and a decay that falls from 1 towards 0 as the window ages -- and draws the window LARGE when score >= bar. All three factors are at most 1, so the bar is a threshold on a PRODUCT: weight 0.8, relevance 0.9 and decay 0.7 make a score of 0.50. A usable bar lies between 0.2 and 0.5, and 0.3 is the normal choice; go higher only when the person wants quiet or is asleep, or when one urgent thing has to push everything else off the screen. From 0.8 up almost nothing can reach the bar any more, and that is a closed screen, not a concentrated one.
A weight is not an off switch: it says how much a context counts right now, for every window of that context at once. A context whose window answers the person's last question weighs at least 0.7 -- and "no sender has a bonus" holds for the conversation too, so `conversation` is not automatically 1.0.
What matters now is the window that shows the answer: when a window's topic or content is what `last_answer` is about (`weather:berlin` after a question about the weather, a card whose title or topic fits the question), give it a `judged_relevance` of 0.8 or more and do not hide it. The conversation (topic `chat`) then steps back by itself: once a window carries the turn the answer belongs to, the screen puts the chat away, so you do not have to hide it.
Your own last verdict rides in the picture as information, not as an anchor. Without a new event -- a new turn, a new answer, a new window, a window whose content changed, a timer that ran out -- do not move a verdict: the same picture gets the same verdict.
Hide only what would disturb the person now. A clock or a weather window with no occasion belongs in the dock, and you say that with a low relevance, never with a high bar.
`topic_dupe` is not yours to lift: a window the curator holds back repeats what another application already says.
A weight below 0.05 is read as 0.05 for the dock's order only, so the dock stays readable whatever you weigh to nothing.
Answer with ONE JSON object and nothing else:
{"bar": <0..1, the threshold the score is measured against; 0.3 unless the situation asks for another>,
 "weights": {"<context>": <0..1>, ...},
 "windows": [{"id": "<window id>", "judged_hidden": <true|false, optional>, "judged_relevance": <0..1, optional>}]}
Your verdict REPLACES the one before it: a window you do not name loses its old verdict, and a context you do not name weighs 0.5."""


def parse_model_json(text):
    """A fenced or wrapped JSON answer, tolerated (the memory hive's lesson)."""
    t = (text or "").strip()
    if t.startswith("```"):
        t = t.split("\n", 1)[1] if "\n" in t else t[3:]
        if t.rstrip().endswith("```"):
            t = t.rstrip()[:-3]
    try:
        return json.loads(t)
    except ValueError:
        a, b = t.find("{"), t.rfind("}")
        if a >= 0 and b > a:
            try:
                return json.loads(t[a:b + 1])
            except ValueError:
                return None
    return None


def pass_verdict(body, hop):
    """The judge answered: a pass without a write, over the state in memory.

    The payload is translated into the ONE event of \u00a7 4.1 here, at the edge, so the pass
    reads a verdict and never a model's answer: `windows` becomes a map by id, and the
    two per-window words keep the names \u00a7 3 gives them.
    """
    if str(hop.get("finish_reason") or "") != "stop":
        return []                                   # error, length, filter: the state stands
    text = next((m.get("text") for m in body.get("messages") or []
                 if isinstance(m, dict) and m.get("type") == "text" and m.get("text")), "")
    answer = parse_model_json(text)
    if not isinstance(answer, dict):
        sys.stderr.write("judge: no JSON in the verdict\n")
        return []
    windows = {}
    for entry in answer.get("windows") or []:
        if not isinstance(entry, dict) or not entry.get("id"):
            continue
        one = {}
        if isinstance(entry.get("judged_hidden"), bool):
            one["judged_hidden"] = entry["judged_hidden"]
        rel = entry.get("judged_relevance")
        if isinstance(rel, (int, float)) and not isinstance(rel, bool):
            one["judged_relevance"] = rel
        windows[str(entry["id"])] = one
    verdict = {"windows": windows,
               "weights": answer.get("weights") if isinstance(answer.get("weights"), dict) else {}}
    if isinstance(answer.get("bar"), (int, float)) and not isinstance(answer.get("bar"), bool):
        verdict["bar"] = answer["bar"]
    return pass_or_queue(dict({"kind": "verdict"}, **verdict), now_ms(), [])


def as_int(value, default):
    """A positive integer, or the default when it is not one."""
    try:
        n = int(value)
    except (TypeError, ValueError):
        return default
    return n if n > 0 else default


def read_knobs(params):
    """The curator's dials out of `params`, RAW (\u00a7 4.7).

    Raw is the point: the door of the reference model normalises settings and profiles
    in every pass and refuses what it replaces -- once, in the pass that replaces it.
    A value normalised here would be refused by nobody and read by nobody.

    Every knob is declared in `contract.settings` with the same default: they are the
    member's dials, and what a member wants shown differently is another value on that
    member's own screen.
    """
    global KNOB_SETTINGS, KNOB_SCREENS
    KNOB_SETTINGS = {k: params[k] for k in SETTING_KEYS if k in params}
    said = params.get("screens")
    KNOB_SCREENS = said if isinstance(said, dict) and said else copy.deepcopy(DEFAULT_SCREENS)
    global VOICE_MOUNT, FONT_BASE, GROUND
    GROUND = "night" if str(params.get("ground") or "") == "night" else "day"
    defaults = params.get("notice_defaults")
    if isinstance(defaults, dict):
        for name, pair in defaults.items():
            if isinstance(pair, list) and len(pair) == 2:
                NOTICE_DEFAULTS[str(name)] = (as_unit(pair[0], DEFAULT_WEIGHT),
                                              as_int(pair[1], 60000))
        CLASS_RELEVANCE.clear()
        CLASS_RELEVANCE.update((k, v[0]) for k, v in NOTICE_DEFAULTS.items())


def main():
    global VOICE_MOUNT, FONT_BASE, BROWSER_MOUNT
    doc = json.load(sys.stdin)
    # The three params this cell reads by name. `voice_mount` names the `voice`
    # cell the screen's microphone joins, so one screen can be pointed at a
    # voice cell that was mounted under another name -- and a screen with no
    # voice cell beside it simply has a button whose join is refused, out loud,
    # on the page. `browser_mount` is the same statement for pages: the cell a
    # `page:<page>` join finds. `font_base` is where the operator serves the two
    # faces from, or empty for no faces at all.
    params = doc.get("params") or {}
    if isinstance(params, dict):
        VOICE_MOUNT = str(params.get("voice_mount") or "voice")
        BROWSER_MOUNT = str(params.get("browser_mount") or "browser")
        FONT_BASE = str(params.get("font_base") or "")
    read_knobs(params if isinstance(params, dict) else {})
    body = doc.get("body") or {}
    envelope = doc.get("envelope") or {}
    header = envelope.get("header") or {}
    hop = header.get("hop") or {}
    ctx = header.get("context") or {}
    origin = str(ctx.get("display_origin") or "")
    route = str(hop.get("route") or "")

    # The hive's own clock and judge answer BEFORE the origin is read: the
    # question to either leaves during a read pass and carries that pass's
    # context with it, and the context comes back on the reply. The lane the
    # hive's edge stamps on the reply is the one thing that says what it is.
    if route == "in_tick":
        return pass_tick(hop)
    if route == "in_tick_error":
        # An order that was already struck cannot be removed; expected, silent.
        return []
    if route == "in_verdict":
        return pass_verdict(body, hop)

    # The replies FIRST, because answering an acknowledgement with a request is
    # how a loop starts (GH #161). `display_origin` is stamped by the hive's own
    # edges and travels back on the reply; nothing in the body could tell these apart.
    if origin == "patch":
        return pass_patched(body, hop)
    if origin == "read":
        return pass_read(body)
    if origin == "views":
        return pass_views(body, ctx, hop)

    if route == "event":
        return pass_event(body)
    if route in ("in_view", "in_withdraw"):
        return pass_request(body, envelope, route == "in_withdraw")
    if route == "in_notice":
        return pass_notice(body, envelope, hop)
    return []


if __name__ == "__main__":
    out = main()
    # One emission is written as an object, several as an array, and an empty
    # list stays an empty array -- which is how a `code` cell says "nothing to
    # send" (`parse_stdout_json`: a top-level array of length 0 is zero
    # emissions).
    sys.stdout.write(json.dumps(out[0] if len(out) == 1 else out))
