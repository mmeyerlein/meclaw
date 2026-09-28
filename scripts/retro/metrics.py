"""The numbers of ruling R-P3, and what they are measured from.

Q1..Q10 and Q12 carry a threshold and a verdict; Q11 and Q13 are findings
without one (`BEFUND`): Q11 until a wave has measured what the other cache
lifetime saves, Q13 until a ruling sets a value.

Every metric answers in one of three ways: a value with a verdict, or `n/a`
with the reason its source is missing. A missing source is never an abort and
never a red verdict -- a wave that ran without transcripts simply has four
numbers fewer, and the report says which and why.
"""

from __future__ import annotations

import json
import statistics
from pathlib import Path

from . import cache, gates, prompts, reports, transcripts

HERE = Path(__file__).resolve().parent
THRESHOLDS = HERE / "thresholds.json"


def load_thresholds(path: Path | None = None) -> dict:
    """The one file every threshold comes from (R-P3)."""
    return json.loads((path or THRESHOLDS).read_text(encoding="utf-8"))


def collect(wave_dir: Path, transcript_root: Path, sessions=(),
            gate_dir: Path | None = None, planning=()) -> dict:
    """All evidence of one wave, gathered once.

    `gate_dir` is the second receipt source of the contract (`target/gate/`),
    read only while the wave kept no receipts of its own -- that is the wave
    measuring itself before its receipts are archived (review M2).

    `planning` names the planning session(s) instead of the marker (Q12).
    A planning session is found by its own rule, independent of `sessions`:
    naming the build sessions of a wave does not name its planning.
    """
    token = wave_dir.name.split("-20")[0]
    found = transcripts.sessions_for(transcript_root, token, sessions)
    orchestrators = [transcripts.scan(p) for p in found]
    agents = [transcripts.scan(p) for s in found
              for p in transcripts.agents_of(s)]
    builders = [a for a in agents if a["role"] == "Bauer"] or agents
    # The planning session carries the marker too, so it may already be
    # scanned; a transcript is read once.
    seen = {s["path"]: s for s in orchestrators + agents}
    planned = [[seen.get(str(p)) or transcripts.scan(p)
                for p in [session] + transcripts.agents_of(session)]
               for session in transcripts.planning_sessions(
                   transcript_root, token, planning)]
    runs = gates.runs_of(wave_dir, "strand", gate_dir)
    return {
        "wave": wave_dir.name,
        "wave_dir": wave_dir,
        "strands": reports.strands(wave_dir),
        "runs": runs,
        "lock_s": gates.lock_waits(wave_dir, runs),
        "orchestrators": orchestrators,
        "agents": agents,
        "builders": builders,
        "groups": _groups(agents, builders),
        "findings": reports.findings(wave_dir),
        "undocumented": gates.undocumented(wave_dir),
        "planning": planned,
        "planning_named": bool(planning),
        "plan_parts": reports.plan_parts(wave_dir),
        "reported": reports.reported(wave_dir),
    }


def _groups(agents: list, builders: list) -> list[tuple]:
    """One strand per builder: its builder, its reviews, its fix rounds.

    A strand of wave P is not one agent over the whole wave but three -- the
    builder, the review, the fix round -- and each of them a session of its
    own. The three are found over the words of their descriptions
    (`transcripts.strand_key`): a review and a fix round name the strand
    their builder names, and nothing else.
    """
    out = []
    for builder in builders:
        key = builder["strand_key"]
        members = [a for a in agents
                   if a is not builder and a["strand_key"]
                   and a["strand_key"] <= key]
        out.append((builder, members))
    return out


def _k(value: int) -> str:
    return f"{value / 1000:.0f}k" if value >= 1000 else str(value)


def _de(value: float, digits: int = 1) -> str:
    return f"{value:.{digits}f}".replace(".", ",")


def _q1(e):
    if not e["strands"]:
        return None, "keine Strang-Berichte in berichte/"
    return len(e["runs"]) / len(e["strands"]), None


