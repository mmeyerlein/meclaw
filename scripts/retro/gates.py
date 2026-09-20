"""Gate runs of a wave: how many, how long, how many red, and whose fault.

Derived from the P0 tools `gate_harvest.py`, `receipts.py` and `red_gates.py`
under `plans/welle-p-2026-09-19/befund/tools/`.

TWO THINGS THE NUMBERS WOULD LIE ABOUT OTHERWISE:

* One run is quoted in several places -- the report, the review, the wave
  receipt, and often once more in a commit message. Runs are therefore keyed
  by `(mode, rev, secs)`. Two genuinely different runs of the same commit
  that took the same second collapse into one; that is the documented cost of
  not counting one run four times.
* A receipt JSON and a GATE-SUMMARY line describe the SAME run. The receipt
  has no top-level `secs`, so its wall clock is `finished - started` -- which
  is exactly the number the summary prints (finding 01 section 3.3).
"""

from __future__ import annotations

import json
import re
from datetime import datetime
from pathlib import Path

from . import reports

SUMMARY = re.compile(
    r"GATE-SUMMARY\s+(?P<mode>strand|integration|release|ci)\s+"
    r"(?P<rev>[0-9a-f]{6,40})\s+(?P<green>\d+)/(?P<total>\d+)\s+"
    r"(?P<secs>\d+)s\s+(?P<verdict>GREEN|RED)")

LOCK_WAIT = re.compile(r"GATE lock-wait \[(\d+)s behind other runs\] (\d+)s")

#: What the two patterns above CANNOT count: a summary line quoted without
#: its seconds, and a queue wait written as prose. Finding 01 section 3.2
#: names three runs of H2 in the first shape and half a dozen waits in the
#: second, and ends with the sentence that makes them matter: "die
#: Berichts-Zählung ist die Untergrenze". They are not guessed into the
#: numbers -- they turn Q1 and Q3 into a lower bound that says so.
LOOSE_SUMMARY = re.compile(
    r"GATE-SUMMARY\s+(?:strand|integration|release|ci)\s+\S+\s+\d+/\d+")
LOOSE_LOCK = re.compile(r"[Ll]ock[_ -]?[Ww]ait|[Ll]ock-Wartezeit|behind other runs")

#: What makes a red run somebody else's fault: the markers finding 01
#: section 3.2 classified the twenty-three red runs of H2/H3/G0 by -- a drift
#: of the base, a measured flake, a foreign session in the same target tree.
#: Nothing wider: a plain "Flake" in prose says which run somebody talked
#: about, not which run was one.
FOREIGN = {
    "Basis-Drift": r"710::the_copy_is_the_document|the_copy_is_the_document"
                   r"|710-Drift|Basis-Drift",
    "Flake": r"registry lacks|#721|gh643",
    "fremde Session": r"fremde[rn]? Session|foreign tree|Geister-Binary",
}

#: ... and what takes it back. A run that ALSO failed for a reason of its own
#: is the strand's, even when a second station was somebody else's -- that is
#: how the hand count in section 3.2 classified every mixed run (chat-time
#: 1154 s and 1924 s each carry a genuine failure next to a contention one,
#: and count as the strand's). These are the words the reports mark the
#: genuine half with.
OWN = (r"\*\*Echt|war echt|echter Fehler|eigener Fehler|\bMeiner\b"
       r"|einer meiner|Roter, meiner|Aufr\u00e4umarbeit dieses Strangs")


def _mark(section: str) -> list[str]:
    if re.search(OWN, section):
        return []
    return [name for name, pattern in FOREIGN.items()
            if re.search(pattern, section)]


def _sections(lines: list[str]) -> list[tuple[int, str]]:
    """Every GATE-SUMMARY line with the text that belongs to THAT run.

    A run's reason stands under its own summary line and ends where the next
    summary line starts. A window of fixed size around the line reads the
    neighbour's reason as well, and in a report that documents three runs in
    forty lines every red run then looks foreign (review C1: H2 83 % against
    33 % by hand, G0 100 % against at most one of three).
    """
    marks = [n for n, line in enumerate(lines) if SUMMARY.search(line)]
    out = []
    for index, start in enumerate(marks):
        end = marks[index + 1] if index + 1 < len(marks) else len(lines)
        out.append((start, "\n".join(lines[start:end])))
    return out


