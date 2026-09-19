"""The display hive pass — display-hive.md § 3 (the state) and § 4 (the pass), executable.

This file is documentation. It reads like the twelve steps of § 4: one pure function per
step, plain dicts, stdlib only, no I/O. `run_pass(state, event, now)` returns a NEW state
and never mutates its input. Where § 4 allowed two readings or was silent, the chosen
reading is marked `# Decision 16.09. <n>` at the line and listed in `decisions.md`.

Sentence references: "§ 4.n" is sentence n of § 4 of display-hive.md; a bare "§ 5.2" is
that document's section and sentence. Every docstring names the sentences it implements.

State shape (mirrors the table of § 3.4; names as there):

    state = {
      "settings": {linger_ms, fade_ms, focus_default, judge_min_interval_ms, judge, default_screen},
      "screens":  {name: profile},                      # after the door of § 4.7
      "views":    {oid: view},                          # the store `views`
      "bar":      float,                                # the effective bar (§ 4.16)
      "weights":  {context: w},                         # written by the judge (§ 4.5)
      "chat":     {"last_turn_id": str|None},           # what § 4.13 reads
      "unseen":   int,                                  # § 4.33
      "strokes":  [epoch ms, ...],                      # ordered by § 4.34
      "dock_order": [entry, ...],                       # bottom → top, before any cut (§ 4.28)
      "judge":    {"last_call": ms|None, "called": bool, "verdict": dict|None},
      "pass":     {...}                                 # what this pass saw (touches, refusals, errors)
    }

    view = {
      "owner": str, <own props: the hints of § 3 + title, kicker, ...>,
      "state": "urgent"|"hidden"|absent, "touched": int|absent,
      "children": {...},                                # tile, lines, content nodes: never a touch
      "ttl_ms": int, "written_at": ms, "withdrawn": bool,
      "verdict": {"judged_relevance": float|None, "judged_hidden": bool|None},
      "curator": {since, dismissed_at, led_until, verdict_cleared, topic_dupe, present, rung,
                  score, decay, age, level, rank, front, open}
    }

Events (one per pass; § 4.1 closed list):
    {"kind": "app_write", "oid", "view"}   the app writes a view (new or update; a write
                                            that changes only `children` is the clock's minute)
    {"kind": "app_withdraw", "oid"}
    {"kind": "verdict", "bar", "weights", "windows": {oid: {judged_relevance, judged_hidden}}}
    {"kind": "tap", "for": oid}
    {"kind": "hold"}
    {"kind": "stroke"}
A press on the OS mark is client-only (§ 5.5): no event, no pass.
"""

import copy

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