def _q2(e):
    red = [r for r in e["runs"] if r["verdict"] == "RED"]
    if not red:
        return 0.0, None
    return 100.0 * sum(1 for r in red if r["foreign"]) / len(red), None


def _q3(e):
    """Gate seconds of a strand against the wall clock of that strand.

    The wall clock runs from the builder's dispatch to the last turn of the
    strand -- its fix round or its review, whichever ended last. Measured
    against the builder alone it left out every hour a strand spent in
    review and fix, which is how wave P reported 141 percent: a share above
    100 is not a measurement but a run belonging to no strand, and it says
    so instead of printing a number.
    """
    wall = sum(_strand_wall(b, members) for b, members in e["groups"])
    if not wall:
        return None, "kein Transkript der Bauer"
    gate_s = sum(r["secs"] for r in e["runs"])
    share = 100.0 * (gate_s + e["lock_s"]) / wall
    if share > 100.0:
        return None, ("Gate-Sekunden über der Strang-Wanduhr — "
                      "Läufe ohne zugehörigen Strang")
    return share, None


def _strand_wall(builder: dict, members: list) -> float:
    """First dispatch of the builder to the last turn of the strand."""
    if not builder["first"]:
        return 0.0
    last = max([builder["last"]] + [m["last"] for m in members if m["last"]])
    return (last - builder["first"]).total_seconds()


def _q4(e):
    """The fix round of a strand against its first build.

    Two shapes, because two waves did it two ways: H2 and H3 sent the
    builder back with a coordinator message, so the fix round is a later
    section of the same transcript; wave P gave every fix round a fresh
    agent (`Fix-Runde <strand>`), so it is a transcript of its own. Both are
    counted, and a strand that needed no fix round counts as zero.
    """
    if not e["groups"]:
        return None, "kein Transkript der Bauer"
    ratios = []
    for builder, members in e["groups"]:
        sections = builder["sections"]
        if not sections or not sections[0]:
            continue
        later = sum(m["wall_s"] for m in members if m["role"] == "Fix-Runde")
        ratios.append((sum(sections[1:]) + later) / sections[0])
    if not ratios:
        return None, "keine Abschnittsgrenzen im Transkript"
    return statistics.median(ratios), None


def _q5(e):
    total, form = e["findings"]
    if not total:
        return None, "keine Review-Funde gefunden"
    return 100.0 * form / total, None


def _q6(e):
    if not e["builders"]:
        return None, "kein Transkript der Bauer"
    peak = max(a["peak_ctx"] for a in e["builders"])
    big = max(a["big_creations"] for a in e["builders"])
    return (peak, big), None


def _q7(e):
    if not e["agents"]:
        return None, "kein Transkript der Agenten"
    return float(sum(a["polls"] for a in e["agents"])), None


def _q8(e):
    if not e["agents"]:
        return None, "kein Transkript der Agenten"
    share = prompts.repeated_share([a["prompt"] for a in e["agents"]])
    if share is None:
        return None, "zu wenige Dispatch-Prompts"
    return share, None


def _q9(e):
    if not e["builders"]:
        return None, "kein Transkript der Bauer"
    return statistics.median(a["idle_s"] / 60.0 for a in e["builders"]), None


def _q10(e):
    top = sum(o["read"] for o in e["orchestrators"])
    below = sum(a["read"] for a in e["agents"])
    if not top:
        return None, "kein Transkript der Orchestrator-Sitzung"
    return 100.0 * top / (top + below), None


def _q11(e):
    """Calls after a 5-60 minute pause, their share of the cache writes, and
    the share written to the one-hour cache -- over every agent of the wave,
    as the lesson measured it."""
    got = cache.pauses(e.get("agents") or [])
    if got is None:
        return None, "keine Cache-Writes in den Transkripten der Agenten"
    return (100.0 * got["late"] / got["calls"],
            100.0 * got["late_writes"] / got["writes"],
            100.0 * got["long_writes"] / got["writes"]), None


