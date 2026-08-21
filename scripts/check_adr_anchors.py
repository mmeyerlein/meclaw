#!/usr/bin/env python3
"""Gate for ADR anchors (`plans/adr/`).

Binding contract: `docs/development-rules.md` § 2b, seeded out of the
2026-08-20 consistency audit (GH #319).

An accepted ADR with nothing in the tree holding it up is a wish. Legacy ADR
0011 is the case that made the rule: a built, tested, accepted capability
vanished, the ADR kept reading `Status: Akzeptiert` for three months, and the
spec kept promising the capability -- because nothing connected the decision to
a line of code that could disappear.

So: every ADR whose status is `Accepted`/`Akzeptiert` carries a `Pinned-by:`
line naming at least one target that must exist. This script resolves every
target and exits 1 naming the ADR when one does not.

ANCHOR GRAMMAR
==============
Targets are backtick-quoted on the `Pinned-by:` line (which may wrap over
several lines). Three forms, and nothing else is accepted:

    `test:<fn_name>`          a `#[test]` / `#[tokio::test]`-annotated `fn` of
                              that name under `crates/` -- the same tightened
                              indexing `scripts/check_claims.py` uses, so
                              `#[cfg(test)]` and the `test-hooks` feature gates
                              do NOT count as tests.
    `code:<literal>`          the literal occurs in some `*.rs` under `crates/`.
    `file:<path>`             the path exists, relative to the repo root.
    `file:<path>#<literal>`   ... and contains that literal.

THE 0002 EXCEPTION
==================
`plans/adr/0002-*` carries an uncommitted working-tree edit belonging to the
spec owner, so this task was not allowed to touch the file. Skipping it would
make the gate vacuous for exactly the ADR that is hardest to pin, so instead
its anchor lives one level out, in `plans/adr/README.md`, and is declared in
EXTERNAL_ANCHORS below. The anchor is still resolved like every other; only its
home differs. Three guards keep the exception honest:

  * the README must literally carry the declared `Pinned-by:` line, so the
    constant here and the index cannot drift apart;
  * the ADR file must NOT carry a `Pinned-by:` line of its own -- the day the
    owner's edit lands and the line moves into the file, this gate goes red and
    says so, which is how the exception gets removed instead of forgotten;
  * the entry pins the sha256 of the ADR's COMMITTED content. An exception that
    expires only when a specific line appears is not an expiry: a content-only
    edit would leave it green for good. Any landed change to the file trips the
    gate instead, and whoever lands it must either move the anchor into the file
    or renew the exception knowingly.

WHY THE **COMMITTED** BLOB AND NOT THE FILE ON DISK
---------------------------------------------------
`git show HEAD:<path>` is what is hashed, not the working copy. The working copy
right now carries the owner's uncommitted edit, which this gate must not react
to -- hashing it would mean the gate went red the moment somebody opened an
editor, and stayed red until they were finished. Hashing what is committed makes
the trigger the *landing* of a change, which is the event the exception is about.
The cost is stated plainly: this one check needs `git` and a real repository, so
it runs only in the private tree (the published tree has no `plans/adr/` and no
EXTERNAL_ANCHORS to check). Where it cannot run, it fails loudly rather than
skipping -- a silently skipped expiry is the defect this guard exists to prevent.

TWO TREE SHAPES
===============
Like the spec-claims gate next door, this runs in both, and the corpus is not
in both.

* PRIVATE tree -- `plans/adr/` exists. The ADRs themselves are read: status,
  `Pinned-by:` presence, target resolution. The derived registry
  `.github/gates/adr-anchors.tsv` is regenerated in memory and compared BYTE
  for byte against the committed copy, so a new or edited ADR that never
  reached the travelling copy is red here.
* PUBLIC tree -- `plans/` never travels (export rule R2), so there are no ADR
  files to read. The travelling registry is what is read, and every target in
  it is resolved against `crates/` (which does travel) and the repo root. That
  is the load-bearing half: the failure this gate exists to catch is a test or
  a symbol being deleted, and that deletion is visible in the public tree.

Detection is a property of the tree, not a flag: `plans/adr/` is a directory,
or it is not.

Exit 0 when every accepted ADR carries a resolvable anchor; exit 1 naming each
ADR that does not.

Usage:  python3 scripts/check_adr_anchors.py [--check] [--write]
        `--check` is the default. `--write` regenerates
        `.github/gates/adr-anchors.tsv` from `plans/adr/` (private tree only)
        and is the sanctioned way to move both copies in one commit.
"""

