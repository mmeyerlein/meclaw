"""Write the wave's `retro.md`, its line in the history, and the rule sheet.

Three outputs, three rules:

* `plans/<wave>/retro.md` -- at most 40 lines. A retro nobody reads is not a
  retro, so the suggestion column stays empty where the number is fine.
* `plans/retro/RETRO.md`  -- one line per wave, replaced in place when the
  wave is measured again. Idempotent by wave name.
* `plans/retro/README.md` -- generated from `thresholds.json`, so the rule
  sheet and the numbers can never say different things.
"""

from __future__ import annotations

import datetime
import re
from pathlib import Path

LINE_LIMIT = 40

HISTORY_HEAD = [
    "# Retro-Verlauf",
    "",
    "Eine Zeile je Welle, erzeugt von `scripts/wave_retro.py` (Ruling R-P3).",
    "Regelsatz und Schwellen: [README.md](README.md). Ein Verstoss ist ein",
    "Befund, nie ein Blocker.",
    "",
    "| Datum | Welle | Q1 | Q2 | Q3 | Q4 | Q5 | Q6 | Q7 | Q8 | Q9 | Q10 | Verstoesse |",
    "|---|---|---|---|---|---|---|---|---|---|---|---|---|",
]


def wave_date(wave: str) -> str:
    hit = re.search(r"(20\d\d-\d\d-\d\d)", wave)
    return hit.group(1) if hit else datetime.date.today().isoformat()


def _breaches(rows) -> list[str]:
    return [r["id"] for r in rows if r["verdict"] == "VERSTOSS"]


def provisional_note(evidence: dict) -> str:
    """How many strands a provisional number was measured over.

    A wave measures itself in ONE worktree while the other strands are still
    unmerged, so Q1, Q3 and Q9 are the numbers of the strands that are there
    (review M1). The line says it, and the wave's orchestrator makes it again
    after the merge.
    """
    count = len(evidence["strands"])
    return f"vorläufig ({count} Strang)" if count == 1 \
        else f"vorläufig ({count} Stränge)"


def retro_md(wave: str, rows, evidence: dict, spec: dict,
             provisional: bool = False) -> str:
    """The wave's own retro, capped at 40 lines."""
    runs = evidence["runs"]
    red = sum(1 for r in runs if r["verdict"] == "RED")
    total, form = evidence["findings"]
    breaches = _breaches(rows)
    out = [
        f"# Retro — {wave}",
        "",
        f"Erzeugt {datetime.date.today().isoformat()} von `scripts/wave_retro.py`; "
        f"Schwellen aus Ruling {spec['ruling']} ({spec['decided']}), "
        "Regelsatz in `plans/retro/README.md`.",
        "",
        (f"**{provisional_note(evidence)}** — gemessen, solange die Welle "
         "läuft; nach dem Merge aller Stränge noch einmal erzeugen.\n"
         if provisional else "") +
        f"Quellen: {len(evidence['strands'])} Strang-Berichte · "
        f"{len(runs)} Strang-Gate-Läufe ({red} rot) · "
        f"{total} Review-Funde ({form} Form) · "
        f"{len(evidence['orchestrators'])} Sitzungen mit "
        f"{len(evidence['agents'])} Agenten ({len(evidence['builders'])} Bauer).",
        "",
        "| Q | Kennzahl | Wert | Schwelle | Verdikt | Vorschlag |",
        "|---|---|---|---|---|---|",
    ]
    for row in rows:
        out.append(f"| {row['id']} | {row['title']} | {row['value']} | "
                   f"{row['threshold']} | {row['verdict']} | {row['advice']} |")
    out += [
        "",
        ("Verstöße: " + ", ".join(breaches)) if breaches
        else "Verstöße: keine.",
        "",
        "Ein Verstoß ist ein Befund, kein Blocker. Schwellen ändern sich nur "
        "per Ruling mit Datum.",
        "",
    ]
    if len(out) > LINE_LIMIT:
        raise AssertionError(f"retro.md hat {len(out)} Zeilen, erlaubt sind {LINE_LIMIT}")
    return "\n".join(out)


def history_line(wave: str, rows, note: str = "") -> str:
    values = " | ".join(r["value"] for r in rows)
    breaches = _breaches(rows)
    name = f"{wave} — {note}" if note else wave
    return (f"| {wave_date(wave)} | {name} | {values} | "
            f"{' '.join(breaches) if breaches else '—'} |")