#: Where a wave RAN a gate: its strand reports, its receipt, its receipt
#: files. Everything else under `plans/<wave>/` -- a plan, a finding, a
#: measurement -- may QUOTE a run of another wave, and a wave that measures
#: other waves quotes dozens. Those are not its runs.
RUN_SOURCES = ("berichte", "receipts", "receipt.md", "RECEIPT.md")


def _is_own(path: Path, wave_dir: Path) -> bool:
    parts = path.relative_to(wave_dir).parts
    return parts[0] in RUN_SOURCES


def from_markdown(wave_dir: Path) -> list[dict]:
    """Every gate run the wave's own reports and receipts carry."""
    runs = []
    for path in sorted(p for p in wave_dir.rglob("*.md") if _is_own(p, wave_dir)):
        lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
        for number, section in _sections(lines):
            hit = SUMMARY.search(lines[number])
            runs.append({
                "mode": hit.group("mode"),
                "rev": hit.group("rev"),
                "green": int(hit.group("green")),
                "total": int(hit.group("total")),
                "secs": int(hit.group("secs")),
                "verdict": hit.group("verdict"),
                "foreign": _mark(section) if hit.group("verdict") == "RED" else [],
                "source": f"{path.name}:{number + 1}",
            })
    return runs


def from_receipts(folder: Path, pattern: str = "*.json") -> list[dict]:
    """Every gate run whose receipt JSON lies in one folder.

    The folder is a strand's archive under `plans/<wave>/receipts/<strand>/`
    or, while a wave measures itself before archiving, `target/gate/` with
    the pattern `*/last-*.json` (review M2).
    """
    runs = []
    for path in sorted(folder.glob(pattern)):
        try:
            doc = json.loads(path.read_text(errors="replace"))
        except ValueError:
            continue
        if not isinstance(doc, dict) or "stations" not in doc:
            continue
        started, finished = doc.get("started"), doc.get("finished")
        secs = 0
        if started and finished:
            try:
                secs = int((datetime.fromisoformat(finished)
                            - datetime.fromisoformat(started)).total_seconds())
            except ValueError:
                secs = 0
        runs.append({
            "mode": doc.get("mode", ""),
            "rev": (doc.get("rev") or "")[:40],
            "secs": secs,
            "verdict": doc.get("verdict", ""),
            "foreign": [],
            "lock_wait_secs": int(doc.get("lock_wait_secs") or 0),
            "source": path.name,
        })
    return runs


def lock_waits(wave_dir: Path, runs: list[dict]) -> int:
    """Seconds spent in the runner's queue, counted once per distinct value.

    The same wait is printed as a `GATE lock-wait` line AND written into the
    receipt, so the two sources are merged over their value, not added.
    `runs` are the COUNTED runs of the wave -- a queue wait belongs to a run
    of this wave or to nothing.
    """
    seen = set()
    for strand in reports.strands(wave_dir):
        for path in (strand["report"], strand["review"]):
            if path is None:
                continue
            text = path.read_text(encoding="utf-8", errors="replace")
            for hit in LOCK_WAIT.finditer(text):
                seen.add(int(hit.group(2)))
    for run in runs:
        if run.get("lock_wait_secs"):
            seen.add(run["lock_wait_secs"])
    return sum(seen)


def undocumented(wave_dir: Path) -> dict:
    """How much evidence the strict patterns had to drop, by kind.

    A non-zero count does not change a number; it makes the number a lower
    bound, and the retro prints it with a leading sign (review M4).
    """
    runs = locks = 0
    for path in sorted(p for p in wave_dir.rglob("*.md") if _is_own(p, wave_dir)):
        text = path.read_text(encoding="utf-8", errors="replace")
        runs += len(LOOSE_SUMMARY.findall(SUMMARY.sub("", text)))
        locks += len(LOOSE_LOCK.findall(LOCK_WAIT.sub("", text)))
    return {"runs": runs, "locks": locks}


def _short(rev: str) -> str:
    """The key a revision is compared and de-duplicated by.

    A receipt writes the full forty characters, a GATE-SUMMARY line seven.
    Keyed by the string, the archive and the report of the SAME run are two
    runs -- which is half of what made wave P report 8,3 runs per strand.
    """
    return (rev or "")[:7]


def _belongs(rev: str, commits: list[str]) -> bool:
    """Is this run's revision a commit of the strand?

    Without a commit list (every wave before the head block, OR-P5) there is
    nothing to compare against and every quoted run counts, as before.
    """
    if not commits:
        return True
    short = _short(rev)
    return any(c.startswith(short) or short.startswith(_short(c))
               for c in commits)


