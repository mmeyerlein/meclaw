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

THE CLOCK COLUMNS (tv-p1b, VORGABEN TV-Z8 / § 5.4)
=================================================
The same clippy run reads `.github/gates/clock-bans/clippy.toml` through
`CLIPPY_CONF_DIR` -- only here, never in the clippy station -- and counts
`clippy::disallowed_methods` per package in two more columns, chosen by the
`reason` prefix of the ban: `mono` (std `Instant::now`/`elapsed`: production
reads `tokio::time::Instant`, virtual under `start_paused`) and `wall`
(`SystemTime`, `chrono` now). Every column may only go down; `mono` is 0 in
meclaw-cells and meclaw-colony. Budget line: `<package> <unwrap+expect> <mono> <wall>`.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
BUDGET = Path(__file__).resolve().parent / "unwrap_budget.txt"
CLOCK_BANS = Path(__file__).resolve().parent / "clock-bans"
LINTS = ("clippy::unwrap_used", "clippy::expect_used")
BAN_LINT = "clippy::disallowed_methods"
COLUMNS = ("unwrap", "mono", "wall")
ADVICE = {
    "unwrap": "new unwrap()/expect() in library code. CONTRIBUTING: no unwrap "
              "outside tests. Return a Result, or say why this call site is "
              "infallible and re-pin deliberately.",
    "mono": "new std monotonic clock read in production. Read "
            "tokio::time::Instant::now() instead (TV-Z8): the same clock in "
            "production, virtual under start_paused.",
    "wall": "new wall-clock read in production. The column may only go down "
            "(VORGABEN § 5.4); re-pin only with the reason in the commit.",
}


def read_bans(conf: Path) -> dict[str, str]:
    """Ban path -> column (`mono`/`wall`), from the `reason` prefix."""
    doc = tomllib.loads(conf.read_text(encoding="utf-8"))
    bans: dict[str, str] = {}
    for entry in doc.get("disallowed-methods", []):
        column = entry["reason"].split(":", 1)[0].strip()
        if column not in ("mono", "wall"):
            raise SystemExit(
                f"unwrap budget: ban {entry['path']!r} has no `mono:`/`wall:` reason")
        bans[entry["path"]] = column
    return bans


def _ban_column(msg: dict, bans: dict[str, str]) -> str:
    """The column of one `disallowed_methods` hit: the reason note, else the path."""
    for child in msg.get("children") or []:
        text = child.get("message") or ""
        column = text.split(":", 1)[0].strip()
        if column in ("mono", "wall") and ":" in text:
            return column
    text = msg.get("message") or ""
    for path, column in bans.items():
        if f"`{path}`" in text:
            return column
    raise SystemExit(f"unwrap budget: a disallowed method outside the clock bans: {text!r}")


def _unresolved_ban(text: str) -> bool:
    """clippy's word for a ban path it cannot resolve (it then bans nothing)."""
    return ("does not refer to an existing" in text
            or "does not refer to a reachable" in text)


def measure(root: Path = ROOT, conf_dir: Path = CLOCK_BANS,
            env: dict[str, str] | None = None) -> dict[str, dict[str, int]]:
    """Run clippy over the default targets once and count the three columns."""
    bans = read_bans(conf_dir / "clippy.toml")
    run_env = dict(os.environ if env is None else env)
    run_env["CLIPPY_CONF_DIR"] = str(conf_dir)
    proc = subprocess.run(
        [
            "cargo", "clippy", "--workspace", "--message-format=json",
            "--", "-W", "clippy::unwrap_used", "-W", "clippy::expect_used",
            "-W", BAN_LINT,
        ],
        cwd=root,
        env=run_env,
        capture_output=True,
        text=True,
    )
    if proc.returncode != 0:
        sys.stderr.write(proc.stderr)
        raise SystemExit(f"unwrap budget: cargo clippy failed ({proc.returncode})")
    # A ban path clippy cannot resolve bans nothing -- say so instead of
    # counting zero (VORGABEN § 5.4).
    for line in proc.stderr.splitlines():
        if _unresolved_ban(line):
            raise SystemExit(f"unwrap budget: clock ban does not resolve: {line.strip()}")

    # package_id spellings differ by cargo version and by whether the directory
    # name matches the crate name ("...#meclaw-colony@0.7.0" vs "...colony#0.7.0").
    # Guessing at the string is how a gate starts counting a package called
    # "0.7.0"; ask cargo instead.
    by_id = workspace_members(root, run_env)

    counts = {name: dict.fromkeys(COLUMNS, 0) for name in by_id.values()}
    for line in proc.stdout.splitlines():
        if not line.startswith("{"):
            continue
        try:
            msg = json.loads(line)
        except json.JSONDecodeError:
            continue
        if msg.get("reason") != "compiler-message":
            continue
        message = msg.get("message") or {}
        if _unresolved_ban(message.get("message") or ""):
            raise SystemExit(f"unwrap budget: clock ban does not resolve: {message.get('message')}")
        lint = (message.get("code") or {}).get("code")
        if lint in LINTS:
            column = "unwrap"
        elif lint == BAN_LINT:
            column = _ban_column(message, bans)
        else:
            continue
        pkg_id = msg.get("package_id", "")
        name = by_id.get(pkg_id)
        if name is None:
            raise SystemExit(
                f"unwrap budget: diagnostic from an unknown package id "
                f"{pkg_id!r} -- the workspace moved under the gate"
            )
        counts[name][column] += 1

    return counts


