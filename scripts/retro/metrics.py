"""The ten numbers of ruling R-P3, and what they are measured from.

Every metric answers in one of three ways: a value with a verdict, or `n/a`
with the reason its source is missing. A missing source is never an abort and
never a red verdict -- a wave that ran without transcripts simply has four
numbers fewer, and the report says which and why.
"""

from __future__ import annotations

import json
import statistics
from pathlib import Path

from . import gates, prompts, reports, transcripts

HERE = Path(__file__).resolve().parent
THRESHOLDS = HERE / "thresholds.json"


def load_thresholds(path: Path | None = None) -> dict:
    """The one file every threshold comes from (R-P3)."""
    return json.loads((path or THRESHOLDS).read_text(encoding="utf-8"))


def collect(wave_dir: Path, transcript_root: Path, sessions=(),
            gate_dir: Path | None = None) -> dict:
    """All evidence of one wave, gathered once.

    `gate_dir` is the second receipt source of the contract (`target/gate/`),
    read only while the wave kept no receipts of its own -- that is the wave
    measuring itself before its receipts are archived (review M2).
    """
    token = wave_dir.name.split("-20")[0]
    found = transcripts.sessions_for(transcript_root, token, sessions)
    orchestrators = [transcripts.scan(p) for p in found]
    agents = [transcripts.scan(p) for s in found
              for p in transcripts.agents_of(s)]
    builders = [a for a in agents if a["role"] == "Bauer"] or agents
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


RULES = {"Q1": _q1, "Q2": _q2, "Q3": _q3, "Q4": _q4, "Q5": _q5,
         "Q6": _q6, "Q7": _q7, "Q8": _q8, "Q9": _q9, "Q10": _q10}


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
    return _de(value)


def _threshold_text(spec) -> str:
    unit, limit = spec["unit"], spec["threshold"]
    if unit == "%":
        return f"≤ {limit:.0f} %"
    if unit == "min":
        return f"≤ {limit:.0f} min"
    if unit == "calls":
        return "= 0" if limit == 0 else f"≤ {limit:.0f}"
    if unit == "tokens / count":
        return f"≤ {_k(int(limit))} / {spec['threshold_secondary']}"
    return f"≤ {_de(limit)}"


def _breached(spec, value) -> bool:
    if spec["unit"] == "tokens / count":
        return value[0] > spec["threshold"] or value[1] > spec["threshold_secondary"]
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
        value, reason = RULES[metric["id"]](evidence)
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
        out.append({
            "id": metric["id"],
            "title": metric["title_de"],
            "value": text,
            "threshold": _threshold_text(metric),
            "verdict": "VERSTOSS" if breached else "OK",
            "advice": metric["advice_de"] if breached else "—",
        })
    return out