from __future__ import annotations

import argparse
import hashlib
import re
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
ADR_DIR = REPO_ROOT / "plans" / "adr"
ADR_INDEX = ADR_DIR / "README.md"
# The travelling registry. Same construction and same reason as
# `.github/gates/claims.tsv`: the canonical source is under `plans/`, which
# never travels, and a gate whose reference does not travel is not a gate.
PUBLIC_REGISTRY = REPO_ROOT / ".github" / "gates" / "adr-anchors.tsv"
CRATES_DIR = REPO_ROOT / "crates"

COLUMNS = ("adr", "status", "anchor_home", "target")

# ADR file -> (targets, sha256 of the COMMITTED file, why). The hash is what
# makes the exception expire: it is taken over `git show HEAD:<path>`, so ANY
# landed change to the ADR trips the gate. See "THE 0002 EXCEPTION" above.
EXTERNAL_ANCHORS: dict[str, tuple[tuple[str, ...], str, str]] = {
    "0002-kanal-talky-generation-und-das-publikum-einer-erinnerung.md": (
        (
            "test:the_brief_answers_a_disclosed_audience_and_a_stranger_gets_nothing",
            "test:the_audience_set_spelling_is_a_round_in_both_directions",
        ),
        "cf838e3e506ef5b175b7f7f119ff016cd5acdf56b1544c1f2a9d57216532d862",
        "the spec owner's edit landed as bc270f4d (Nachtrag 2026-08-20, E7 "
        "resolved) but the ADR still carries no Pinned-by line of its own; "
        "its anchor is carried by plans/adr/README.md (GH #303). Exception "
        "renewed 2026-08-21 after re-checking both anchored tests still "
        "exist and still describe the audience decision",
    ),
}

STATUS_RE = re.compile(r"^\s*[-*]?\s*\**Status:?\**", re.IGNORECASE)
PINNED_RE = re.compile(r"^\s*[-*]?\s*\**Pinned-by:?\**", re.IGNORECASE)
BACKTICKED_RE = re.compile(r"`([^`]+)`")
SUPERSEDED_WORDS = ("superseded", "abgelöst", "abgeloest", "revoked", "widerrufen")
ACCEPTED_WORDS = ("accepted", "akzeptiert", "angenommen")

# A `fn` definition line, with or without `pub`/`async`/`const`/`unsafe`.
FN_RE = re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:const\s+|async\s+|unsafe\s+)*fn\s+(\w+)")
# EXACTLY the attributes that make a function a test -- the same tightened set
# `check_claims.py` measured over this tree. `#[cfg(test)]` annotates a module,
# not a test, and must not count.
TEST_ATTR_RE = re.compile(r"^\s*#\[(?:test|tokio::test)\s*(?:\([^\]]*\))?\s*\]\s*$")
ATTR_RE = re.compile(r"^\s*#\[")
IGNORABLE_RE = re.compile(r"^\s*(//|/\*|\*|$)")


class Failure(Exception):
    pass


def rel(path: Path) -> str:
    try:
        return str(path.relative_to(REPO_ROOT))
    except ValueError:
        return str(path)


# --------------------------------------------------------------------------
# Resolution
# --------------------------------------------------------------------------


