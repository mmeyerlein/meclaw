#!/usr/bin/env python3
"""Gate for the roadmap anchors (`ROADMAP.md`).

Binding contract: `docs/development-rules.md` § 5b, seeded out of the
2026-08-28 ruling that split one overloaded word into three places:

    GitHub issues            what is decided work
    docs/defer-register.md   what is deliberately NOT built, with its trigger
    ROADMAP.md               in what order the decided work arrives

A roadmap is the one document in a repository that ages without anybody
touching it: the tracker moves, the file does not, and a stream that still
names work finished two releases ago is worse than no roadmap at all -- it
reads as current and it is not. So every entry names where its substance
actually lives, and this script resolves the name.

ANCHOR GRAMMAR
==============
Every top-level bullet under a stream heading (Now / Next / Later / Alongside)
carries at least one of:

    [#<n>](https://github.com/<owner>/<repo>/issues/<n>)
        A tracker anchor. The issue MUST BE OPEN. A closed issue under a future
        horizon is precisely the defect this gate exists to catch: the work
        shipped, and the line promising it stayed.

    (register: <id>)
        A register anchor, for a topic deliberately not built. It resolves
        against a `reg:<id>` tag in the **Item** cell of the matching row in
        docs/defer-register.md. Ids are lowercase words joined by hyphens and
        are never reused.

`## Shipped` is exempt by construction: it is the graveyard, its lines name
releases rather than open work, and its issue links are closed on purpose.

TWO TREE SHAPES, AND A NETWORK THAT MAY NOT BE THERE
====================================================
Like the two gates next door, this runs in the private tree and in the
published one, and the corpus is not the same in both.

* PRIVATE tree -- docs/defer-register.md is present. Register anchors are
  resolved.
* PUBLIC tree -- the register does not travel (it is internal, and DOCS_MAP
  does not map it). Register anchors cannot be resolved there, so they are
  SKIPPED and COUNTED by name, never silently dropped. The bullet still has to
  carry an anchor; only the far end is unverifiable.

The tracker half has the same shape for a different reason: it needs the
network. A rate limit, an offline runner or a GitHub outage is not a statement
about this commit, so an unreachable API is a counted WARNING and exit 0 --
while a reachable API that says "closed" is exit 1. A gate that goes red for
somebody else's downtime gets switched off, and then it guards nothing.

Detection is a property of the tree and of the run, not a flag.

USAGE
=====
    python3 scripts/check_roadmap_anchors.py [--check] [--offline]

`--check` is accepted for symmetry with check_claims.py / check_adr_anchors.py
and changes nothing; there is no --write mode, because this gate derives no
travelling copy. `--offline` skips the tracker lookup deliberately (useful in a
sandbox); it reports the skip in the same counted form.

Exit 0 = every anchor resolved, or was skipped for a named, counted reason.
Exit 1 = a bullet without an anchor, a closed issue, or a dangling register id.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import urllib.error
import urllib.request
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
ROADMAP = REPO_ROOT / "ROADMAP.md"
REGISTER = REPO_ROOT / "docs" / "defer-register.md"

# The streams that carry future work. `Shipped` is deliberately absent.
STREAMS = ("now", "next", "later", "alongside")

# A heading is `## Now: ...` -- the stream name is what stands before the colon.
HEADING = re.compile(r"^##\s+([^:#\n]+?)\s*(?::.*)?$")

ISSUE_LINK = re.compile(
    r"https://github\.com/([A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+)/issues/(\d+)"
)
REGISTER_ANCHOR = re.compile(r"\(register:\s*([a-z0-9][a-z0-9-]*)\)")
REGISTER_TAG = re.compile(r"`reg:([a-z0-9][a-z0-9-]*)`")

TIMEOUT = 20


class Bullet:
    def __init__(self, stream: str, line_no: int, text: str) -> None:
        self.stream = stream
        self.line_no = line_no
        self.text = text

    @property
    def label(self) -> str:
        first = self.text.strip().splitlines()[0]
        return first[:72] + ("..." if len(first) > 72 else "")


def parse_roadmap(path: Path) -> list[Bullet]:
    """Top-level bullets under a stream heading, continuation lines folded in."""
    bullets: list[Bullet] = []
    stream: str | None = None
    pending: Bullet | None = None

    for n, raw in enumerate(path.read_text(encoding="utf-8").splitlines(), start=1):
        head = HEADING.match(raw)
        if head:
            pending = None
            name = head.group(1).strip().lower()
            stream = name if name in STREAMS else None
            continue
        if stream is None:
            pending = None
            continue
        if raw.startswith("- "):
            pending = Bullet(stream, n, raw[2:])
            bullets.append(pending)
            continue
        if pending is not None and raw.startswith("  ") and raw.strip():
            pending.text += "\n" + raw.strip()
            continue
        pending = None

    return bullets


def open_issue_numbers(repo: str) -> tuple[set[int] | None, str]:
    """Every open issue number of `repo`, or (None, reason) when unreachable.

    Deliberately ONE listing instead of N lookups: the unauthenticated GitHub
    rate limit is 60 requests an hour per IP, and a gate that spends one request
    per roadmap line would start failing the moment the roadmap grew -- which is
    the opposite of what it is for.
    """
    if os.environ.get("MECLAW_ROADMAP_GATE_OFFLINE"):
        return None, "MECLAW_ROADMAP_GATE_OFFLINE is set"

    if shutil.which("gh"):
        try:
            out = subprocess.run(
                [
                    "gh", "api", "--paginate",
                    f"repos/{repo}/issues?state=open&per_page=100",
                    "--jq", ".[] | select(.pull_request == null) | .number",
                ],
                capture_output=True, text=True, timeout=TIMEOUT * 3,
            )
            if out.returncode == 0:
                nums = {int(x) for x in out.stdout.split() if x.strip().isdigit()}
                if nums:
                    return nums, "gh api"
        except (OSError, subprocess.SubprocessError, ValueError):
            pass  # fall through to the plain HTTP path

    nums: set[int] = set()
    page = 1
    headers = {
        "Accept": "application/vnd.github+json",
        "User-Agent": "meclaw-roadmap-gate",
    }
    token = os.environ.get("GITHUB_TOKEN") or os.environ.get("GH_TOKEN")
    if token:
        headers["Authorization"] = f"Bearer {token}"

    while page <= 10:
        url = (
            f"https://api.github.com/repos/{repo}/issues"
            f"?state=open&per_page=100&page={page}"
        )
        req = urllib.request.Request(url, headers=headers)
        try:
            with urllib.request.urlopen(req, timeout=TIMEOUT) as resp:
                batch = json.loads(resp.read().decode("utf-8"))
        except urllib.error.HTTPError as exc:
            if exc.code in (403, 429):
                return None, f"GitHub rate limit or forbidden (HTTP {exc.code})"
            return None, f"GitHub API HTTP {exc.code}"
        except (urllib.error.URLError, OSError, ValueError) as exc:
            return None, f"GitHub API unreachable ({exc})"

        if not batch:
            break
        nums.update(
            int(item["number"]) for item in batch
            if "pull_request" not in item
        )
        if len(batch) < 100:
            break
        page += 1

    return nums, "api.github.com"


def register_ids(path: Path) -> set[str] | None:
    if not path.is_file():
        return None
    return set(REGISTER_TAG.findall(path.read_text(encoding="utf-8")))


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--check", action="store_true",
                    help="accepted for symmetry with the sibling gates; a no-op")
    ap.add_argument("--offline", action="store_true",
                    help="skip the tracker lookup and report the skip")
    args = ap.parse_args()

    if not ROADMAP.is_file():
        print(f"roadmap anchors: {ROADMAP.name} is missing", file=sys.stderr)
        return 1

    bullets = parse_roadmap(ROADMAP)
    if not bullets:
        print(
            "roadmap anchors: no bullet found under any of "
            f"{', '.join(STREAMS)}. Either the headings were renamed or the "
            "streams are empty; both are a change somebody has to make "
            "knowingly, not a green run.",
            file=sys.stderr,
        )
        return 1

    failures: list[str] = []
    notices: list[str] = []

    # --- anchors present -----------------------------------------------------
    wanted_issues: dict[int, list[Bullet]] = {}
    wanted_registers: dict[str, list[Bullet]] = {}
    repos: set[str] = set()

    for b in bullets:
        issues = ISSUE_LINK.findall(b.text)
        registers = REGISTER_ANCHOR.findall(b.text)
        if not issues and not registers:
            failures.append(
                f"ROADMAP.md:{b.line_no}: no anchor -- '{b.label}'\n"
                f"    A stream entry names an OPEN issue or a "
                f"(register: <id>) row. See docs/development-rules.md § 5b."
            )
            continue
        for repo, num in issues:
            repos.add(repo)
            wanted_issues.setdefault(int(num), []).append(b)
        for rid in registers:
            wanted_registers.setdefault(rid, []).append(b)

    # --- register anchors ----------------------------------------------------
    ids = register_ids(REGISTER)
    if ids is None:
        if wanted_registers:
            notices.append(
                f"{len(wanted_registers)} register anchor(s) SKIPPED: "
                f"docs/defer-register.md does not travel to this tree "
                f"({', '.join(sorted(wanted_registers))})"
            )
    else:
        for rid, users in sorted(wanted_registers.items()):
            if rid in ids:
                continue
            lines = ", ".join(f"ROADMAP.md:{b.line_no}" for b in users)
            failures.append(
                f"{lines}: (register: {rid}) resolves to nothing -- "
                f"docs/defer-register.md carries no `reg:{rid}` tag.\n"
                f"    Either the row moved to the archive (then the roadmap "
                f"line goes too) or the tag was never written."
            )

    # --- issue anchors -------------------------------------------------------
    if wanted_issues:
        if len(repos) > 1:
            failures.append(
                "ROADMAP.md links issues of more than one repository: "
                + ", ".join(sorted(repos))
            )
        repo = sorted(repos)[0]
        if args.offline:
            notices.append(
                f"{len(wanted_issues)} issue anchor(s) SKIPPED: --offline"
            )
        else:
            open_nums, how = open_issue_numbers(repo)
            if open_nums is None:
                notices.append(
                    f"{len(wanted_issues)} issue anchor(s) SKIPPED: {how}. "
                    f"An unreachable tracker is not a statement about this "
                    f"commit."
                )
            else:
                for num, users in sorted(wanted_issues.items()):
                    if num in open_nums:
                        continue
                    lines = ", ".join(f"ROADMAP.md:{b.line_no}" for b in users)
                    failures.append(
                        f"{lines}: #{num} is not open on {repo}.\n"
                        f"    A closed issue leaves its stream and appears once "
                        f"under § Shipped; an issue that turns out to wait on an "
                        f"event becomes a row in docs/defer-register.md and the "
                        f"line gets a (register: <id>) anchor instead."
                    )
                notices.append(
                    f"{len(wanted_issues)} issue anchor(s) resolved via {how} "
                    f"({len(open_nums)} open on {repo})"
                )

    for notice in notices:
        print(f"  note: {notice}")

    if failures:
        print(
            f"\nroadmap anchors RED ({len(failures)}):", file=sys.stderr
        )
        for f in failures:
            print(f"  - {f}", file=sys.stderr)
        return 1

    print(
        f"roadmap anchors OK: {len(bullets)} bullet(s) across "
        f"{', '.join(STREAMS)}; {len(wanted_issues)} issue anchor(s), "
        f"{len(wanted_registers)} register anchor(s)."
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
