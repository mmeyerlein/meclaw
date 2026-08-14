#!/usr/bin/env python3
"""meclaw-os -- the unwrap/expect ratchet (GH #115).

WHY THIS EXISTS
===============
CONTRIBUTING states the rule: no `unwrap()` / `panic!()` outside tests. Nothing
enforced it. `clippy::unwrap_used` and `clippy::expect_used` are restriction
lints, allow-by-default, so the pipeline's `-D warnings` never saw them -- the
rule lived on trust alone.

Switching them to `deny` in one move is not available: the workspace libraries
carry 184 of them today (13 unwrap, 171 expect). Rewriting that many call sites
is a change to load-bearing code, not a CI task, and one of the files involved
is the frozen corridor file.

So this gate does the one thing that is both true and green today: it pins the
count per package and fails when it GROWS. New code cannot add an unwrap. Old
code gets paid off whenever someone touches it, and the pin follows down.

WHAT IS COUNTED
===============
`cargo clippy --workspace` with both lints warned. No `--all-targets`: only the
default targets (lib, bins) are linted, so `#[cfg(test)]` modules and the
`tests/` directories are out of scope by construction -- exactly the carve-out
CONTRIBUTING intends. Diagnostics are read as JSON, not scraped from prose, so
the number means the lint and nothing else.

`meclaw-testing` is in the budget too, and its number is not debt: the crate IS
test scaffolding, its `expect()`s are the intended shape. The pin is there so
the crate cannot quietly turn into something else.

USAGE
=====
    .github/gates/unwrap_budget.py            # check against the pin
    .github/gates/unwrap_budget.py --write    # re-pin (after a real change)

Exit 0 = no package exceeds its pin. Exit 1 = a package grew, or the package
set changed. A package UNDER its pin is not a failure -- it prints an invitation
to re-pin, and on GitHub it shows up as a warning annotation.
"""

from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
BUDGET = Path(__file__).resolve().parent / "unwrap_budget.txt"
LINTS = ("clippy::unwrap_used", "clippy::expect_used")


def measure() -> dict[str, int]:
    """Run clippy over the workspace's default targets and count the two lints."""
    proc = subprocess.run(
        [
            "cargo", "clippy", "--workspace", "--message-format=json",
            "--", "-W", "clippy::unwrap_used", "-W", "clippy::expect_used",
        ],
        cwd=ROOT,
        capture_output=True,
        text=True,
    )
    if proc.returncode != 0:
        sys.stderr.write(proc.stderr)
        raise SystemExit(f"unwrap budget: cargo clippy failed ({proc.returncode})")

    # package_id spellings differ by cargo version and by whether the directory
    # name matches the crate name ("...#meclaw-colony@0.7.0" vs "...colony#0.7.0").
    # Guessing at the string is how a gate starts counting a package called
    # "0.7.0"; ask cargo instead.
    by_id = workspace_members()

    counts: dict[str, int] = {name: 0 for name in by_id.values()}
    for line in proc.stdout.splitlines():
        if not line.startswith("{"):
            continue
        try:
            msg = json.loads(line)
        except json.JSONDecodeError:
            continue
        if msg.get("reason") != "compiler-message":
            continue
        code = (msg.get("message") or {}).get("code") or {}
        if code.get("code") not in LINTS:
            continue
        pkg_id = msg.get("package_id", "")
        name = by_id.get(pkg_id)
        if name is None:
            raise SystemExit(
                f"unwrap budget: diagnostic from an unknown package id "
                f"{pkg_id!r} -- the workspace moved under the gate"
            )
        counts[name] += 1

    return counts


def workspace_members() -> dict[str, str]:
    """Map package id -> package name for the workspace's own crates."""
    proc = subprocess.run(
        ["cargo", "metadata", "--no-deps", "--format-version", "1"],
        cwd=ROOT, capture_output=True, text=True, check=True,
    )
    meta = json.loads(proc.stdout)
    return {p["id"]: p["name"] for p in meta["packages"]}


def read_budget() -> dict[str, int]:
    pinned: dict[str, int] = {}
    for raw in BUDGET.read_text(encoding="utf-8").splitlines():
        line = raw.split("#", 1)[0].strip()
        if not line:
            continue
        name, _, count = line.partition(" ")
        pinned[name.strip()] = int(count.strip())
    return pinned


def write_budget(counts: dict[str, int]) -> None:
    lines = [
        "# The unwrap/expect ceiling per workspace package (GH #115).",
        "# Format: <package> <max clippy::unwrap_used + clippy::expect_used hits>",
        "# Measured by .github/gates/unwrap_budget.py over the default targets",
        "# (lib + bins) -- test modules and tests/ are out of scope by design.",
        "# A number may only ever go DOWN. Re-pin with `unwrap_budget.py --write`",
        "# and say in the commit why the number moved.",
        "",
    ]
    lines += [f"{name} {counts[name]}" for name in sorted(counts)]
    BUDGET.write_text("\n".join(lines) + "\n", encoding="utf-8")


def main() -> int:
    counts = measure()

    if "--write" in sys.argv:
        write_budget(counts)
        print(f"unwrap budget: re-pinned {BUDGET.relative_to(ROOT)}")
        for name in sorted(counts):
            print(f"  {name} {counts[name]}")
        return 0

    pinned = read_budget()
    failures: list[str] = []

    for name in sorted(set(pinned) | set(counts)):
        have = counts.get(name)
        want = pinned.get(name)
        if want is None:
            failures.append(
                f"{name}: {have} hits, not in the budget file -- a new package "
                f"enters the workspace with its ceiling written down, not by "
                f"default. Run `.github/gates/unwrap_budget.py --write`."
            )
        elif have is None:
            failures.append(
                f"{name}: pinned at {want} but no longer in the workspace -- "
                f"drop the line."
            )
        elif have > want:
            failures.append(
                f"{name}: {have} hits, ceiling is {want} -- {have - want} new "
                f"unwrap()/expect() in library code. CONTRIBUTING: no unwrap "
                f"outside tests. Return a Result, or say why this call site is "
                f"infallible and re-pin deliberately."
            )
        elif have < want:
            print(
                f"::warning::unwrap budget: {name} is down to {have} (pin says "
                f"{want}) -- lower the pin with "
                f"`.github/gates/unwrap_budget.py --write`"
            )
            print(f"  {name}: {have}/{want} (below ceiling)")
        else:
            print(f"  {name}: {have}/{want}")

    if failures:
        print("unwrap budget: FAILED", file=sys.stderr)
        for f in failures:
            print(f"  {f}", file=sys.stderr)
        return 1

    print("unwrap budget: OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
