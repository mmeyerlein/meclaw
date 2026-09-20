"""How much of a dispatch prompt is a rule somebody copied in again.

Derived from the P0 tool `prompt_repeat.py` under
`plans/welle-p-2026-09-19/befund/tools/`: split into sentences, normalise,
count in how many DIFFERENT prompts a sentence appears, then measure the
share of words sitting in the repeated ones.

Finding 04 section 7.2 measured 15.6 percent over five waves and 2.8 percent
for the one wave that had a preamble -- which is what the threshold is for.
"""

from __future__ import annotations

import re
import statistics
from collections import Counter

SENTENCE = re.compile(r"(?<=[.!?;:])\s+|\n+|\s+—\s+|\s+–\s+")
DECOR = re.compile(r"^[\s>*\-+#`0-9.)\]\[]+")
SPACE = re.compile(r"\s+")

#: Below this many prompts the document frequency of a sentence says nothing.
MIN_PROMPTS = 3


def _normalise(raw: str) -> str:
    text = DECOR.sub("", raw).strip().strip("`*_ ").lower()
    return SPACE.sub(" ", text).strip(" .;:!?,-")


def _sentences(text: str):
    for raw in SENTENCE.split(text):
        norm = _normalise(raw)
        if len(norm.split()) >= 4:
            yield norm, len(raw.split())


def repeated_share(prompts: list[str]) -> float | None:
    """Median share of prompt words that sit in a repeated sentence.

    `None` when there are not enough prompts to say anything at all.
    """
    prompts = [p for p in prompts if p and p.split()]
    if len(prompts) < 2:
        return None
    minimum = min(MIN_PROMPTS, len(prompts))
    per_prompt = [(list(_sentences(p)), len(p.split())) for p in prompts]
    frequency = Counter()
    for sentences, _total in per_prompt:
        for norm in {s for s, _ in sentences}:
            frequency[norm] += 1
    repeated = {s for s, n in frequency.items() if n >= minimum}
    shares = [100.0 * sum(words for s, words in sentences if s in repeated) / total
              for sentences, total in per_prompt if total]
    return statistics.median(shares) if shares else None
