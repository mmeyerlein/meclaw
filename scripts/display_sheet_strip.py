#!/usr/bin/env python3
"""The rule by which the shipped copies of the sheet lose their comments (OR-G0.1).

`display-dna.css` is the source and keeps every byte of every comment: that is where
the WHY has to survive the next reader, and that is the file a person opens, greps and
diffs. The copies that TRAVEL -- `KIT_CSS` in `compose.py`, and `compose.py` again in
`config.json`'s `params.script_inline` -- carry the rules and not the reasoning. Nobody
reads a reason in a view-source, and every screen was loading about 47 kB of it.

The head comment is the one that travels. It is the sheet's nameplate -- the marker a
running screen is recognised by -- and it says where the source is, so a copy found in
the wild leads back to the file that explains it.

One rule, one place. `display_sync.py` applies it when it copies, and the drift lock
`the_design_language_is_the_same_sheet_in_three_places` asks THIS file rather than
restating the rule in Rust: two implementations of one rule are two places a copy can
be wrong, and the lock exists to catch exactly that. It travels into the published
tree with `crates/`, because that lock runs there too.

It knows the two places where `/*` is not a comment: inside a string, and inside an
unquoted `url(...)`, where a slash and a star are ordinary characters of a path. The
sheet has neither case today; a stripper that only works on today's sheet is a trap
laid for whoever writes tomorrow's rule.

Beyond dropping comments it rstrips the lines they left behind and collapses runs of
blank lines to one. Nothing else is reformatted -- this is not a minifier, and the
sheet that reaches a screen is still a sheet a person can read. Idempotent: stripping
a stripped sheet returns it unchanged, because the head comment is the first comment
either way.
"""

import sys


def strip(css: str) -> str:
    """`css` with every comment but the first removed."""
    out = []
    i = 0
    n = len(css)
    seen_first = False
    quote = None
    while i < n:
        ch = css[i]
        if quote is not None:
            out.append(ch)
            if ch == "\\" and i + 1 < n:
                out.append(css[i + 1])
                i += 2
                continue
            if ch == quote:
                quote = None
            i += 1
            continue
        if ch in "\"'":
            quote = ch
            out.append(ch)
            i += 1
            continue
        if ch == "u" and _url_at(css, i):
            end = css.find(")", i)
            end = n if end < 0 else end + 1
            out.append(css[i:end])
            i = end
            continue
        if ch == "/" and css.startswith("/*", i):
            end = css.find("*/", i + 2)
            end = n if end < 0 else end + 2
            if not seen_first:
                out.append(css[i:end])
                seen_first = True
            i = end
            continue
        out.append(ch)
        i += 1
    return _tidy("".join(out))


def _url_at(css: str, i: int) -> bool:
    """Whether an unquoted `url(` starts here and is not part of a longer word.

    A quoted one needs no special case: the quote carries it through the string
    branch above. `_at_url` is only about `url(data:image/svg+xml;...)` and its
    kind, where `/` and `*` are path characters.
    """
    if not css.startswith("url(", i):
        return False
    if i and (css[i - 1].isalnum() or css[i - 1] in "-_"):
        return False
    rest = css[i + 4:]
    return not rest[:1] in ('"', "'")


def _tidy(css: str) -> str:
    """Trailing whitespace a removed comment left behind, and blank runs, go."""
    lines = [line.rstrip() for line in css.split("\n")]
    kept = []
    for line in lines:
        if line == "" and kept and kept[-1] == "":
            continue
        kept.append(line)
    return "\n".join(kept)


if __name__ == "__main__":
    src = open(sys.argv[1], encoding="utf-8").read() if len(sys.argv) > 1 else sys.stdin.read()
    sys.stdout.write(strip(src))
