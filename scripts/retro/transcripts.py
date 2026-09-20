"""Read Claude Code session transcripts, streaming, one line at a time.

Derived from the P0 tools `tokens.py` (usage accounting), `prompt_extract.py`
(first user message) and `transkript_zeitachse.py` (sections and wall clock)
under `plans/welle-p-2026-09-19/befund/tools/`. Only what the retro needs.

TWO MEASURING TRAPS ARE BUILT IN, both from finding 02 section 2.1:

1. One API answer is written as SEVERAL assistant lines (one per content
   block) and EVERY line carries the same `usage`. Summing the lines counts
   the same turn two or three times. Turns are therefore keyed by
   `requestId`, and per key the line with the highest `output_tokens` wins --
   the last block carries the full output.
2. A transcript can be 60 MB. Nothing here loads a file whole; everything
   streams, and the dispatch prompt stops reading after the first user line.
"""

from __future__ import annotations

import json
import os
import re
from datetime import datetime
from pathlib import Path

#: A gap longer than this between two turns is idle time, not work (Q9).
IDLE_GAP_S = 300

#: The orchestrator's follow-up message that starts a new section (Q4).
COORDINATOR = re.compile(
    r"(The coordinator sent a message|Another Claude session sent a message)")

#: A fix round that is an agent of its own. Wave P dispatched every one of
#: them as a fresh agent (`plans/PREAMBLE.md` section 2), so no builder had
#: a second section and Q4 read 0,0 over eight fix rounds.
FIX_ROUND = re.compile(r"\s*fix[- ]runde\b", re.I)

#: Words in a description that say a role or a verb, not which strand. What
#: is left is the strand's name, and that is what pairs `Fix-Runde P1 kit`
#: and `Review P1 kit` with `P1 kit bauen`.
NOT_A_NAME = frozenset((
    "strang", "strand", "review", "fix", "runde", "round", "bau", "bauen",
    "baut", "umsetzen", "nachzug", "beweis", "messung", "messen", "mess",
    "der", "die", "das", "und", "ein", "eine", "an", "bis", "zur", "von",
))

#: What a poll looks at (Q7): a log file, a gate station log under
#: `target/gate/<tree>/logs/`, or the output of a background task. The three
#: files the guard of 19.09. names, and the three the release agent of H3
#: read 2 112 times (finding 02 section 3.4).
POLL_FILE = re.compile(
    r"\S*\.log\b|\S*/tasks/[\w.+-]+\.output\b|\S*/logs?/[\w./*+-]*")

#: How a Bash command looks at one. The guard names `tail`, `cat`, `head`
#: and `wc`; `less`, `sed`, `grep` and `rg` are the same act on the same
#: file, and wave P's proof strand waited with `grep '^RESULT '` 38 times.
POLL_CMD = re.compile(r"\b(?:tail|cat|head|wc|less|sed|grep|rg)\b")

#: A bare wait. `sleep` inside a command chain is the same poll (Q7).
SLEEP = re.compile(r"(?:^|[;&|(]\s*)sleep\s+[0-9]")

#: And a wait without a wait: two builders of wave P kept themselves awake
#: with `echo idle`, 1 331 and 209 times (`plans/PREAMBLE.md` section 2).
IDLE = re.compile(r"echo\s+[\"']?idle\b")


def default_root(explicit: str | None = None) -> Path:
    """Where Claude Code keeps this machine's transcripts.

    Derived, never spelled out: the project directory is the home directory
    with its slashes turned into dashes. Spelling it out would put a person's
    name into a public file.
    """
    if explicit:
        return Path(explicit)
    env = os.environ.get("MECLAW_TRANSCRIPTS")
    if env:
        return Path(env)
    home = Path.home()
    return home / ".claude" / "projects" / str(home).replace("/", "-")


def _records(path: Path):
    with open(path, "r", errors="replace") as fh:
        for line in fh:
            line = line.strip()
            if not line.startswith("{"):
                continue
            try:
                yield json.loads(line)
            except ValueError:
                continue


def _text(content) -> str:
    if isinstance(content, str):
        return content
    if not isinstance(content, list):
        return ""
    out = []
    for block in content:
        if isinstance(block, str):
            out.append(block)
        elif isinstance(block, dict) and block.get("type") == "text":
            out.append(block.get("text") or "")
    return "\n".join(out)


def _stamp(value: str | None):
    if not value:
        return None
    try:
        return datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError:
        return None


def first_prompt(path: Path) -> str:
    """The dispatch prompt: the first user message, then stop reading."""
    for rec in _records(path):
        if rec.get("type") != "user":
            continue
        text = _text((rec.get("message") or {}).get("content"))
        if text.strip():
            return text
    return ""


def strand_key(description: str) -> frozenset:
    """Which strand a description belongs to, as a set of words.

    `Fix-Runde P1 kit` and `Review P1 kit` both reduce to {p1, kit}, and
    `P1 kit bauen` to the same -- so a fix round and a review are found as
    subsets of their builder. A description that names the strand before a
    colon (`gate-2: precheck RED stoppt Cargo`) keeps only that name.
    """
    text = description or ""
    head = re.match(r"^([A-Za-z0-9][\w.+-]*):\s", text)
    if head:
        text = head.group(1)
    words = [w.lower() for w in re.split(r"[^0-9A-Za-z]+", text) if w]
    return frozenset(w for w in words if w not in NOT_A_NAME)