def _same_wave(line: str, wave: str) -> bool:
    cells = line.split("|")
    return len(cells) > 2 and cells[2].strip().startswith(wave)


def update_history(path: Path, wave: str, rows, note: str = "") -> None:
    """Replace this wave's line, or append it; never a second line.

    The name cell may carry a note (`vorläufig (2 Stränge)`), so the line of
    a wave is found by the name it starts with -- the final run of a wave
    replaces its provisional line instead of standing next to it.
    """
    line = history_line(wave, rows, note)
    if path.is_file():
        body = [l for l in path.read_text(encoding="utf-8").splitlines()
                if l.startswith("|") and not l.startswith("| Datum")
                and not l.startswith("|---")]
    else:
        body = []
    body = [l for l in body if not _same_wave(l, wave)]
    body.append(line)
    body.sort(key=lambda l: l.split("|")[1].strip())
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("\n".join(HISTORY_HEAD + body) + "\n", encoding="utf-8")


def readme(spec: dict) -> str:
    """The rule sheet, generated from the thresholds file."""
    out = [
        f"# {spec['title']}",
        "",
        f"Ruling {spec['ruling']}, decided {spec['decided']}. "
        "`scripts/wave_retro.py <wave>` is the last step of a wave, "
        "before the receipt is committed.",
        "",
        spec["note"],
        "",
        "| # | Metric | Unit | Threshold | Measured from |",
        "|---|---|---|---|---|",
    ]
    for metric in spec["metrics"]:
        limit = metric["threshold"]
        text = f"{limit:g}"
        if "threshold_secondary" in metric:
            text += f" / {metric['threshold_secondary']:g}"
        # "<= 0" is a rule nobody reads twice; "= 0" is the rule.
        sign = "=" if limit == 0 and "threshold_secondary" not in metric else metric["cmp"]
        out.append(f"| {metric['id']} | {metric['title']} | {metric['unit']} | "
                   f"{sign} {text} | {metric['source']} |")
    out += [
        "",
        "## What the retro writes",
        "",
        "* `plans/<wave>/retro.md` -- the table above with this wave's values, "
        "a verdict per metric and a suggestion where a threshold was missed. "
        "At most 40 lines.",
        "* `plans/retro/RETRO.md` -- one line per wave, replaced in place when "
        "a wave is measured again.",
        "",
        "## What a number cannot say",
        "",
        "* A missing source is `n/a` with its reason, never a red verdict and "
        "never an abort. The exit code is always 0; `--check <wave>` returns 1 "
        "only when the wave has no `retro.md` at all.",
        "* Q5 splits form from substance over a wording vocabulary. That is a "
        "heuristic meant to move with the share a human review would find, not "
        "to reproduce it.",
        "* Q2 reads the section under a run's own summary line, which ends "
        "where the next run begins, and asks two questions of it: does it "
        "name a drift, a measured flake or a foreign session, and does it "
        "also name a failure of the strand's own? A run that carries both is "
        "the strand's. That is the wording of a report, not a verdict on the "
        "failures.",
        "* Q1 and Q3 count a gate run where a strand RAN it: its own "
        "archive under `plans/<wave>/receipts/<strand>/`, and the "
        "GATE-SUMMARY lines of its report whose revision is one of the "
        "commits in its head block. A line a strand quotes about another "
        "wave is not a run of this one. A strand older than the head block "
        "declares no commits, and then every line of its own report counts.",
        "* Q1 and Q3 count what the wave wrote down. Where a run was quoted "
        "without its seconds, or a queue wait as prose, the value carries a "
        "leading `>=` and is a lower bound.",
        "* Q3 is `n/a` above 100 percent. Gate seconds longer than the wall "
        "clock of every strand together are not a share, they are a run "
        "belonging to no strand.",
        "* Q3 adds gate seconds and queue seconds although a summary already "
        "contains its own wait, so the share is an upper bound.",
        "* Runs are counted once per mode, revision, duration and station "
        "count. Two runs of the same commit that took the same second and "
        "ended on the same count collapse into one -- the price of not "
        "counting one run four times, once per document quoting it.",
        "",
        "This file is generated: `scripts/wave_retro.py --readme`. Edit "
        "`scripts/retro/thresholds.json` and run it again.",
        "",
    ]
    return "\n".join(out)