class Tree:
    """The searchable half of the tree, read once."""

    def __init__(self) -> None:
        self.test_fns: set[str] = set()
        self.sources: list[str] = []
        if not CRATES_DIR.is_dir():
            return
        for src in sorted(CRATES_DIR.rglob("*.rs")):
            text = src.read_text(encoding="utf-8", errors="replace")
            self.sources.append(text)
            pending = False
            for line in text.splitlines():
                fn = FN_RE.match(line)
                if fn:
                    if pending:
                        self.test_fns.add(fn.group(1))
                    pending = False
                    continue
                if ATTR_RE.match(line):
                    if TEST_ATTR_RE.match(line):
                        pending = True
                    continue
                if IGNORABLE_RE.match(line):
                    continue
                pending = False

    def has_literal(self, needle: str) -> bool:
        return any(needle in text for text in self.sources)


def resolve(target: str, tree: Tree) -> str | None:
    """None when the target resolves; otherwise the reason it does not."""
    kind, sep, rest = target.partition(":")
    if not sep or kind not in ("test", "code", "file"):
        return (
            f"malformed anchor {target!r} -- expected `test:<fn>`, `code:<literal>` "
            "or `file:<path>[#<literal>]`"
        )
    rest = rest.strip()
    if not rest:
        return f"empty anchor target in {target!r}"

    if kind == "test":
        if rest in tree.test_fns:
            return None
        return (
            f"no #[test]/#[tokio::test]-annotated fn named {rest!r} under crates/ "
            "(the decision has lost the test that embodied it)"
        )

    if kind == "code":
        if tree.has_literal(rest):
            return None
        return f"the literal {rest!r} occurs in no *.rs under crates/"

    path_part, _, literal = rest.partition("#")
    path = REPO_ROOT / path_part
    if not path.is_file():
        return f"file {path_part!r} does not exist"
    if literal and literal not in path.read_text(encoding="utf-8", errors="replace"):
        return f"file {path_part!r} no longer contains {literal!r}"
    return None


# --------------------------------------------------------------------------
# The private tree: read the ADRs
# --------------------------------------------------------------------------


def parse_adr(path: Path) -> tuple[str, tuple[str, ...], bool]:
    """(status word, anchor targets, whether the file carries a Pinned-by line)."""
    lines = path.read_text(encoding="utf-8").splitlines()
    status = ""
    targets: list[str] = []
    has_pinned = False

    for i, line in enumerate(lines):
        if not status and STATUS_RE.match(line):
            low = line.lower()
            if any(w in low for w in SUPERSEDED_WORDS):
                status = "superseded"
            elif any(w in low for w in ACCEPTED_WORDS):
                status = "accepted"
            else:
                status = "unknown"
            continue
        if PINNED_RE.match(line):
            has_pinned = True
            # The line may wrap: keep consuming while the continuation is
            # indented and is not the next `- **Field:**` bullet.
            block = [line]
            for cont in lines[i + 1 :]:
                if not cont.strip():
                    break
                if re.match(r"^\s*[-*]\s", cont) or cont.startswith("#"):
                    break
                block.append(cont)
            targets = BACKTICKED_RE.findall("\n".join(block))
    return status or "unknown", tuple(targets), has_pinned


def committed_sha256(relpath: str) -> str:
    """sha256 of `git show HEAD:<relpath>`, i.e. of the file as COMMITTED.

    Deliberately not the working copy -- see "WHY THE COMMITTED BLOB" above.
    Raises `Failure` when it cannot be determined, because a skipped expiry
    check is worth less than no expiry check: it looks like one.
    """
    try:
        out = subprocess.run(
            ["git", "show", f"HEAD:{relpath}"],
            cwd=REPO_ROOT,
            capture_output=True,
            check=False,
        )
    except FileNotFoundError as e:
        raise Failure(
            f"cannot hash the committed {relpath}: git is not available ({e}). "
            "The EXTERNAL_ANCHORS expiry check needs a real repository; it runs "
            "only in the private tree, and it does not skip."
        ) from e
    if out.returncode != 0:
        raise Failure(
            f"cannot read {relpath} at HEAD: "
            f"{out.stderr.decode('utf-8', 'replace').strip() or 'git show failed'}. "
            "The EXTERNAL_ANCHORS expiry check hashes the COMMITTED file; an ADR "
            "listed there must be committed."
        )
    return hashlib.sha256(out.stdout).hexdigest()


