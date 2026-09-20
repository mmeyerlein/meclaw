"""Measuring library behind `scripts/wave_retro.py` (ruling R-P3).

Four readers and two writers, nothing else:

* `transcripts` -- streams Claude Code session transcripts and turns them into
  turns, tool calls and wall clock. Derived from the P0 tools `tokens.py`,
  `transkript_zeitachse.py` and `prompt_extract.py`.
* `gates`       -- GATE-SUMMARY, lock-wait and gate receipts. Derived from the
  P0 tools `gate_harvest.py`, `receipts.py` and `red_gates.py`.
* `reports`     -- the strands of a wave and the findings of their reviews.
* `prompts`     -- repeated sentences across the dispatch prompts of a wave.
  Derived from the P0 tool `prompt_repeat.py`.
* `metrics`     -- the ten numbers Q1..Q10 and their verdicts.
* `render`      -- the wave's `retro.md` and its line in the history.

The P0 tools stay where they are, unchanged, as the record of the measurement
that produced the thresholds; only the functions the retro needs moved here.
"""

from . import gates, metrics, prompts, render, reports, transcripts  # noqa: F401

__all__ = ["gates", "metrics", "prompts", "render", "reports", "transcripts"]
