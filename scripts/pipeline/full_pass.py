"""A full pass after odd effects (owner's ruling 10.10., option 2; welle-tb #1165).

A narrowed pass -- a strand on the coverage map, a merge after
`narrow.merge_decision`, an integration after `narrow.integration_decision`
-- that shows one of

    unexplained red   a RED station whose reason is `flakes.UNEXPLAINED`
    FLAKY             any station with the verdict FLAKY
    map mismatch      a plan note that says the coverage map does not fit
                      (`narrow.map_fits`, `narrow.narrow`: MAP_MISFIT)
    cache doubt       the pass served CACHED stations from keys made for
                      other tools than the lane ran with (`gate._record`)

marks its wave: the NEXT pass of that wave plans full -- an integration the
full suite (`narrow.full_suite`; the browser plan rule of the narrowed branch
falls away with it), a strand or a merge without narrowing. `--full` at the
gate plans the same by hand. The plan says it (`gate: NOTE full pass: <why>`)
and the receipt carries `full_pass: {why, since, run}`.

A GREEN pass of the wave that was planned full (the mark, `--full`, an
integration on the full suite, a release) clears the mark; a red one leaves it
standing. A mark carries the scope of the pass that set it, and only a full
pass at least as wide clears it (re-check S1b M1): an integration or a release
clears every mark, a strand or a merge only the marks of strands and merges --
it runs the resolver floor of its own diff, never the integration's suite. The
mark lives in the store's journal and nothing is deleted:
`full_pass_marked {wave, why, run, scope, strand}` and `full_pass_cleared
{wave, run, scope, strand}`; walking a wave's events newest first, the first
clear of scope `integration` ends the walk, a clear of scope `strand` hides the
older strand marks of the same strand, and the first mark left is the state.

A strand mark binds its own strand and every integration, never the other
strands of the wave (re-check S1b m-4): strand B's GREEN full pass does not
clear strand A's mark, and B's next pass is not planned full by it. The strand
is the `<strand>` of the lease holder (`strand_of`). Fail-closed both ways: a
pass without a strand name (a merge gate runs detached) sees every mark and
clears only marks without a name; a mark without a name binds every strand,
and any strand's GREEN full pass clears it (as before m-4).

The wave is the short name (`welle-x`, the date suffix cut as
`schedule.py` does): `MECLAW_GATE_WAVE`, `--wave`, else the `<wave>/` of the
strand's lease holder (`gate.holder_key`). Without a wave there is no mark --
the note says so.

No store file, no mark. A store that is there but cannot be read may hold
one: the plan goes full and says why (re-check S1b m-5).
"""

import json
import re
import sqlite3

MARKED = "full_pass_marked"
CLEARED = "full_pass_cleared"
BY_HAND = "--full"
# The narrowing's notes that say the coverage map does not fit the tree.
MAP_MISFIT = ("binaries HEAD does not have", "is not in this repository",
              "tests moved since coverage map", "the map does not fit")
# Narrowing notes of a pass that was NOT narrowed.
_UNNARROWED = ("narrowing: off", "narrowing: merge stays unnarrowed")
_DATE = re.compile(r"-\d{4}-\d{2}-\d{2}$")
# The scope of a pass: what its full pass runs (re-check S1b M1).
STRAND, INTEGRATION = "strand", "integration"
SCOPES = (STRAND, INTEGRATION)


def scope_of(mode):
    """`integration` for an integration or a release (the full suite), else `strand`."""
    return INTEGRATION if mode in ("integration", "release") else STRAND


def wave_key(name):
    """The short wave name of `name` (a wave, a `plans/<wave>` path), or None."""
    w = _DATE.sub("", str(name or "").strip().rstrip("/").split("/")[-1])
    return w or None


def wave_of(holder, *named):
    """The wave of a pass: the first `named` wave, else the `<wave>/` of the
    lease holder `<wave>/<strand>[:<n>]`, else None."""
    for name in named:
        if wave_key(name):
            return wave_key(name)
    holder = str(holder or "")
    return wave_key(holder.split("/", 1)[0]) if "/" in holder else None


def strand_of(holder):
    """The `<strand>` of the lease holder `<wave>/<strand>[:<n>]`, or None."""
    holder = str(holder or "")
    if "/" not in holder:
        return None
    return holder.split("/", 1)[1].split(":", 1)[0].strip() or None


# --- the mark in the journal --------------------------------------------------

def pending(db, wave, strand=None):
    """`{why, since, run, scope, strand}` of the mark of `wave` that binds a
    pass of `strand` (None: an integration, or a strand without a name --
    every mark), or None. A mark without a scope needs an integration, a clear
    without one counts for strands only -- the wider demand, the narrower
    clear (fail-closed)."""
    if db is None or not wave:
        return None
    cleared = set()     # the strand names (None: nameless) whose clear hides older marks
    for at, event, data in db.execute(
            "SELECT at, event, data FROM journal WHERE event IN (?, ?) ORDER BY seq DESC",
            (MARKED, CLEARED)):
        data = json.loads(data or "{}")
        if wave_key(data.get("wave")) != wave:
            continue
        if event == CLEARED:
            if data.get("scope") == INTEGRATION:
                return None
            cleared.add(data.get("strand"))
            continue
        scope = data.get("scope") if data.get("scope") in SCOPES else INTEGRATION
        who = data.get("strand")
        if scope == STRAND and (who in cleared or (who is None and cleared)
                                or (strand and who and who != strand)):
            continue
        return {"why": data.get("why") or "", "since": at, "run": data.get("run"),
                "scope": scope, "strand": who}
    return None


