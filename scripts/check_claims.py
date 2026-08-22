#!/usr/bin/env python3
"""Gate for the spec-claims registry (`plans/spec-claims/claims.tsv`).

Binding contract: `docs/development-rules.md` § 2a, seeded out of the
2026-08-20 consistency audit (GH #254).

A documented behaviour with no test pinning it is a wish, not a contract, and a
spec that runs ahead of the code passes every review by construction -- a review
compares code against spec. This script is the other direction: it compares the
spec's own text against the registry that classifies it.

Three checks, one per class, plus one that applies to every row:

* every row -- the `anchor` fragment must occur LITERALLY in the named document.
  A paragraph cannot be rewritten or deleted out from under its registry row.
* `pinned` -- `test_or_issue` names a test function; a test function of that
  name must exist in the crate tree (a `#[test]`/`#[tokio::test]`-annotated
  `fn`, in `crates/**/tests/**` or in an inline `#[cfg(test)]` module).
* `specified-not-built` -- the anchor's own line must carry the visible marker,
  and the marker's issue number must be the one the row names. The marker
  exists in two spellings, one per language:

      *(specified, not built -- see GH #N)*
      *(spezifiziert, nicht gebaut -- siehe GH #N)*

  Both are accepted: the rules document quotes the English spelling, but a
  German document carries the German one, and a marker a reader cannot read is
  not a marker. Such a row is furthermore PAIRED: its id ends in `-de` or
  `-en`, and the twin id must exist in the registry and point at the twin
  document (`X.md` <-> `X.en.md`). That is how "in BOTH language versions" is
  enforced when the two languages cannot share one anchor string.
* `built-unpinned` -- the anchor check only. These are the rows waiting for a
  test; `test_or_issue` carries the issue that would produce it, or `-`.

TWO TREE SHAPES
===============
This script runs in both, and `docs/` does not look the same in them.

* PRIVATE tree -- paired documents side by side: `docs/X.md` (German) and
  `docs/X.en.md` (English). Every row is checked, both twins of every
  retraction, plus the canonical-vs-public registry divergence check.
* PUBLIC tree -- the export publishes each `docs/X.en.md` UNDER THE PLAIN NAME
  `docs/X.md` (the export script's `DOCS_MAP`); no `.en.md` file and no German
  document travels. So a row naming `docs/X.en.md` must be resolved to
  `docs/X.md` there, and a row naming a German original cannot be checked at
  all -- its document does not exist in that tree. Those rows are SKIPPED and
  COUNTED, never silently dropped; a skipped `pinned` row still has its test
  checked, because `crates/` does travel.

Detection is a property of the tree, not a flag: a tree is public-shaped when
`docs/` exists and contains no `*.en.md` file at all. That is exactly what
`DOCS_MAP` produces and it needs no list to be kept in step. To keep the
detection from ever weakening the private gate by accident, a public-shaped
tree that still carries `plans/spec-claims/claims.tsv` is a hard failure: that
combination means a private tree lost its English twins, which is a defect, not
a publication.

A German original is recognised without a hard-coded list too: `docs/X.md` is
one when some row of the registry names `docs/X.en.md`. A document that ships
under its own name in both trees is therefore checked in both.

Exit 0 when every row holds, exit 1 naming each row that does not.

Usage:  python3 scripts/check_claims.py [--check] [--registry PATH]
        (`--check` is accepted for symmetry with the other gates and is the
        default behaviour; the script never writes anything.)
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
# The canonical registry, named by docs/development-rules.md § 2a.
CANONICAL_REGISTRY = REPO_ROOT / "plans" / "spec-claims" / "claims.tsv"
# Its public twin. `plans/` never travels with an export (the export script's
# rule R2), and CI runs in the PUBLIC tree -- a gate whose reference file does
# not travel is not a gate, it is a red workflow. Same treatment the corridor
# byte gates already get: the copy under `.github/` is what CI reads, and the
# two exemplars are held together by the drift check below.
PUBLIC_REGISTRY = REPO_ROOT / ".github" / "gates" / "claims.tsv"
CRATES_DIR = REPO_ROOT / "crates"

COLUMNS = ("id", "doc", "anchor", "class", "test_or_issue")
CLASSES = ("pinned", "built-unpinned", "specified-not-built")

# The two spellings of the retraction marker. Written with an explicit
# alternation over the dash so that a hyphen-minus in a hand-edited document is
# not silently accepted as an em dash -- and vice versa.
MARKER_RE = re.compile(
    r"\*\((?:specified, not built|spezifiziert, nicht gebaut)"
    r"\s*[—-]\s*"
    r"(?:see|siehe)\s+GH\s+#(\d+)\)\*"
)

ISSUE_RE = re.compile(r"^GH #(\d+)$")
TEST_NAME_RE = re.compile(r"^[a-z_][a-z0-9_]*$")

# Shortest anchor accepted. An anchor short enough to occur by accident pins
# nothing -- a one-character anchor passes against any document. The floor is
# measured, not guessed: the shortest anchor in the seeded registry is 22
# characters ("**Edge UUIDs visible**"), so 16 rejects the vacuous case with
# room to spare for a genuinely terse future row.
MIN_ANCHOR_LEN = 16

# A `fn` definition line, with or without `pub`/`async`/`const`/`unsafe`.
FN_RE = re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:const\s+|async\s+|unsafe\s+)*fn\s+(\w+)")
# EXACTLY the attributes that make a function a test. Measured over the tree:
# `#[test]` (2717), `#[tokio::test]` with or without a flavor argument (1344).
# A loose "contains the word test" match also swallowed `#[cfg(test)]`,
# `#[cfg(feature = "test-hooks")]`, `#[cfg(not(feature = "test-hooks"))]` and
# `#[cfg(all(test, feature = "test-hooks"))]`, which annotate modules and items
# that are not tests -- and a pinned row must name a REAL test.
TEST_ATTR_RE = re.compile(r"^\s*#\[(?:test|tokio::test)\s*(?:\([^\]]*\))?\s*\]\s*$")
ATTR_RE = re.compile(r"^\s*#\[")
IGNORABLE_RE = re.compile(r"^\s*(//|/\*|\*|$)")


class Failure(Exception):
    pass


def load_rows(registry: Path) -> list[dict[str, str]]:
    """Read the TSV. Comment lines (`#`) and blank lines are skipped."""
    if not registry.is_file():
        raise Failure(f"registry not found: {rel(registry)}")

    rows: list[dict[str, str]] = []
    header_seen = False
    seen_ids: dict[str, int] = {}

    for lineno, raw in enumerate(registry.read_text(encoding="utf-8").splitlines(), 1):
        if not raw.strip() or raw.lstrip().startswith("#"):
            continue
        fields = raw.split("\t")
        if not header_seen:
            if tuple(f.strip() for f in fields) != COLUMNS:
                raise Failure(
                    f"{rel(registry)}:{lineno}: header must be exactly "
                    + "<TAB>".join(COLUMNS)
                    + f", found: {fields!r}"
                )
            header_seen = True
            continue
        if len(fields) != len(COLUMNS):
            raise Failure(
                f"{rel(registry)}:{lineno}: expected {len(COLUMNS)} tab-separated "
                f"columns, found {len(fields)}"
            )
        row = dict(zip(COLUMNS, (f.strip() for f in fields)))
        row["_lineno"] = str(lineno)
        if row["id"] in seen_ids:
            raise Failure(
                f"{rel(registry)}:{lineno}: duplicate claim id {row['id']!r} "
                f"(first seen on line {seen_ids[row['id']]})"
            )
        seen_ids[row["id"]] = lineno
        rows.append(row)

    if not header_seen:
        raise Failure(f"{rel(registry)}: no header row found")
    if not rows:
        raise Failure(f"{rel(registry)}: no claim rows -- an empty gate is not a gate")
    return rows


def rel(path: Path) -> str:
    try:
        return str(path.relative_to(REPO_ROOT))
    except ValueError:
        return str(path)


def test_fns_in_text(text: str) -> set[str]:
    """Names of every `#[test]` / `#[tokio::test]`-annotated `fn` in ONE source text.

    Cut out of `collect_test_fns` so that the taxonomy AND the walk over it exist
    exactly once. The export gate (`plans/export-fixtures/make_export.py`, R13)
    asks the same question of a corpus that is not on disk -- the blobs of the
    export tree -- and a second copy of this loop would be a second answer that
    ages silently. Whoever changes what counts as a test changes it here.
    """
    names: set[str] = set()
    pending = False
    for line in text.splitlines():
        fn = FN_RE.match(line)
        if fn:
            if pending:
                names.add(fn.group(1))
            pending = False
            continue
        if ATTR_RE.match(line):
            if TEST_ATTR_RE.match(line):
                pending = True
            continue
        if IGNORABLE_RE.match(line):
            continue
        pending = False
    return names


def collect_test_fns() -> set[str]:
    """Names of every `#[test]` / `#[tokio::test]`-annotated `fn` under `crates/`.

    Covers integration tests in `crates/*/tests/` and inline `#[cfg(test)]`
    modules alike -- a pin is a pin wherever it lives. `#[cfg(test)]` and the
    `test-hooks` feature gates are NOT test attributes and do not count.
    """
    names: set[str] = set()
    if not CRATES_DIR.is_dir():
        return names
    for src in CRATES_DIR.rglob("*.rs"):
        names |= test_fns_in_text(src.read_text(encoding="utf-8", errors="replace"))
    return names


def tree_is_public() -> bool:
    """True when this checkout is an EXPORTED tree rather than the work tree.

    The observable property, and the one that actually matters here: the export
    publishes `docs/X.en.md` under the plain name `docs/X.md`, so a published
    tree carries no `*.en.md` at all. No list to keep in step, no flag to pass
    wrongly.
    """
    docs = REPO_ROOT / "docs"
    if not docs.is_dir():
        return False
    return not any(docs.glob("*.en.md"))


def resolve_doc(doc: str, public: bool, english_twins: set[str]) -> tuple[str | None, str]:
    """(path to read, reason) for one row's document in this tree shape.

    A `None` path means "not checkable here" and the reason says why -- that is
    a counted skip, never a silent pass.
    """
    if not public:
        return doc, ""
    if doc.endswith(".en.md"):
        # DOCS_MAP: the English source ships under the plain name.
        return doc[: -len(".en.md")] + ".md", "published as the plain name"
    if doc in english_twins:
        # A German original. It has an English twin in the work tree, and the
        # export ships that twin instead -- this file does not travel.
        return None, "German original; not published (its English twin ships as this name)"
    return doc, ""


def twin_of(doc: str) -> str | None:
    """`docs/x.md` <-> `docs/x.en.md`; None when the document is unpaired."""
    if doc.endswith(".en.md"):
        return doc[: -len(".en.md")] + ".md"
    if doc.endswith(".md"):
        return doc[: -len(".md")] + ".en.md"
    return None


def check_row(
    row: dict[str, str],
    doc_lines: dict[str, list[str]],
    test_fns: set[str],
    rows_by_id: dict[str, dict[str, str]],
    public: bool,
    english_twins: set[str],
) -> tuple[list[str], str]:
    """(problems, skip reason). A non-empty skip reason is reported and counted.

    A skip never covers the whole row: the shape checks and, for a `pinned`
    row, the test-existence check run in every tree, because `crates/` travels.
    Only the text checks -- anchor and marker -- need the document.
    """
    problems: list[str] = []
    rid, doc, anchor, klass, ref = (
        row["id"],
        row["doc"],
        row["anchor"],
        row["class"],
        row["test_or_issue"],
    )

    if klass not in CLASSES:
        problems.append(f"class must be one of {'/'.join(CLASSES)}, found {klass!r}")
        return problems, ""
    if not anchor:
        problems.append("anchor is empty -- a row without an anchor pins nothing")
        return problems, ""
    if len(anchor) < MIN_ANCHOR_LEN:
        problems.append(
            f"anchor is {len(anchor)} characters, below the {MIN_ANCHOR_LEN}-character "
            f"floor: {anchor!r}. An anchor that short matches by accident and pins nothing"
        )
        return problems, ""

    # --- checks that need no document ------------------------------------
    if klass == "pinned":
        if not TEST_NAME_RE.match(ref):
            problems.append(
                f"pinned row must name a test function in test_or_issue, found {ref!r}"
            )
        elif ref not in test_fns:
            problems.append(
                f"pinned test {ref!r} does not exist in the test corpus "
                "(no #[test]/#[tokio::test]-annotated fn of that name under crates/)"
            )

    if klass == "specified-not-built":
        problems.extend(check_twin(rid, doc, rows_by_id))
        if not ISSUE_RE.match(ref):
            problems.append(
                f"specified-not-built row must name its issue as 'GH #N', found {ref!r}"
            )

    # --- checks that need the document -----------------------------------
    read_path, reason = resolve_doc(doc, public, english_twins)
    if read_path is None:
        return problems, reason

    lines = doc_lines.get(read_path)
    if lines is None:
        problems.append(
            f"document not found: {read_path}"
            + (f" (row names {doc}, {reason})" if read_path != doc else "")
        )
        return problems, ""

    where = read_path if read_path == doc else f"{read_path} (published {doc})"
    hits = [i for i, line in enumerate(lines) if anchor in line]
    if not hits:
        if anchor in "\n".join(lines):
            problems.append(
                f"anchor spans a line break in {where} -- an anchor must sit on ONE line"
            )
        else:
            problems.append(
                f"anchor no longer occurs in {where}: {anchor!r} "
                "(the paragraph was rewritten out from under this row)"
            )
        return problems, ""

    if klass == "specified-not-built":
        issue = ISSUE_RE.match(ref)
        marked = [i for i in hits if MARKER_RE.search(lines[i])]
        if not marked:
            problems.append(
                f"{where}:{hits[0] + 1} carries no visible marker on the anchor line -- "
                "expected *(specified, not built — see GH #N)* or "
                "*(spezifiziert, nicht gebaut — siehe GH #N)*"
            )
        elif issue:
            found = {MARKER_RE.search(lines[i]).group(1) for i in marked}
            if issue.group(1) not in found:
                problems.append(
                    f"{where}:{marked[0] + 1} marker names GH #{sorted(found)[0]}, "
                    f"the row names GH #{issue.group(1)}"
                )

    return problems, ""


def check_twin(rid: str, doc: str, rows_by_id: dict[str, dict[str, str]]) -> list[str]:
    """"In BOTH language versions", enforced over the registry rather than the tree.

    The two languages cannot share one anchor string, so the pairing lives in
    twin rows (`-de` / `-en`). Checking it against the registry means it holds
    in the public tree too, where the German half of the pair is not present.
    """
    problems: list[str] = []
    if rid.endswith("-de"):
        twin_id = rid[:-3] + "-en"
    elif rid.endswith("-en"):
        twin_id = rid[:-3] + "-de"
    else:
        return [
            "specified-not-built id must end in '-de' or '-en' so its twin "
            "row is nameable (the marker is due in BOTH language versions)"
        ]

    twin_row = rows_by_id.get(twin_id)
    expected_doc = twin_of(doc)
    if twin_row is None:
        problems.append(
            f"twin row {twin_id!r} is missing -- the marker is due in BOTH "
            f"language versions ({expected_doc})"
        )
    elif twin_row["doc"] != expected_doc:
        problems.append(
            f"twin row {twin_id!r} points at {twin_row['doc']}, "
            f"expected the paired document {expected_doc}"
        )
    elif twin_row["class"] != "specified-not-built":
        problems.append(
            f"twin row {twin_id!r} is {twin_row['class']!r}; a retraction "
            "holds in both languages or in neither"
        )
    return problems


def resolve_registry() -> tuple[Path, str | None]:
    """The registry to read, plus a drift complaint when the two exemplars differ.

    Private tree: the canonical file under `plans/`. Public tree: only the copy
    under `.github/gates/` exists. Where both exist they must be byte-identical
    -- two copies that may disagree are worse than one, and a registry that
    disagrees with the one CI reads would gate nothing.
    """
    if CANONICAL_REGISTRY.is_file():
        if PUBLIC_REGISTRY.is_file():
            if PUBLIC_REGISTRY.read_bytes() != CANONICAL_REGISTRY.read_bytes():
                return CANONICAL_REGISTRY, (
                    f"{rel(PUBLIC_REGISTRY)} has drifted from "
                    f"{rel(CANONICAL_REGISTRY)} -- the copy CI reads must be "
                    f"byte-identical. Fix: cp {rel(CANONICAL_REGISTRY)} "
                    f"{rel(PUBLIC_REGISTRY)}"
                )
        else:
            return CANONICAL_REGISTRY, (
                f"{rel(PUBLIC_REGISTRY)} is missing -- CI runs in the public "
                f"tree, where `plans/` does not exist. Fix: cp "
                f"{rel(CANONICAL_REGISTRY)} {rel(PUBLIC_REGISTRY)}"
            )
        return CANONICAL_REGISTRY, None
    return PUBLIC_REGISTRY, None


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--check",
        action="store_true",
        help="verify the registry (default; the script never writes)",
    )
    parser.add_argument(
        "--registry",
        type=Path,
        default=None,
        help=(
            f"path to claims.tsv (default: {rel(CANONICAL_REGISTRY)}, "
            f"falling back to {rel(PUBLIC_REGISTRY)} in a public tree)"
        ),
    )
    args = parser.parse_args()

    public = tree_is_public()
    if public and CANONICAL_REGISTRY.is_file():
        # A tree with no `docs/*.en.md` but with `plans/` is not a publication:
        # it is a work tree that lost its English twins. Refusing here keeps the
        # detection from ever weakening the private gate by accident.
        print(
            f"FAIL docs/ carries no *.en.md while {rel(CANONICAL_REGISTRY)} exists -- "
            "this looks like a work tree that lost its English twins, not a "
            "published tree. Restore them; the gate will not run in reduced mode here.",
            file=sys.stderr,
        )
        return 1

    drift: str | None = None
    if args.registry is None:
        args.registry, drift = resolve_registry()

    try:
        rows = load_rows(args.registry)
    except Failure as exc:
        print(f"FAIL {exc}", file=sys.stderr)
        return 1

    english_twins = {
        twin for row in rows if (twin := twin_of(row["doc"])) and row["doc"].endswith(".en.md")
    }

    doc_lines: dict[str, list[str]] = {}
    for doc in sorted({row["doc"] for row in rows}):
        read_path, _ = resolve_doc(doc, public, english_twins)
        if read_path is None or read_path in doc_lines:
            continue
        path = REPO_ROOT / read_path
        if path.is_file():
            doc_lines[read_path] = path.read_text(encoding="utf-8").splitlines()

    test_fns = collect_test_fns()
    rows_by_id = {row["id"]: row for row in rows}

    failures: list[str] = []
    if drift:
        failures.append(drift)
    per_class = {klass: 0 for klass in CLASSES}
    skipped: list[tuple[str, str]] = []
    for row in rows:
        per_class[row["class"]] = per_class.get(row["class"], 0) + 1
        problems, skip_reason = check_row(
            row, doc_lines, test_fns, rows_by_id, public, english_twins
        )
        if skip_reason:
            skipped.append((row["id"], skip_reason))
        for problem in problems:
            failures.append(
                f"{rel(args.registry)}:{row['_lineno']}: [{row['id']}] {problem}"
            )

    shape = "public" if public else "private"
    counts = ", ".join(f"{k}={per_class.get(k, 0)}" for k in CLASSES)
    tail = (
        f"{len(rows)} row(s) ({counts}), {len(rows) - len(skipped)} text-checked, "
        f"{len(skipped)} skipped; {len(test_fns)} test fns indexed; {shape} tree."
    )

    if skipped:
        # Counted, named, never silent. An empty result and a forgotten check
        # must not look alike.
        print(f"note: {len(skipped)} row(s) not text-checked in this tree shape:")
        for rid, reason in skipped:
            print(f"  skip [{rid}] {reason}")

    if failures:
        for failure in failures:
            print(f"FAIL {failure}", file=sys.stderr)
        print(f"\n{len(failures)} claim(s) failed over {tail}", file=sys.stderr)
        return 1

    print(f"claims OK: {tail}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
