#!/usr/bin/env python3
"""Copy `pass.py` into `compose.py` between the two marker lines (display-hive.md § 0.7).

The pass of the display hive is written ONCE, in the document; `pass.py` beside this file
is its byte copy, and the cell carries the same bytes so that a running screen and the
scenarios cannot drift apart. Nobody edits the section in `compose.py` by hand: a rule
changes in the document first, then `pass.py` is copied again, then this script runs.
Idempotent; `scripts/display_sync.py` calls it before it syncs the three places.

    python3 sync_pass.py            write the section
    python3 sync_pass.py --check    exit 1 when the section is not the model
"""
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
COMPOSE = os.path.join(HERE, "..", "compose.py")
MODEL = os.path.join(HERE, "pass.py")
BEGIN = "# --- The pass: display-hive.md § 4, model/pass.py verbatim (begin) ---"
END = "# --- The pass: display-hive.md § 4, model/pass.py verbatim (end) ---"
# Where the section goes when it is not there yet: ahead of the display's own vocabulary,
# after the constants, so the twelve steps stand with the other module-level definitions.
ANCHOR = ("# ---------------------------------------------------------------------------\n"
          "# The display's own vocabulary\n")


def model_body():
    """`pass.py` from its first constant on: the module docstring and `import copy` are the
    model's head and are not repeated in the cell (compose.py imports `copy` itself)."""
    text = open(MODEL, encoding="utf-8").read()
    return text[text.index("RUNGS = ("):].strip()


def section(body):
    return "%s\n%s\n%s\n" % (BEGIN, body, END)


def main(argv):
    body = model_body()
    cell = open(COMPOSE, encoding="utf-8").read()
    if BEGIN in cell and END in cell:
        start = cell.index(BEGIN)
        stop = cell.index(END) + len(END) + 1
        new = cell[:start] + section(body) + cell[stop:]
    else:
        if ANCHOR not in cell:
            sys.stderr.write("sync_pass: neither markers nor the anchor found in compose.py\n")
            return 2
        at = cell.index(ANCHOR)
        new = cell[:at] + section(body) + "\n" + cell[at:]
    if "--check" in argv:
        if new != cell:
            sys.stderr.write("sync_pass: compose.py's pass section is not pass.py\n")
            return 1
        return 0
    if new != cell:
        open(COMPOSE, "w", encoding="utf-8").write(new)
        print("sync_pass: wrote %d B of the reference model into compose.py" % len(body))
    else:
        print("sync_pass: compose.py already carries the reference model")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