def input_equivalents(calls, weights: dict) -> float:
    """What the calls cost, in uncached input tokens of their own model.

    The formula of `plans/welle-fix-2026-09-27/cache_ttl_calc.py` `units`:
    input + 5-minute writes x 1,25 + one-hour writes x 2 + reads x the read
    factor of the model's family + output x 5. Ratios, never prices -- the
    prices stay out of a public file (`cache.py`), and the ratios live in
    `thresholds.json`. The calls are the ones `transcripts.scan` counts, one
    per request id with its longest output; Q12 counts no second way.
    """
    total = 0.0
    for c in calls:
        short, long_ = c["cc5"], c["cc1h"]
        if not (short or long_):
            # A transcript without the split of its writes: every write was
            # a 5-minute write, as `cache_ttl_calc.py` `cost_real` reads it.
            short = c["cc"]
        total += (c["inp"] + short * weights["cache_write_5m"]
                  + long_ * weights["cache_write_1h"]
                  + c["cr"] * _read_factor(c["model"], weights["cache_read"])
                  + c["out"] * weights["output"])
    return total


def _read_factor(model: str, reads: dict) -> float:
    name = (model or "").lower()
    for family, factor in reads.items():
        if family != "default" and family in name:
            return factor
    return reads["default"]


def _span(session: list) -> float:
    """Wall clock of one session with its agents, first to last line.

    Timestamps, not clock times: a planning that runs over midnight is one
    span. Two sessions are two spans, never the hours that lie between them.
    """
    firsts = [s["first"] for s in session if s["first"]]
    lasts = [s["last"] for s in session if s["last"]]
    return (max(lasts) - min(firsts)).total_seconds() if firsts else 0.0


def _q12(e, metric):
    """The planning of the wave: its input equivalents, its wall clock, and
    its share of the build tokens (GH #891; ceiling PLANUNG.md rule 8).

    The share is measured over the sessions of the wave that are NOT the
    planning -- the planning session carries the wave marker as well, and
    counted on both sides the share would measure itself.
    """
    sessions = e.get("planning") or []
    if not sessions:
        return None, ("keine Planungssitzung unter --planning"
                      if e.get("planning_named")
                      else "keine Planungssitzung am Marker")
    weights = metric["weights"]
    units = sum(input_equivalents(s["calls"], weights)
                for session in sessions for s in session)
    hours = sum(_span(session) for session in sessions) / 3600.0
    own = {s["path"] for session in sessions for s in session}
    build = sum(input_equivalents(s["calls"], weights)
                for s in (e.get("orchestrators") or []) + (e.get("agents") or [])
                if s["path"] not in own)
    return (units, hours, 100.0 * units / build if build else None), None


def _q12_note(value, metric) -> str:
    """The share of the build tokens: a text beside the verdict, never a
    second threshold (plan R, contract Q12)."""
    target = metric["share_target"]
    if value[2] is None:
        return f"Bau-Token n/a, keine Bau-Sitzung am Marker (Ziel ≤ {target} %)"
    return f"{value[2]:.0f} % der Bau-Token (Ziel ≤ {target} %)"


def _q13(e):
    """Strands with a report but without a plan part, over the plan parts.

    The plan names its parts `plan-parts/<S>-<topic>.md`; a strand the build
    had to add -- a second half, a repair, a measurement -- reports without
    one. A struck strand (a part without a report) stays in the count and
    adds nothing; a file without a head block is no strand at all.
    """
    parts = e.get("plan_parts")
    if parts is None:
        return None, "kein plan-parts/ in der Welle"
    if not parts:
        return None, "keine Plan-Teile in plan-parts/"
    planned = set(parts)
    return ([s for s in e.get("reported") or [] if s not in planned],
            len(parts)), None


RULES = {"Q1": _q1, "Q2": _q2, "Q3": _q3, "Q4": _q4, "Q5": _q5,
         "Q6": _q6, "Q7": _q7, "Q8": _q8, "Q9": _q9, "Q10": _q10,
         "Q11": _q11, "Q12": _q12, "Q13": _q13}

