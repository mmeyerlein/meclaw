#!/usr/bin/env python3
"""Git merge driver for `templates/display/compose/config.json`.

The file carries the whole of `compose.py` as a one-line `params.script_inline`
string. Two branches that edit `compose.py` in two different places merge cleanly
there and collide here as a single line of tens of thousands of characters -- a
conflict nobody resolves by hand. It is not rare: of five master→strand re-merges
in one day, four hit it, and every one was resolved the same way, by taking the
other side and letting `display_sync.py` write the copies again.

This driver does that automatically, and only for the line nobody can read. It
runs TWO three-way merges over the same three sides:

1. `params.script_inline` -- the three strings go through `git merge-file`, the
   same merge git runs on `compose.py` itself, over the same bytes, so the two
   files come out of the merge agreeing. Reading the merged `compose.py` off
   disk would not work: git computes every file's merge before it writes any of
   them back to the working tree.
2. every other field -- the three configs are written out with `script_inline`
   replaced by one fixed marker and go through `git merge-file` as well. A field
   only one side changed survives from that side, the same value on both sides
   is no change at all, and two sides with two values conflict. Taking the other
   side wholesale would have been cheaper and silently drops any field this side
   added -- a clean merge that loses data without a word is worse than the
   conflict it avoids.

`set_script_inline` and `dump_config` are imported from `display_sync.py` rather
than spelled again: two spellings of one rule are two places a copy can be wrong,
and the drift lock exists to catch exactly that.

On a conflict the driver exits 1 and git marks `config.json` the way it marks any
conflicted file. A field conflict keeps its `<<<<<<<` markers around the readable
line, with the merged script already back in place of the marker -- the collision
stays where a person can see it, and no `script_inline` is lost while resolving
it. A conflict in `compose.py` itself is left to `compose.py`: regenerating
`script_inline` from a file full of markers would hide it in a line no reviewer
can read.

Enable it once per clone -- `strand.sh new` does this for a fresh worktree:

    git config merge.display-sync.name 'display template sync'
    git config merge.display-sync.driver 'python3 scripts/git_merge_display_sync.py %O %A %B'

`.gitattributes` names the driver for the one file it applies to. The `git config`
half is local on purpose: git never runs a driver a repository merely asks for.
"""
import json
import pathlib
import subprocess
import sys
import tempfile

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from display_sync import dump_config, set_script_inline  # noqa: E402


# The one line the merge must never see. Both sides carry it identically, so the
# unreadable string can never be what two branches collide on.
MARKER = "__display_sync_script_inline__"


def script_of(path: pathlib.Path) -> str:
    """The `compose.py` one side of the merge carries."""
    return json.loads(path.read_text(encoding="utf-8"))["params"]["script_inline"]


def config_without_script(path: pathlib.Path) -> str:
    """One side of `config.json`, its script line replaced by `MARKER`."""
    cfg = json.loads(path.read_text(encoding="utf-8"))
    return dump_config(set_script_inline(cfg, MARKER))


def merge_texts(base: str, ours: str, theirs: str) -> tuple[str, bool]:
    """The three texts merged the way git merges a file; (result, conflicted)."""
    with tempfile.TemporaryDirectory() as tmp:
        paths = []
        for name, text in (("ours", ours), ("base", base), ("theirs", theirs)):
            path = pathlib.Path(tmp) / name
            path.write_text(text, encoding="utf-8")
            paths.append(str(path))
        done = subprocess.run(["git", "merge-file", "-p", "-L", "ours", "-L", "base",
                               "-L", "theirs", *paths], capture_output=True, text=True)
    return done.stdout, done.returncode != 0


def merge_scripts(base: str, ours: str, theirs: str) -> str | None:
    """The three scripts merged the way git merges `compose.py`; `None` on conflict."""
    merged, conflicted = merge_texts(base, ours, theirs)
    return None if conflicted else merged


def main(argv: list[str]) -> int:
    if len(argv) < 4:
        print("usage: git_merge_display_sync.py %O %A %B", file=sys.stderr)
        return 1
    base, ours, theirs = (pathlib.Path(a) for a in argv[1:4])
    try:
        scripts = [script_of(p) for p in (base, ours, theirs)]
        configs = [config_without_script(p) for p in (base, ours, theirs)]
    except (OSError, ValueError, KeyError, TypeError) as exc:
        print(f"display-sync: a side of config.json is not usable: {exc}", file=sys.stderr)
        return 1
    merged_script = merge_scripts(*scripts)
    if merged_script is None:
        print("display-sync: compose.py itself conflicted -- resolve it, run "
              "scripts/display_sync.py, then `git add` both files.", file=sys.stderr)
        return 1
    merged_config, conflicted = merge_texts(*configs)
    # Put the script back before anything else touches the file, so even a
    # conflicted config.json still carries a whole compose.py.
    text = merged_config.replace(json.dumps(MARKER), json.dumps(merged_script))
    if conflicted:
        ours.write_text(text, encoding="utf-8")
        print("display-sync: config.json fields conflicted -- the markers sit around "
              "the readable lines, script_inline is intact. Resolve, run "
              "scripts/display_sync.py, then `git add` both files.", file=sys.stderr)
        return 1
    try:
        merged = json.loads(text)
    except ValueError as exc:
        ours.write_text(text, encoding="utf-8")
        print(f"display-sync: the merged config.json is not valid JSON: {exc}",
              file=sys.stderr)
        return 1
    ours.write_text(dump_config(merged), encoding="utf-8")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
