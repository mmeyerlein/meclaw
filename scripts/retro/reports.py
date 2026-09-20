"""The strands of a wave and the findings of their reviews.

A strand is a report in `plans/<wave>/berichte/`. Two shapes are accepted,
because the head block (ruling OR-P5) is younger than the waves the retro has
to be able to read:

* `<name>-report.md`, the older shape, or
* `<name>.md` with a `<name>-review.md` next to it.

Anything else in the folder -- a plan, a hand-over, a preparation note -- is
not a strand and is left alone.

The form/substance split of the findings is a HEURISTIC over a wording
vocabulary, not a judgement: finding 01 section 3.4 classified 111 findings by
hand and found 45 percent form. The number here is meant to move with that
share, not to reproduce it exactly, and the README says so.
"""

from __future__ import annotations

import re
from pathlib import Path

#: The head block of a report (OR-P5), if it carries one.
HEAD = re.compile(r"\A---\n(.*?)\n---\n", re.DOTALL)

#: A key of the head block. Anything else is the continuation of the key
#: before it: `strand.sh report` wraps a long `commits:` list over several
#: lines, and a parser that reads line by line then sees a third of it. That
#: is not cosmetic -- the commit list is what says which gate run belongs to
#: the strand, and wave P's `gate` strand lost six of its thirteen commits
#: to the wrap alone.
HEAD_KEY = re.compile(r"([A-Za-z_][\w-]*):\s?(.*)$")

#: A single review finding. Four scales are in use across the waves, and two
#: shapes: a bullet or a table row, and -- the H2/H3 house style -- a heading
#: `### C1 — <sentence>`. A finder that only reads bullets misses most of a
#: review and reports a form share over the handful it did see.
FINDING = re.compile(
    r"^\s*(?:[#>\-*|]+\s*)?(?:\*\*)?"
    r"(C|M|N|F|I|R\d-[CMNF]|Critical|Blocker|Major|Minor|Nit|Fund|Befund)"
    r"\s?(\d+)\b", re.IGNORECASE)

#: Wording, not behaviour: the vocabulary of a form finding.
FORM_WORDS = re.compile(
    r"wortlaut|formulier|tippfehler|schreibweise|changelog|kommentar|docstring"
    r"|jsdoc|doc-comment|rustfmt|\bfmt\b|byte-deckel|deckel|namen?sgebung"
    r"|benennung|heisst|umbenenn|readme|satzstellung|sprache|englisch|deutsch"
    r"|typo|link|zeilenumbruch|markdown",
    re.IGNORECASE)


def head_block(text: str) -> dict:
    """The machine-readable head of a report, as a flat mapping.

    Wrapped values are joined: a line that does not open a key belongs to
    the key above it.
    """
    hit = HEAD.match(text)
    if not hit:
        return {}
    out: dict[str, str] = {}
    key = None
    for line in hit.group(1).splitlines():
        found = HEAD_KEY.match(line)
        if found:
            key = found.group(1)
            out[key] = found.group(2).strip()
        elif key and line.strip():
            out[key] = (out[key] + " " + line.strip()).strip()
    return {k: v.strip().strip('"') for k, v in out.items()}


def commits(strand: dict) -> list[str]:
    """The commits a strand declares in its head block, as short shas.

    Empty for every wave older than the head block (OR-P5) -- and an empty
    list means "cannot say", never "no commits": `gates` then counts every
    run the report quotes, as it did before.
    """
    raw = (strand.get("head") or {}).get("commits") or ""
    return [c.strip() for c in raw.strip("[]").split(",") if c.strip()]


def strands(wave_dir: Path) -> list[dict]:
    """Every strand of the wave, with its report and its review."""
    folder = wave_dir / "berichte"
    if not folder.is_dir():
        return []
    files = {p.name: p for p in sorted(folder.glob("*.md"))}
    found = []
    for name, path in files.items():
        stem = path.stem
        if stem.endswith("-review"):
            continue
        base = stem[:-7] if stem.endswith("-report") else stem
        review = files.get(base + "-review.md")
        text = path.read_text(encoding="utf-8", errors="replace")
        head = head_block(text)
        if review is None and not stem.endswith("-report") and not head:
            continue
        found.append({
            "name": head.get("strang") or base,
            "report": path,
            "review": review,
            "head": head,
        })
    return found


def _review_text(strand: dict) -> str:
    """The review of a strand: its own document, else the report's section."""
    if strand["review"] is not None:
        return strand["review"].read_text(encoding="utf-8", errors="replace")
    text = strand["report"].read_text(encoding="utf-8", errors="replace")
    hit = re.search(r"^##+\s*Review\b(.*)", text, re.MULTILINE | re.DOTALL)
    return hit.group(1) if hit else ""


def _blocks(text: str) -> dict[str, list[str]]:
    """The findings of one review, keyed by their id.

    A review names a finding up to three times -- once in a summary table,
    once as the heading of its own section, once in a closing list. They are
    ONE finding, so the id is the key and every mention feeds the same block.
    Everything under a heading until the next one belongs to it: the sentence
    that says what is wrong lives there, not in the heading.
    """
    out: dict[str, list[str]] = {}
    current = None
    for line in text.splitlines():
        hit = FINDING.match(line)
        if hit:
            current = (hit.group(1).upper()[:1] + hit.group(2))
            out.setdefault(current, []).append(line)
            continue
        if line.startswith("#"):
            current = None
        elif current:
            out[current].append(line)
    return out


def findings(wave_dir: Path) -> tuple[int, int]:
    """(all findings, form findings) over every review of the wave."""
    total = form = 0
    for strand in strands(wave_dir):
        for lines in _blocks(_review_text(strand)).values():
            total += 1
            if FORM_WORDS.search("\n".join(lines)):
                form += 1
    return total, form