def derive_rows() -> list[tuple[str, str, str, str]]:
    """The registry, derived from `plans/adr/`. Deterministic order."""
    rows: list[tuple[str, str, str, str]] = []
    problems: list[str] = []

    adrs = sorted(p for p in ADR_DIR.glob("*.md") if p.name != "README.md")
    if not adrs:
        raise Failure(f"{rel(ADR_DIR)}: no ADR files -- an empty gate is not a gate")

    index_text = ADR_INDEX.read_text(encoding="utf-8") if ADR_INDEX.is_file() else ""

    for adr in adrs:
        status, targets, has_pinned = parse_adr(adr)
        if status != "accepted":
            continue

        external = EXTERNAL_ANCHORS.get(adr.name)
        if external is not None:
            ext_targets, expected_sha, reason = external
            actual_sha = committed_sha256(adr.relative_to(REPO_ROOT).as_posix())
            if actual_sha != expected_sha:
                problems.append(
                    f"{rel(adr)}: it has changed since its anchor exception was "
                    f"granted (committed sha256 {actual_sha}, "
                    f"expected {expected_sha}).\n"
                    f"      The exception exists because: {reason}.\n"
                    "      Whoever landed that change decides which it is: move the "
                    "`Pinned-by:` line INTO the ADR and delete the EXTERNAL_ANCHORS "
                    "entry, or renew the exception knowingly by updating its hash "
                    "(`git show HEAD:<path> | sha256sum`) after re-checking that the "
                    "anchor still describes the decision."
                )
                continue
            if has_pinned:
                problems.append(
                    f"{rel(adr)}: carries its own Pinned-by line while it is still "
                    f"listed in EXTERNAL_ANCHORS of {rel(Path(__file__))} ({reason}). "
                    "The exception has served its purpose -- delete the entry and let "
                    "the file's own anchor be the anchor."
                )
                continue
            for t in ext_targets:
                if f"`{t}`" not in index_text:
                    problems.append(
                        f"{rel(adr)}: its external anchor `{t}` is declared in "
                        f"{rel(Path(__file__))} but is not carried by {rel(ADR_INDEX)}. "
                        "The two must agree -- the index is where a reader looks."
                    )
                rows.append((adr.name, "accepted", "README", t))
            continue

        if not has_pinned:
            problems.append(
                f"{rel(adr)}: status is Accepted but there is no `Pinned-by:` line. "
                "docs/development-rules.md § 2b: an accepted ADR names at least one "
                "test or code symbol that embodies the decision."
            )
            continue
        if not targets:
            problems.append(
                f"{rel(adr)}: the `Pinned-by:` line names no backtick-quoted target."
            )
            continue
        for t in targets:
            rows.append((adr.name, "accepted", "self", t))

    if problems:
        raise Failure("\n".join(problems))
    if not rows:
        raise Failure("no accepted ADR carries an anchor -- an empty gate is not a gate")
    return rows


def render(rows: list[tuple[str, str, str, str]]) -> str:
    head = (
        "# Derived from plans/adr/ by scripts/check_adr_anchors.py --write.\n"
        "# plans/ never travels with an export (rule R2), so this copy is what the\n"
        "# public `gates` job reads. Do not hand-edit: regenerate it in the same\n"
        "# commit as the ADR change. The private tree fails if the two diverge.\n"
    )
    body = "\t".join(COLUMNS) + "\n"
    body += "".join("\t".join(r) + "\n" for r in rows)
    return head + body