def _role(prompt: str, description: str) -> str:
    d = (description or "").lower()
    if FIX_ROUND.match(d):
        return "Fix-Runde"
    if d.startswith("review") or "review" in d:
        return "Review"
    if d.startswith("mess") or "messung" in d:
        return "Messagent"
    if d.startswith("strang") or d.startswith("bau"):
        return "Bauer"
    p = (prompt or "").lower()
    if "reviewer" in p or "plan-fremd" in p or "review-bericht" in p:
        return "Review"
    if "du bist der bauer" in p or "bauer des strangs" in p or "du bist ein opus-bauer" in p:
        return "Bauer"
    return "Sonstiges"


def scan(path: Path) -> dict:
    """Everything the ten metrics need from one transcript."""
    turns: dict[str, dict] = {}
    order: list[str] = []
    stamps: list[datetime] = []
    sections: list[datetime] = []
    prompt = ""
    polls = 0

    for rec in _records(path):
        stamp = _stamp(rec.get("timestamp"))
        if stamp:
            stamps.append(stamp)
        kind = rec.get("type")
        if kind == "user":
            text = _text((rec.get("message") or {}).get("content"))
            if not text.strip():
                continue
            if not prompt:
                prompt = text
            elif COORDINATOR.search(text) and stamp:
                sections.append(stamp)
            continue
        if kind != "assistant":
            continue
        message = rec.get("message") or {}
        key = rec.get("requestId") or message.get("id")
        usage = message.get("usage") or {}
        if key:
            known = turns.get(key)
            if known is None:
                order.append(key)
            if known is None or (usage.get("output_tokens") or 0) >= (
                    known.get("output_tokens") or 0):
                turns[key] = usage
        for block in message.get("content") or []:
            if not isinstance(block, dict) or block.get("type") != "tool_use":
                continue
            if _is_poll(block):
                polls += 1

    read = creation = output = peak = 0
    big_creations = 0
    for key in order:
        usage = turns[key]
        got = ((usage.get("input_tokens") or 0)
               + (usage.get("cache_creation_input_tokens") or 0)
               + (usage.get("cache_read_input_tokens") or 0))
        read += got
        creation += usage.get("cache_creation_input_tokens") or 0
        output += usage.get("output_tokens") or 0
        peak = max(peak, got)
        if (usage.get("cache_creation_input_tokens") or 0) > 100_000:
            big_creations += 1

    stamps.sort()
    wall = (stamps[-1] - stamps[0]).total_seconds() if len(stamps) > 1 else 0.0
    idle = sum((b - a).total_seconds() for a, b in zip(stamps, stamps[1:])
               if (b - a).total_seconds() > IDLE_GAP_S)

    meta = {}
    meta_path = Path(str(path).replace(".jsonl", ".meta.json"))
    if meta_path.is_file():
        try:
            meta = json.loads(meta_path.read_text(errors="replace"))
        except ValueError:
            meta = {}

    return {
        "path": str(path),
        "agent_id": path.stem.replace("agent-", ""),
        "description": meta.get("description", ""),
        "prompt": prompt,
        "role": _role(prompt, meta.get("description", "")),
        "strand_key": strand_key(meta.get("description", "")),
        "turns": len(order),
        "read": read,
        "cache_creation": creation,
        "output": output,
        "peak_ctx": peak,
        "big_creations": big_creations,
        "polls": polls,
        "wall_s": wall,
        "idle_s": idle,
        "first": stamps[0] if stamps else None,
        "last": stamps[-1] if stamps else None,
        "sections": _sections(stamps, sections),
    }


def _is_poll(block: dict) -> bool:
    """Is this tool call a look at a running job?

    EVERY look counts, not only the second one at the same file. The run is
    in the background and the harness wakes the agent when it ends; the
    first look is as unnecessary as the tenth, and each one costs a full
    turn over the agent's whole context (finding 02 section 3.4). Counting
    only repeats reported 46 polls for wave P where its transcripts hold
    over two thousand.
    """
    name = block.get("name")
    args = block.get("input") or {}
    if name == "Read":
        return bool(POLL_FILE.fullmatch(str(args.get("file_path") or "")))
    if name != "Bash":
        return False
    command = str(args.get("command") or "")
    if SLEEP.search(command) or IDLE.search(command):
        return True
    return bool(POLL_CMD.search(command) and POLL_FILE.search(command))


def _sections(stamps, boundaries):
    """Wall clock of each section between the orchestrator's messages.

    Section one is the first build, every later one a fix round -- the same
    cut finding 01 section 3.1 makes, because a strand is ONE agent over the
    whole wave, not one agent per round.

    A section ENDS at its last turn, not at the next order: the hour an agent
    lies waiting for its review belongs to neither section (it is Q9), and
    counting it as build time would make every fix round look cheap.
    """
    if not stamps:
        return []
    edges = sorted(boundaries)
    out = []
    for start, stop in zip([stamps[0]] + edges, edges + [None]):
        inside = [s for s in stamps if s >= start and (stop is None or s < stop)]
        out.append((inside[-1] - start).total_seconds() if inside else 0.0)
    return out


def sessions_for(root: Path, wave_token: str, explicit=()) -> list[Path]:
    """The session transcripts of a wave.

    Either named on the command line, or found by the wave marker in the
    first user message of a session -- `plans/<wave>` or `welle-x` in the
    prompt that started it.
    """
    if not root.is_dir():
        return []
    if explicit:
        found = []
        for name in explicit:
            path = root / (name if name.endswith(".jsonl") else name + ".jsonl")
            if path.is_file():
                found.append(path)
        return found
    marker = re.compile(re.escape(wave_token) + r"\b", re.IGNORECASE)
    return [p for p in sorted(root.glob("*.jsonl"))
            if marker.search(first_prompt(p))]


def agents_of(session: Path) -> list[Path]:
    folder = session.parent / session.stem / "subagents"
    return sorted(folder.glob("agent-*.jsonl")) if folder.is_dir() else []