#: Rules that read their own entry of `thresholds.json` (Q12's weights).
WITH_SPEC = frozenset(("Q12",))

#: A text the suggestion column carries whatever the verdict.
NOTES = {"Q12": _q12_note}


def _value_text(spec, value) -> str:
    unit = spec["unit"]
    if unit == "%":
        return f"{value:.0f} %"
    if unit == "min":
        return f"{value:.0f} min"
    if unit == "calls":
        return str(int(value))
    if unit == "tokens / count":
        return f"{_k(value[0])} / {value[1]}"
    if unit == "% / %":
        return f"{_de(value[0])} % / {value[1]:.0f} % (1 h: {value[2]:.0f} %)"
    if unit == "input equivalents / h":
        return f"{_de(value[0] / 1e6)} Mio / {_de(value[1])} h"
    if unit == "supplements / plan parts":
        extra, parts = value
        names = f": {', '.join(extra)}" if extra else ""
        return f"{_de(len(extra) / parts, 2)} ({len(extra)}/{parts}{names})"
    return _de(value)


def _threshold_text(spec) -> str:
    unit, limit = spec["unit"], spec["threshold"]
    if limit is None:
        return "—"
    if unit == "%":
        return f"≤ {limit:.0f} %"
    if unit == "min":
        return f"≤ {limit:.0f} min"
    if unit == "calls":
        return "= 0" if limit == 0 else f"≤ {limit:.0f}"
    if unit == "tokens / count":
        return f"≤ {_k(int(limit))} / {spec['threshold_secondary']}"
    if unit == "input equivalents / h":
        return f"≤ {limit / 1e6:g} Mio".replace(".", ",")
    return f"≤ {_de(limit)}"


def _breached(spec, value) -> bool:
    if spec["threshold"] is None:
        return False
    if spec["unit"] == "tokens / count":
        return value[0] > spec["threshold"] or value[1] > spec["threshold_secondary"]
    if spec["unit"] == "input equivalents / h":
        return value[0] > spec["threshold"]
    return value > spec["threshold"]


#: Which metric is only as complete as the wave's own bookkeeping. Q1 counts
#: runs, Q3 counts their seconds plus the queue, so both sit on exactly the
#: evidence `gates.undocumented` finds and cannot parse (review M4).
LOWER_BOUND = {"Q1": ("runs",), "Q3": ("runs", "locks")}


def _is_lower_bound(evidence: dict, metric_id: str) -> bool:
    missing = evidence.get("undocumented") or {}
    return any(missing.get(kind) for kind in LOWER_BOUND.get(metric_id, ()))


def rows(evidence: dict, spec: dict) -> list[dict]:
    """One row per metric, ready for the table."""
    out = []
    for metric in spec["metrics"]:
        rule = RULES[metric["id"]]
        value, reason = (rule(evidence, metric) if metric["id"] in WITH_SPEC
                         else rule(evidence))
        if value is None:
            out.append({
                "id": metric["id"],
                "title": metric["title_de"],
                "value": "n/a",
                "threshold": _threshold_text(metric),
                "verdict": "n/a",
                "advice": reason or "Quelle fehlt",
            })
            continue
        breached = _breached(metric, value)
        text = _value_text(metric, value)
        if _is_lower_bound(evidence, metric["id"]):
            text = "≥ " + text
        # A metric without a threshold is a finding to read, never a breach.
        if metric["threshold"] is None:
            verdict = "BEFUND"
        else:
            verdict = "VERSTOSS" if breached else "OK"
        advice = metric["advice_de"] if breached else "—"
        if metric["id"] in NOTES:
            note = NOTES[metric["id"]](value, metric)
            advice = note if advice == "—" else f"{note}. {advice}"
        out.append({
            "id": metric["id"],
            "title": metric["title_de"],
            "value": text,
            "threshold": _threshold_text(metric),
            "verdict": verdict,
            "advice": advice,
        })
    return out