def _archived(wave_dir: Path, strand: dict, mode: str,
              commits: list[str]) -> list[dict]:
    """The receipts the strand's own gate runs left under `receipts/<name>/`.

    The archive wins over the quoted line: it carries the queue wait and the
    exact span, and it exists even for a run nobody wrote down. It is not a
    free pass, though -- a strand that worked in the main tree inherited 33
    one-second receipts of throw-away repos there, so a receipt whose rev is
    a commit of no strand is dropped like a quoted foreign line.
    """
    folder = wave_dir / "receipts" / strand["name"]
    if not folder.is_dir():
        return []
    runs = []
    for run in from_receipts(folder):
        if run["mode"] == mode and _belongs(run["rev"], commits):
            runs.append(run)
    return runs


def _quoted(strand: dict, mode: str, commits: list[str]) -> list[dict]:
    """The runs the strand's report and review write down.

    A strand of wave P reviewed other waves and quoted their GATE-SUMMARY
    lines by the dozen; a review quotes the run it is reviewing. Only a line
    whose revision is a commit of THIS strand is a run of this strand.
    """
    runs = []
    for path in (strand["report"], strand["review"]):
        if path is None:
            continue
        lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
        for number, section in _sections(lines):
            hit = SUMMARY.search(lines[number])
            if hit.group("mode") != mode or not _belongs(hit.group("rev"), commits):
                continue
            runs.append({
                "mode": hit.group("mode"),
                "rev": hit.group("rev"),
                "green": int(hit.group("green")),
                "total": int(hit.group("total")),
                "secs": int(hit.group("secs")),
                "verdict": hit.group("verdict"),
                "foreign": _mark(section) if hit.group("verdict") == "RED" else [],
                "source": f"{path.name}:{number + 1}",
            })
    return runs


def _run_key(run: dict) -> tuple:
    """What makes two runs the same run.

    A quoted line also carries the station count, and it has to: the rerun
    of one station and its green repeat carry the same commit and the same
    second and differ only there (review M5). A receipt has no station
    count, so it is matched against the quoted runs by mode, revision and
    seconds alone -- and wins, which `runs_for_strand` does by checking that
    shorter key before it keeps a quoted run.
    """
    if "green" in run:
        return ("q", run["mode"], _short(run["rev"]), run["secs"],
                run["green"], run["total"])
    return ("r", run["mode"], _short(run["rev"]), run["secs"])


def runs_for_strand(wave_dir: Path, strand: dict,
                    mode: str = "strand") -> list[dict]:
    """Every run of one strand, each counted once.

    Two sources, merged over `(mode, short rev, seconds)`: the strand's gate
    archive, which wins, and the GATE-SUMMARY lines of its report and review.
    A quoted run is kept when the archive has no run of that commit and
    second -- the rerun of a station and its green repeat carry the same
    commit and the same second and differ only in the station count, so
    those two are still told apart among themselves (review M5).
    """
    commits = reports.commits(strand)
    merged: dict[tuple, dict] = {}
    for run in _archived(wave_dir, strand, mode, commits):
        merged.setdefault(_run_key(run), run)
    archived = {k[1:] for k in merged}
    quoted: dict[tuple, dict] = {}
    for run in _quoted(strand, mode, commits):
        quoted.setdefault(_run_key(run), run)
    for key, run in quoted.items():
        if key[1:4] in archived:
            continue
        merged.setdefault(key, run)
    return list(merged.values())


def runs_of(wave_dir: Path, mode: str = "strand",
            gate_dir: Path | None = None) -> list[dict]:
    """All runs of one mode over the whole wave, each counted once.

    A run is counted where a strand RAN it -- its archive or its own report.
    Everything else under `plans/<wave>/` may QUOTE a run of another wave,
    and a wave that measures other waves quotes dozens.

    `gate_dir` is the second receipt source of the contract, read only while
    the wave has no run of its own: that is the wave measuring itself before
    its receipts are archived (review M2).
    """
    merged: dict[tuple, dict] = {}
    for strand in reports.strands(wave_dir):
        for run in runs_for_strand(wave_dir, strand, mode):
            merged.setdefault(_run_key(run), run)
    if merged or gate_dir is None or not gate_dir.is_dir():
        return list(merged.values())
    for run in from_receipts(gate_dir, pattern="*/last-*.json"):
        if run["mode"] == mode:
            merged.setdefault(_run_key(run), run)
    return list(merged.values())