def mark(db, wave, why, run, scope=STRAND, strand=None):
    from . import events
    data = {"wave": wave, "why": why, "run": run, "scope": scope}
    if strand:
        data["strand"] = strand
    return events.emit(db, MARKED, "status", data, run=run)


def clear(db, wave, run, scope=STRAND, strand=None):
    from . import events
    data = {"wave": wave, "run": run, "scope": scope}
    if strand:
        data["strand"] = strand
    return events.emit(db, CLEARED, "status", data, run=run)


# --- the plan ----------------------------------------------------------------

def for_plan(flag, wave, db=None, strand=None):
    """The `full` of a plan: `--full`, else the mark of `wave` that binds
    `strand` (`pending`), else None. `db` defaults to the store, read-only: no
    store file, no mark; a store that cannot be read plans full (m-5)."""
    if flag:
        return {"why": BY_HAND, "since": None, "run": None}
    if not wave:
        return None
    if db is not None:
        return pending(db, wave, strand)
    from . import store as store_mod
    ro = None
    try:
        ro = store_mod.open_ro()
        return pending(ro, wave, strand)
    except FileNotFoundError:
        return None
    except (OSError, sqlite3.Error) as exc:
        return {"why": "the store cannot be read (%s), the full-pass mark of %s is unknown"
                % (exc, wave), "since": None, "run": None}
    finally:
        if ro is not None:
            ro.close()


def apply(rows, mode, repo, full):
    """Plan the resolver `rows` full in place; the NOTE lines. An integration
    gets the full suite whatever the narrowing switch says, a strand or a
    merge keeps the resolver floor and is never narrowed."""
    from . import narrow
    why = "full pass: %s" % full.get("why")
    if mode == "integration":
        return narrow.full_suite(rows, repo, [why], env={})
    return ["%s -- no narrowing" % why]


def planned_full(doc):
    """Was the pass planned full: the mark or `--full`, a release, or an
    integration on the full suite?"""
    if doc.get("full_pass") or doc.get("mode") == "release":
        return True
    if doc.get("mode") != "integration":
        return False
    tests = next((s for s in doc.get("stations") or [] if s.get("name") == "tests"), None)
    return bool(((tests or {}).get("select") or {}).get("all"))


def narrowed(doc):
    """Did the narrowing decide on the pass (a `narrowing:` note that is not
    the switch off or a merge left unnarrowed)?"""
    if planned_full(doc):
        return False
    return any(n.startswith("narrowing:") and not n.startswith(_UNNARROWED)
               for n in doc.get("notes") or [])


# --- after the pass ------------------------------------------------------------

def triggers(stations, notes, cache_doubt=None):
    """The odd effects of a finished pass: its receipt `stations`, its plan
    `notes` and the cache doubt of `gate._record` -- `[why]`."""
    from . import flakes
    why = []
    for st in stations or []:
        name, verdict = st.get("name"), st.get("verdict")
        if verdict == "RED" and str(st.get("reason") or "").startswith(flakes.UNEXPLAINED):
            why.append("unexplained red in %s" % name)
        elif verdict == "FLAKY":
            why.append("FLAKY %s" % name)
    misfit = [n for n in notes or [] if any(m in n for m in MAP_MISFIT)]
    if misfit:
        why.append("map mismatch: %s" % re.sub(r"^narrowing: ", "", misfit[0]))
    cached = [st.get("name") for st in stations or [] if st.get("verdict") == "CACHED"]
    if cache_doubt and cached:
        why.append("cache doubt: %s served CACHED, %s" % (", ".join(cached), cache_doubt))
    return why


def after_pass(db, wave, doc, stations, verdict, run, cache_doubt=None, strand=None):
    """Mark or clear the wave after a pass of plan `doc` of `strand` (the
    strand name of a strand or merge pass, None for an integration or a
    strand without a name); the NOTE lines."""
    scope = scope_of(doc.get("mode"))
    strand = strand if scope == STRAND else None
    if planned_full(doc):
        mark_ = pending(db, wave, strand) if wave else None
        if mark_ is None:
            return []
        if verdict == "GREEN":
            clear(db, wave, run, scope, strand)
            left = pending(db, wave, strand)
            if left is not None:   # an integration's or another strand's mark stands
                needs = ("an integration" if left["scope"] == INTEGRATION
                         else "a full pass of strand %s or an integration" % left["strand"])
                return ["full pass: GREEN on %s scope -- the full-pass mark of %s stands (%s, "
                        "needs %s)" % (scope, wave, left["why"], needs)]
            return ["full pass: GREEN -- the full-pass mark of %s is cleared" % wave]
        return ["full pass: %s -- the full-pass mark of %s stands (%s)"
                % (verdict, wave, mark_["why"])]
    if not narrowed(doc):
        return []
    why = triggers(stations, doc.get("notes") or [], cache_doubt)
    if not why:
        return []
    text = "; ".join(why)
    if not wave:
        return ["full pass: %s -- no wave named, no mark" % text]
    mark(db, wave, text, run, scope, strand)
    whose = " (strand %s and the integration)" % strand if strand else ""
    return ["full pass: the next pass of %s plans full%s: %s" % (wave, whose, text)]
