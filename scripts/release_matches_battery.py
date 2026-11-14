#!/usr/bin/env python3
"""Refuse a release commit that differs from the measured battery candidate in
anything but version and CHANGELOG places (wave live-2, OR-LV-12, GH #1358), or
whose `templates` tree is not the tree the battery measured (G8a, GH #1390).

Why: the battery of real model tests runs against the frozen master candidate
before the release. A release commit that changes anything else ships code
nobody measured (retro 2026-10-29, item 2). This script is the lock the release
step runs before it tags.

Receipt form (Markdown, path as the first argument). The script reads three
kinds of line, plus the templates tree; each may stand as a plain line or as a list item, and the value
may be wrapped in backticks:

    core_commit: <sha>                       the measured meclaw-core commit
    lab_binary_sha256: <hex>                 sha256 of the lab binary
    manifest_sha256: <hex> <rendered path>   one per rendered manifest
    templates_tree: <sha>                    `git rev-parse <rev>:templates` measured

A receipt is append-only: round 2 of a battery appends a new candidate below
round 1. The LAST `core_commit` line is the candidate, and the hash lines after
it are its own. The binary and manifest hashes are only read and printed for
the release receipt; this script never computes them (the lab binary and the
rendered set are not in this tree). `templates_tree` is mandatory for the last
candidate: template content changed without a version bump is invisible to the
manifests (G8; live-2 compared the tree hash by hand), so the release's
`templates` tree must equal it, or the line `templates tree differs (<release>
!= <measured>)` fails the check. The battery therefore measures the release cut
itself; a template version bump after the battery changes the tree too.

Check: `git diff <core_commit> <release>` (release defaults to HEAD), file by
file, renames split into delete + add, judged by THE release-record rule
`release_offending` in `scripts/pipeline/turnaround.py` (RT2, GH #1385; the
export reads the same function). In short: `CHANGELOG.md`, `docs/status.en.md`
and a wave's `plans/<wave>/{berichte,straenge,receipts}/**` (added or modified,
100644, not named by a script or test) may change; the version places
(`Cargo.toml`, `crates/*/Cargo.toml`, `Cargo.lock`, `templates/**/template.json`)
only in their version strings; `ROADMAP.md` only when lines are added under
`## Shipped` and nothing else (the release's own line, GH #1518). Any other
`ROADMAP.md` change and every other `plans/**` path are foreign (review C1).
Every foreign file is one line `foreign: <path> (<reason>)`.

Exit: 0 clean, 1 foreign files or a differing templates tree, 2 unusable
receipt (no `core_commit`, no `templates_tree`) or unknown commit.

Usage: release_matches_battery.py <receipt.md> [--release <rev>] [--repo <path>]
"""
import argparse
import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent

FIELD = re.compile(
    r"^\s*(?:[-*]\s+)?`?(core_commit|lab_binary_sha256|manifest_sha256|templates_tree)`?\s*:\s*(.+?)\s*$")
SHA = re.compile(r"[0-9a-f]{7,40}")

# The release record rule lives ONCE in `scripts/pipeline/turnaround.py` (RT2,
# GH #1385): this lock calls `turnaround.release_offending`, the export
# (`make_export --gate-receipt`) calls it through `release_record_equal`.
sys.path.insert(0, str(ROOT / "scripts"))
from pipeline import turnaround  # noqa: E402


class ReceiptError(Exception):
    pass


def read_receipt(path):
    """(core_commit, templates_tree, [pass-through lines]) of the last
    candidate in `path`."""
    try:
        lines = pathlib.Path(path).read_text().splitlines()
    except OSError as exc:
        raise ReceiptError("receipt not readable: %s" % exc)
    core, tree, fields = None, None, []
    for line in lines:
        m = FIELD.match(line)
        if not m:
            continue
        key, value = m.group(1), m.group(2).replace("`", "").strip()
        if key == "core_commit":
            if not SHA.fullmatch(value):
                raise ReceiptError("core_commit is not a commit sha: %r" % value)
            core, tree, fields = value, None, []
        elif key == "templates_tree" and core is not None:
            if not SHA.fullmatch(value):
                raise ReceiptError("templates_tree is not a tree sha: %r" % value)
            tree = value
        elif core is not None:
            fields.append("%s: %s" % (key, value))
    if core is None:
        raise ReceiptError("receipt carries no core_commit line: %s" % path)
    if tree is None:
        raise ReceiptError("receipt without templates_tree: %s" % path)
    return core, tree, fields


def git(repo, *args, check=True):
    res = subprocess.run(["git", "-C", str(repo), *args], capture_output=True)
    if check and res.returncode != 0:
        raise ReceiptError("git %s: %s" % (" ".join(args), res.stderr.decode().strip()))
    return res


def commit_of(repo, rev):
    res = git(repo, "rev-parse", "--verify", "--quiet", rev + "^{commit}", check=False)
    if res.returncode != 0:
        raise ReceiptError("unknown commit: %s" % rev)
    return res.stdout.decode().strip()


def templates_tree_of(repo, commit):
    res = git(repo, "rev-parse", "--verify", "--quiet", commit + ":templates", check=False)
    if res.returncode != 0:
        raise ReceiptError("release has no templates tree: %s" % commit)
    return res.stdout.decode().strip()


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("receipt", help="battery receipt (Markdown)")
    ap.add_argument("--release", default="HEAD", help="release commit (default HEAD)")
    ap.add_argument("--repo", default=str(ROOT), help="meclaw-core tree (default: this one)")
    args = ap.parse_args(argv)
    try:
        core_ref, measured, fields = read_receipt(args.receipt)
        core = commit_of(args.repo, core_ref)
        release = commit_of(args.repo, args.release)
        released = templates_tree_of(args.repo, release)
        try:
            found = sorted(turnaround.release_offending(args.repo, core, release).items())
        except ValueError as exc:
            raise ReceiptError(str(exc))
    except ReceiptError as exc:
        print("release_matches_battery: ERROR %s" % exc)
        return 2
    print("core_commit: %s" % core)
    print("release: %s" % release)
    for line in fields:
        print(line)
    print("templates_tree: %s" % measured)
    for path, reason in found:
        print("foreign: %s (%s)" % (path, reason))
    # A short sha in the receipt names the same tree as the full one.
    differs = not released.startswith(measured)
    if differs:
        print("templates tree differs (%s != %s)" % (released, measured))
    if found:
        print("release_matches_battery: FOREIGN %d file(s) changed after the battery" % len(found))
    if found or differs:
        return 1
    print("release_matches_battery: CLEAN")
    return 0


if __name__ == "__main__":
    sys.exit(main())