def workspace_members(root: Path = ROOT, env: dict[str, str] | None = None) -> dict[str, str]:
    """Map package id -> package name for the workspace's own crates."""
    proc = subprocess.run(
        ["cargo", "metadata", "--no-deps", "--format-version", "1"],
        cwd=root, env=env, capture_output=True, text=True, check=True,
    )
    meta = json.loads(proc.stdout)
    return {p["id"]: p["name"] for p in meta["packages"]}


def read_budget(path: Path = BUDGET) -> dict[str, dict[str, int]]:
    pinned: dict[str, dict[str, int]] = {}
    for raw in path.read_text(encoding="utf-8").splitlines():
        line = raw.split("#", 1)[0].strip()
        if not line:
            continue
        name, *numbers = line.split()
        if len(numbers) != len(COLUMNS):
            raise SystemExit(
                f"unwrap budget: {name}: expected <package> <unwrap+expect> <mono> "
                f"<wall>, got {line!r}")
        pinned[name] = dict(zip(COLUMNS, (int(n) for n in numbers)))
    return pinned


def write_budget(counts: dict[str, dict[str, int]]) -> None:
    lines = [
        "# The unwrap/expect and clock ceilings per workspace package (GH #115, tv-p1b).",
        "# Format: <package> <unwrap+expect> <mono> <wall>",
        "#   unwrap+expect  clippy::unwrap_used + clippy::expect_used hits",
        "#   mono / wall    clippy::disallowed_methods hits of the clock bans in",
        "#                  .github/gates/clock-bans/clippy.toml (by reason prefix)",
        "# Measured by .github/gates/unwrap_budget.py over the default targets",
        "# (lib + bins) -- test modules and tests/ are out of scope by design.",
        "# A number may only ever go DOWN. Re-pin with `unwrap_budget.py --write`",
        "# and say in the commit why the number moved.",
        "",
    ]
    lines += [f"{name} " + " ".join(str(counts[name][c]) for c in COLUMNS)
              for name in sorted(counts)]
    BUDGET.write_text("\n".join(lines) + "\n", encoding="utf-8")


def check(counts: dict[str, dict[str, int]],
          pinned: dict[str, dict[str, int]]) -> tuple[list[str], list[str]]:
    """Failures (a column grew, the package set moved) and notes (below a pin)."""
    failures: list[str] = []
    notes: list[str] = []
    for name in sorted(set(pinned) | set(counts)):
        have = counts.get(name)
        want = pinned.get(name)
        if want is None:
            failures.append(
                f"{name}: {have} hits, not in the budget file -- a new package "
                f"enters the workspace with its ceiling written down, not by "
                f"default. Run `.github/gates/unwrap_budget.py --write`."
            )
            continue
        if have is None:
            failures.append(
                f"{name}: pinned at {want} but no longer in the workspace -- "
                f"drop the line."
            )
            continue
        for column in COLUMNS:
            h, w = have[column], want[column]
            if h > w:
                failures.append(
                    f"{name} {column}: {h} hits, ceiling is {w} -- {h - w} "
                    f"{ADVICE[column]}"
                )
            elif h < w:
                notes.append(
                    f"::warning::unwrap budget: {name} {column} is down to {h} (pin "
                    f"says {w}) -- lower the pin with "
                    f"`.github/gates/unwrap_budget.py --write`"
                )
        notes.append(f"  {name}: " + " ".join(
            f"{c} {have[c]}/{want[c]}" for c in COLUMNS))
    return failures, notes


def main() -> int:
    counts = measure()

    if "--write" in sys.argv:
        write_budget(counts)
        print(f"unwrap budget: re-pinned {BUDGET.relative_to(ROOT)}")
        for name in sorted(counts):
            print(f"  {name} " + " ".join(str(counts[name][c]) for c in COLUMNS))
        return 0

    failures, notes = check(counts, read_budget())
    for note in notes:
        print(note)

    if failures:
        print("unwrap budget: FAILED", file=sys.stderr)
        for f in failures:
            print(f"  {f}", file=sys.stderr)
        return 1

    print("unwrap budget: OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
