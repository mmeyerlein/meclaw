#!/usr/bin/env python3
"""The retro of a wave: ten numbers, one table, one line of history (R-P3).

Run it as the last step of a wave, before the receipt is committed:

    python3 scripts/wave_retro.py welle-h3-2026-09-18
    python3 scripts/wave_retro.py welle-h3-2026-09-18 --sessions <id>,<id>
    python3 scripts/wave_retro.py --check welle-h3-2026-09-18
    python3 scripts/wave_retro.py --readme

It reads what the wave already produced -- the strand reports in
`plans/<wave>/berichte/`, the gate receipts and GATE-SUMMARY lines, and the
session transcripts -- and writes `plans/<wave>/retro.md` plus one line in
`plans/retro/RETRO.md`. Sessions are found by the wave marker in the prompt
that started them, or named with `--sessions`.

THE EXIT CODE IS ALWAYS 0. A breached threshold is a finding, a missing
source is an `n/a` with its reason; neither blocks a wave. The one exception
is `--check <wave>`, which returns 1 while the wave has no `retro.md` -- that
is the receipt duty, not a quality gate.

The measuring library is `scripts/retro/`, taken from the P0 tools of wave P
under `plans/welle-p-2026-09-19/befund/tools/`; the thresholds live in
`scripts/retro/thresholds.json` and nowhere else.
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

from retro import metrics, render, transcripts  # noqa: E402

load_thresholds = metrics.load_thresholds


def metric_row(text: str, metric_id: str) -> dict:
    """One row of a rendered retro table, as a mapping. For tests and eyes."""
    for line in text.splitlines():
        cells = [c.strip() for c in line.strip().strip("|").split("|")]
        if len(cells) == 6 and cells[0] == metric_id:
            return dict(zip(
                ("q", "kennzahl", "wert", "schwelle", "verdikt", "vorschlag"),
                cells))
    raise KeyError(f"{metric_id} steht nicht in der Tabelle")


def _default_root() -> Path:
    """The repo this script lives in."""
    return HERE.parent


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(
        description="Wave retro: ten numbers over one wave (ruling R-P3).")
    parser.add_argument("wave", nargs="?",
                        help="the wave directory under plans/, e.g. welle-h3-2026-09-18")
    parser.add_argument("--root", default=None,
                        help="repository root (default: the tree this script is in)")
    parser.add_argument("--transcripts", default=None,
                        help="transcript root (default: this machine's Claude Code projects)")
    parser.add_argument("--sessions", default="",
                        help="comma separated session ids instead of the wave marker")
    parser.add_argument("--gate-dir", default=None,
                        help="gate receipts of the running tree "
                             "(default: <root>/target/gate; read only while "
                             "the wave kept no receipts of its own)")
    parser.add_argument("--since", default=None,
                        help="ignore sessions that started before this date (YYYY-MM-DD)")
    parser.add_argument("--provisional", action="store_true",
                        help="mark the numbers as measured while the wave "
                             "still runs, with the strand count they saw")
    parser.add_argument("--check", action="store_true",
                        help="exit 1 while the wave has no retro.md")
    parser.add_argument("--readme", action="store_true",
                        help="regenerate plans/retro/README.md from the thresholds")
    args = parser.parse_args(argv)

    root = Path(args.root) if args.root else _default_root()
    spec = load_thresholds()

    if args.readme:
        target = root / "plans" / "retro" / "README.md"
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(render.readme(spec), encoding="utf-8")
        print(f"geschrieben: {target}")
        return 0

    if not args.wave:
        parser.error("ohne --readme wird eine Welle gebraucht")

    wave_dir = root / "plans" / args.wave
    if args.check:
        ok = (wave_dir / "retro.md").is_file()
        print(f"{'ok' if ok else 'fehlt'}: {wave_dir / 'retro.md'}")
        return 0 if ok else 1

    if not wave_dir.is_dir():
        print(f"n/a: {wave_dir} gibt es nicht — nichts zu messen.")
        return 0

    sessions = [s for s in re.split(r"[,\s]+", args.sessions) if s]
    troot = transcripts.default_root(args.transcripts)
    gate_dir = Path(args.gate_dir) if args.gate_dir else root / "target" / "gate"
    evidence = metrics.collect(wave_dir, troot, sessions, gate_dir)
    if args.since:
        evidence["agents"] = [a for a in evidence["agents"]
                              if a["first"] and str(a["first"])[:10] >= args.since]
        evidence["builders"] = [a for a in evidence["builders"]
                                if a["first"] and str(a["first"])[:10] >= args.since]
    rows = metrics.rows(evidence, spec)

    retro = wave_dir / "retro.md"
    note = render.provisional_note(evidence) if args.provisional else ""
    retro.write_text(
        render.retro_md(args.wave, rows, evidence, spec, args.provisional),
        encoding="utf-8")
    history = root / "plans" / "retro" / "RETRO.md"
    render.update_history(history, args.wave, rows, note)

    breaches = [r["id"] for r in rows if r["verdict"] == "VERSTOSS"]
    print(f"geschrieben: {retro}")
    print(f"ergaenzt:    {history}")
    print("Verstoesse:  " + (" ".join(breaches) if breaches else "keine"))
    return 0


if __name__ == "__main__":
    sys.exit(main())