def load_registry() -> list[tuple[str, str, str, str]]:
    if not PUBLIC_REGISTRY.is_file():
        raise Failure(f"registry not found: {rel(PUBLIC_REGISTRY)}")
    rows: list[tuple[str, str, str, str]] = []
    header_seen = False
    for lineno, raw in enumerate(
        PUBLIC_REGISTRY.read_text(encoding="utf-8").splitlines(), 1
    ):
        if not raw.strip() or raw.lstrip().startswith("#"):
            continue
        fields = tuple(f.strip() for f in raw.split("\t"))
        if not header_seen:
            if fields != COLUMNS:
                raise Failure(
                    f"{rel(PUBLIC_REGISTRY)}:{lineno}: header must be exactly "
                    + "<TAB>".join(COLUMNS)
                )
            header_seen = True
            continue
        if len(fields) != len(COLUMNS):
            raise Failure(
                f"{rel(PUBLIC_REGISTRY)}:{lineno}: expected {len(COLUMNS)} "
                f"tab-separated columns, found {len(fields)}"
            )
        rows.append(fields)  # type: ignore[arg-type]
    if not header_seen:
        raise Failure(f"{rel(PUBLIC_REGISTRY)}: no header row found")
    if not rows:
        raise Failure(f"{rel(PUBLIC_REGISTRY)}: no rows -- an empty gate is not a gate")
    return rows


# --------------------------------------------------------------------------


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--check", action="store_true", help="check (the default)")
    ap.add_argument(
        "--write",
        action="store_true",
        help="regenerate .github/gates/adr-anchors.tsv from plans/adr/",
    )
    args = ap.parse_args()

    private = ADR_DIR.is_dir()

    try:
        if args.write:
            if not private:
                raise Failure(
                    "--write needs plans/adr/, which only the private tree has"
                )
            text = render(derive_rows())
            PUBLIC_REGISTRY.parent.mkdir(parents=True, exist_ok=True)
            PUBLIC_REGISTRY.write_text(text, encoding="utf-8")
            print(f"wrote {rel(PUBLIC_REGISTRY)}")
            return 0

        tree = Tree()
        failures: list[str] = []

        if private:
            rows = derive_rows()
            derived = render(rows)
            if not PUBLIC_REGISTRY.is_file():
                raise Failure(
                    f"{rel(PUBLIC_REGISTRY)} is missing. It is what CI reads in the "
                    "published tree, where plans/ does not exist. Fix: python3 "
                    "scripts/check_adr_anchors.py --write"
                )
            committed = PUBLIC_REGISTRY.read_text(encoding="utf-8")
            if committed != derived:
                failures.append(
                    f"{rel(PUBLIC_REGISTRY)} drifted from {rel(ADR_DIR)}. An ADR "
                    "edit moves BOTH in the same commit -- the travelling copy is "
                    "what the public gate measures against. Fix: python3 "
                    "scripts/check_adr_anchors.py --write"
                )
            source = f"{rel(ADR_DIR)} (private tree)"
        else:
            rows = load_registry()
            source = f"{rel(PUBLIC_REGISTRY)} (published tree)"

        for adr, _status, home, target in rows:
            why = resolve(target, tree)
            if why is not None:
                where = "" if home == "self" else f" [anchor carried by {home}]"
                failures.append(f"ADR {adr}{where}: {why}")

        if failures:
            print("ADR anchor gate FAILED:\n", file=sys.stderr)
            for f in failures:
                print(f"  - {f}", file=sys.stderr)
            print(
                "\ndocs/development-rules.md § 2b: removing the code that embodies an "
                "accepted decision takes flipping the ADR to Superseded -- with a "
                "pointer to the superseding ADR or issue -- in the same commit.",
                file=sys.stderr,
            )
            return 1

        adr_count = len({r[0] for r in rows})
        print(
            f"ADR anchor gate OK: {adr_count} accepted ADR(s), "
            f"{len(rows)} anchor(s) resolved, source {source}"
        )
        return 0

    except Failure as e:
        print(f"ADR anchor gate FAILED:\n\n  {e}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
