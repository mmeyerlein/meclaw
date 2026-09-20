#!/usr/bin/env python3
"""Build the strand table of a wave receipt out of the reports' header blocks.

    scripts/wave_receipt.py <wave> [--receipt <file>]

WHY THIS EXISTS
===============
A wave receipt repeated what its reports already said. Measured over four
waves: of the 2 640 eight-grams in one receipt exactly two were text that did
not stand in a report, while 25 of its 34 commit SHAs and 10 of its 14 issue
numbers were copies. The same gate summary line stood in up to three files of
one wave -- the report, the review and the receipt. The prose of a receipt is
new; its FACTS are transcription, and transcription is where a wrong SHA
enters.

So the facts are written once, in the header block at the top of every strand
report:

    ---
    strang: kit
    branch: welle-p/kit
    issues: [#756, #755]
    basis: a85bc777
    gate: "GATE-SUMMARY strand 1bf0ad5 12/12 274s GREEN"
    commits: [fb22b543, c790000d]
    ---

`scripts/strand.sh new` writes that block, `scripts/strand.sh report` fills it
from the gate archive, and this tool turns all of them into ONE table, put
into the receipt between two markers:

    <!-- strands:begin -->
    <!-- strands:end -->

A receipt that has no markers yet gets the section appended once; from then on
the tool only ever replaces what stands between them. Running it twice changes
nothing -- that is the property that lets it run at the end of every strand
instead of once at the end of the wave.

The merge column is the one fact no report can know: it is read from the merge
commits of `master` whose subject names the strand's branch.

Exit 0 = the receipt is up to date.
"""

import argparse
import os
import pathlib
import re
import subprocess
import sys

BEGIN = "<!-- strands:begin -->"
END = "<!-- strands:end -->"
HEADER = re.compile(r"^---\n(.*?)\n---\n", re.S)
COLUMNS = ("Strang", "Issues", "Gate", "Commits", "Merge")


def git(repo, *args):
    """git, or the empty string -- a missing history is not a crash here."""
    out = subprocess.run(["git", "-C", str(repo)] + list(args),
                         capture_output=True, text=True)
    return out.stdout.strip() if out.returncode == 0 else ""


def main_root(start):
    """The main worktree of `start`, so a linked one writes the same files."""
    common = git(start, "rev-parse", "--path-format=absolute",
                 "--git-common-dir")
    return pathlib.Path(os.path.dirname(common)) if common else pathlib.Path(start)


def read_header(path):
    """The header block of a report as a dict, or None if it has none."""
    m = HEADER.match(path.read_text())
    if not m:
        return None
    head = {}
    for line in m.group(1).splitlines():
        key, _, value = line.partition(":")
        head[key.strip()] = value.strip()
    return head


def merge_of(repo, branch, merges):
    """The merge commit on master that took this branch, or the empty string.

    The name must end where it ends: `welle-g/g1` is a substring of
    `welle-g/g12`, and a plain `in` credited `g1` with `g12`'s merge (wave G,
    thirteen strands). A trailing digit, letter, `-`, `_`, `.` or `/` means the
    subject names a DIFFERENT branch.
    """
    if not branch:
        return ""
    ends = re.compile(re.escape(branch) + r"(?![\w./-])")
    for sha, subject in merges:
        if ends.search(subject):
            return sha
    return ""


def cell(value):
    """A table cell: no pipe may break the row, and empty reads as a dash."""
    value = (value or "").strip().strip('"').replace("|", "\\|")
    return value if value else "--"


def listing(value):
    """`[a, b]` -> `a, b`."""
    return ", ".join(v.strip() for v in (value or "").strip("[]").split(",")
                     if v.strip())


def build_table(repo, wave_dir):
    reports = sorted((wave_dir / "berichte").glob("*.md")) \
        if (wave_dir / "berichte").is_dir() else []
    merges = [ln.split("\t", 1) for ln in
              git(repo, "log", "--merges", "--abbrev=8", "--format=%h\t%s",
                  "master").splitlines() if "\t" in ln]
    rows = []
    for path in reports:
        head = read_header(path)
        if head is None:
            print("wave_receipt: no header block, skipped: %s" % path.name,
                  file=sys.stderr)
            continue
        gate = head.get("gate", "").strip().strip('"')
        rows.append([
            cell(head.get("strang") or path.stem),
            cell(listing(head.get("issues", ""))),
            # The gate cell goes through `cell()` too: a repo without cargo
            # answers with several suites in ONE line (`test_app.sh 700 |
            # test_ui.sh 1 failure | ...`), and an unescaped pipe there turns
            # five columns into nine.
            "`%s`" % cell(gate).replace("`", "'") if gate else "--",
            cell(listing(head.get("commits", ""))),
            cell(merge_of(repo, head.get("branch", ""), merges)),
        ])
    out = ["| " + " | ".join(COLUMNS) + " |",
           "|" + "|".join(["---"] * len(COLUMNS)) + "|"]
    out += ["| " + " | ".join(r) + " |" for r in rows]
    return "\n".join(out)


def splice(text, table):
    """Replace what stands between the markers, or append the section once."""
    block = "%s\n%s\n%s" % (BEGIN, table, END)
    if BEGIN in text and END in text:
        return re.sub(re.escape(BEGIN) + r".*?" + re.escape(END), lambda _: block,
                      text, count=1, flags=re.S)
    return text.rstrip("\n") + "\n\n" + block + "\n"


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("wave", help="the wave directory under plans/")
    ap.add_argument("--receipt", default=None,
                    help="the receipt to write (default: <wave>/receipt.md)")
    args = ap.parse_args(argv)

    repo = main_root(os.getcwd())
    wave_dir = pathlib.Path(args.wave)
    if not wave_dir.is_dir():
        wave_dir = repo / "plans" / args.wave.strip("/")
    if not wave_dir.is_dir():
        sys.exit("wave_receipt: no such wave: %s" % args.wave)

    receipt = pathlib.Path(args.receipt) if args.receipt \
        else wave_dir / "receipt.md"
    table = build_table(repo, wave_dir)
    before = receipt.read_text() if receipt.is_file() else \
        "# %s -- Receipt\n" % wave_dir.name
    after = splice(before, table)
    if after != before:
        receipt.write_text(after)
    print("wave_receipt: %s" % receipt)
    return 0


if __name__ == "__main__":
    sys.exit(main())
