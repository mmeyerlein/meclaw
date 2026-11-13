#!/usr/bin/env python3
"""Git merge driver for cell configs that carry their script inline.

A code cell's `config.json` holds the whole script as one `params.script_inline`
string. For `templates/curator/policy/config.json` that is one JSON line of about
500 000 characters among some 640 readable ones. Two branches that edit the
script in two different places collide there as a single line nobody can resolve
by hand, so every rebase meant applying the change again. The display's config
has had a driver for this since GH #759 (`git_merge_display_sync.py`), but that
one leans on `compose.py` beside the config; these cells have no source file of
their own -- the config is the only copy of the script.

The driver runs TWO three-way merges over the same three sides:

1. `params.script_inline` -- the three strings go through `git merge-file` as
   text, line by line, the way git would merge the script if it were a file.
2. every other field -- the three configs are merged key by key
   (`merge_objects`). Objects are merged recursively; any other value (a
   string, a number, a list) counts as a whole. A key only one side changed,
   added or deleted follows that side. The result carries this side's keys in
   this side's order, then the other side's new keys in theirs. Taking one
   side wholesale would silently drop whatever the other side added, and
   merging the readable lines as text made a conflict of two strands that each
   append one new param at the end of `params` -- to a text merge the same
   place (measured on the policy cell, OR-58-61).

A config merged without a conflict is written back in the form the tree's
configs have (`form_of`, `render`): two spaces of indent, a final newline, and
characters outside ASCII either as they are or as `\\uXXXX` escapes -- the tree
carries both spellings, and a merge must not turn one into the other.

Conflicts, exit 1 either way:

* in the script: the marks never go into the JSON line. The script with its
  `<<<<<<< ours` / `=======` / `>>>>>>> theirs` marks is written to a file under
  `target/script-inline-conflicts/` (ignored by git, outside the template
  tree), and the config keeps this side's script whole, with the merged fields
  around it. Resolve the marks in that file, then

      python3 scripts/git_merge_script_inline.py --resolve <config> <file>

  writes the script back into the JSON line and removes the file; `git add` the
  config after it.
* in the fields: both sides changed one key to two values, or one side
  deleted a key the other changed. The config never carries a mark -- it stays
  one valid document with this side's state at each of these keys and
  everything else merged. A valid config is what one `git add` takes without a
  word, so the conflict is not left at a message: every such key is written
  with its base, ours and theirs value to `fields.json` beside the script's
  file under `target/script-inline-conflicts/`, the message names each key by
  its path (`params.keep_recent`, a key with a dot in it quoted:
  `params["a.b"]`) with the three values shortened, and `--resolve` refuses
  while that file is there. Set the keys by hand, remove the file, then
  `git add` the config.

A merge with more than one merge base (a criss-cross history) first merges
the bases into a virtual one, with the driver git names in
`merge.script-inline.recursive`: this script with `--inner`. There a key the
bases changed two ways gets a mark value that no config ever holds (a string
that starts with a NUL character and carries both values), and a script hunk
the bases changed two ways gets `git merge-file`'s conflict marks -- what git
puts into a virtual base for a text file. Exit 0, no file under `target/`: the
virtual base is no file anybody resolves. In the real merge each such key and
hunk then conflicts unless both sides agree, visibly, with `fields.json` or the
conflict file. Keeping one base's state made a key only the other side had
changed look changed on both sides and this side's value stayed (OR-58-146,
the followup merge of the d58-2 wave, three merge bases); keeping the older
base's state lost a side whenever the two criss-cross merges had settled the
key two ways, one of them back at that older value (review close-kit I1). A
side the inner merge cannot read as a config with a script -- one without a
script, or an empty `%O` (an add/add, read as `{}`) -- is merged key by key,
its script, where any side has one, as text with a missing one as the empty
text; scripts `git merge-file` cannot merge (an input already carries a mark,
three or more merge bases) give a plain text mark. Only a side that is no
JSON object at all makes the virtual base a mark text the real merge stops
at: there the file is left to a person, with exit 1.

Four more commands:

    --attributes [--write]   the `.gitattributes` lines: every cell config with a
                             script under the roots in `ROOTS` (a root that does
                             not exist yet is taken up the day it appears);
                             `--write` puts them into `.gitattributes`
    --install [<clone>]      the five `git config` lines of the two drivers
                             (`script-inline`, `script-inline-inner`) in the
                             clone; a second run changes nothing
    --inner %O %A %B %P      the internal merge above; git calls it

`.gitattributes` asks for the driver, the `git config` half is local on purpose:
git never runs a driver a repository merely asks for. `strand.sh new` installs
it for every new strand; a clone installed before the inner driver existed
gets it at its next `new` or `--install`. The build lanes only check out
commits and never merge, so they need no driver. The display's driver and its
lines are not touched.
"""
import json
import pathlib
import subprocess
import sys
import tempfile

NAME = "script-inline"
DRIVER_NAME = "cell script_inline three-way merge"
# %P is the config's path in the tree: it names the conflict file.
DRIVER_COMMAND = "python3 scripts/git_merge_script_inline.py %O %A %B %P"
# git's internal merge of two merge bases into a virtual one (a criss-cross
# history) runs `merge.<driver>.recursive`: this mode marks whatever both
# bases changed two ways, so the real merge conflicts there (OR-58-146, I1).
INNER_NAME = "script-inline-inner"
INNER_DRIVER_NAME = "cell script_inline merge of merge bases"
INNER_COMMAND = "python3 scripts/git_merge_script_inline.py --inner %O %A %B %P"

# The templates whose code cells several strands edit side by side.
# `templates/lead/` does not exist yet; the walk skips a missing root.
ROOTS = ("templates/curator", "templates/talky", "templates/collector",
         "templates/assistant", "templates/objects", "templates/lead")

CONFLICT_DIR = "target/script-inline-conflicts"
OURS_MARK, SEPARATOR, THEIRS_MARK = "<<<<<<< ours", "=======", ">>>>>>> theirs"

ATTR_BEGIN = "# script-inline: begin (generated list, see above)"
ATTR_END = "# script-inline: end"
ATTR_HEAD = """\
# A code cell's config.json carries its whole script as one
# `params.script_inline` string, so two branches that edit the script collide
# there as a single unreadable line. scripts/git_merge_script_inline.py merges
# the script as text and the other fields beside it. The list is generated:
#     python3 scripts/git_merge_script_inline.py --attributes --write
# Git only runs a driver the clone itself configures:
#     python3 scripts/git_merge_script_inline.py --install
"""


def say(text: str) -> None:
    print(f"{NAME}: {text}", file=sys.stderr)


# -- the form of a config ------------------------------------------------------

def form_of(*raws: bytes) -> bool:
    """`ensure_ascii` of the first side that tells: the tree carries configs with
    characters outside ASCII written as they are (curator/policy) and written as
    escapes (curator/intake). A side with only ASCII content says nothing; if
    none tells, the answer is the policy cell's spelling."""
    for raw in raws:
        if not raw.isascii():
            return False
        if b"\\u" in raw and not json.dumps(json.loads(raw), ensure_ascii=False).isascii():
            return True
    return False


def encode(text: str) -> bytes:
    # A lone surrogate is legal in a JSON string and not in UTF-8.
    return text.encode("utf-8", "surrogatepass")


def decode(raw: bytes) -> str:
    return raw.decode("utf-8", "surrogatepass")


def render(cfg, ensure_ascii: bool) -> bytes:
    """The bytes of a config in the tree's form."""
    return encode(json.dumps(cfg, indent=2, ensure_ascii=ensure_ascii) + "\n")


def script_of(cfg) -> str:
    script = cfg["params"]["script_inline"]
    if not isinstance(script, str):
        raise TypeError("params.script_inline is not a string")
    return script


def with_script(cfg, script: str):
    """`cfg` with another script, every key where it was."""
    return {**cfg, "params": {**cfg["params"], "script_inline": script}}


# -- the two merges ------------------------------------------------------------

# The marks of the internal merge's text merge: longer than any mark a script
# could carry itself, so a line of the script never reads as one of them.
INNER_MARK_SIZE = 41


def run_merge_file(base: str, ours: str, theirs: str, *options: str):
    """`git merge-file -p` over the three texts; the finished process."""
    with tempfile.TemporaryDirectory() as tmp:
        paths = []
        for name, text in (("ours", ours), ("base", base), ("theirs", theirs)):
            path = pathlib.Path(tmp) / name
            path.write_bytes(encode(text))
            paths.append(str(path))
        # rerere off like every merge of the kit (OR-TM-11, `pipeline/gitenv.NO_RERERE`);
        # the plain two-sided marks whatever `merge.conflictStyle` the clone has.
        done = subprocess.run(["git", "-c", "rerere.enabled=false",
                               "-c", "merge.conflictStyle=merge", "merge-file", "-p",
                               *options, "-L", "ours", "-L", "base", "-L", "theirs",
                               *paths], capture_output=True)
    if done.returncode < 0 or done.returncode > 127:
        raise OSError(f"git merge-file failed: {decode(done.stderr).strip()}")
    return done


def merge_texts(base: str, ours: str, theirs: str) -> tuple[str, bool]:
    """The three texts merged the way git merges a file; (result, conflicted)."""
    done = run_merge_file(base, ours, theirs)
    return decode(done.stdout), done.returncode != 0


def merge_texts_inner(base: str, ours: str, theirs: str) -> tuple[str, int]:
    """The three texts merged for a virtual base: every hunk both sides changed
    two ways keeps `git merge-file`'s conflict marks, as git does for a text
    file; (result, conflicted hunks). The marks differ from every side, so the
    real merge conflicts at that hunk unless both of its sides agree there."""
    done = run_merge_file(base, ours, theirs, f"--marker-size={INNER_MARK_SIZE}")
    return decode(done.stdout), done.returncode


# The start of a mark value of the internal merge. JSON escapes a NUL, so it
# may stand in a config, but no config of the tree carries one: a mark value
# equals no side's value, and the real merge sees the key changed on both.
INNER_MARK = "\x00script-inline: merge bases changed this key two ways\x00"


def inner_mark(mine, other) -> str:
    """The value of a key both merge bases changed two ways: both values in a
    fixed order, so the virtual base is the same whichever base git calls ours."""
    texts = sorted("(none)" if v is MISSING else json.dumps(v, ensure_ascii=False)
                   for v in (mine, other))
    return INNER_MARK + " | ".join(texts)


def conflict_dir(config_path: str, root=None) -> pathlib.Path:
    """Where the conflicts of `config_path` wait: under `target/`, mirroring
    the config's directory. Relative to `root`, else to the working directory
    (git runs a driver at the top of the worktree)."""
    here = pathlib.Path(root) if root is not None else pathlib.Path()
    return here / CONFLICT_DIR / pathlib.PurePosixPath(config_path).parent


def conflict_path(config_path: str, root=None) -> pathlib.Path:
    """The script of `config_path` with its marks, named so an editor reads it
    as a script."""
    return conflict_dir(config_path, root) / "script_inline.py"


def fields_path(config_path: str, root=None) -> pathlib.Path:
    """The keys of `config_path` that conflicted, with base, ours and theirs."""
    return conflict_dir(config_path, root) / "fields.json"


def same(one, other) -> bool:
    """Equal as JSON text: Python's `1 == 1.0 == True` would call a changed
    spelling unchanged, and the tree's form is judged by the byte."""
    return json.dumps(one) == json.dumps(other)


MISSING = object()      # the side has no such key


def where(path: tuple) -> str:
    """A key's path as the message names it: `params.limits.soft`. A key that
    carries a dot or a bracket itself is quoted, `params["a.b"]` -- spelled
    plainly it would read as two keys (mergedrv M4 c)."""
    out = ""
    for key in path:
        if key and not any(c in key for c in '.["]'):
            out += ("." if out else "") + key
        else:
            out += "[" + json.dumps(key, ensure_ascii=False) + "]"
    return out


def merge_objects(base: dict, ours: dict, theirs: dict, path: tuple,
                  conflicts: list, inner: bool = False) -> dict:
    """Three-way merge of one JSON object, key by key. Objects recurse, every
    other value is one whole. A conflict adds `(path, what, base, ours, theirs)`
    to `conflicts` (`MISSING` for a side without the key) and keeps this side's
    state of the key -- or, with `inner`, a mark value carrying both: the merge
    of two merge bases into git's virtual base must favour neither and must
    leave the real merge a conflict there (OR-58-146, review close-kit I1)."""
    out = {}

    def conflict(key, what, mine, other):
        was = base.get(key, MISSING)
        conflicts.append(((*path, key), what, was, mine, other))
        if inner:
            out[key] = inner_mark(mine, other)
        elif mine is not MISSING:
            out[key] = mine

    for key, mine in ours.items():
        if key not in theirs:
            if key not in base:
                out[key] = mine                 # added here
            elif not same(base[key], mine):
                conflict(key, "changed on this side, deleted on the other", mine, MISSING)
            continue                            # else: deleted there, untouched here
        other = theirs[key]
        if isinstance(mine, dict) and isinstance(other, dict) \
                and isinstance(base.get(key, {}), dict):
            out[key] = merge_objects(base.get(key, {}), mine, other, (*path, key),
                                     conflicts, inner)
        elif same(mine, other) or (key in base and same(base[key], other)):
            out[key] = mine
        elif key in base and same(base[key], mine):
            out[key] = other
        else:
            conflict(key, ("changed" if key in base else "added")
                     + " on both sides with two different values", mine, other)
    for key, other in theirs.items():
        if key in ours:
            continue
        if key not in base:
            out[key] = other                    # added there
        elif not same(base[key], other):
            conflict(key, "deleted on this side, changed on the other", MISSING, other)
    return out


def shown(values, width: int = 60) -> list[str]:
    """The three values of one key for the message: JSON, cut at `width`
    characters. Three long texts that differ only far in agree in their first
    `width` characters; then each starts shortly before the first difference."""
    texts = [None if v is MISSING else json.dumps(v, ensure_ascii=False) for v in values]
    present = [t for t in texts if t is not None]
    common = len(present[0]) if present else 0
    for text in present[1:]:
        common = min(common, next((i for i, (a, b) in enumerate(zip(present[0], text))
                                   if a != b), min(len(present[0]), len(text))))
    skip = max(0, common - 20) if common > width - 20 else 0
    out = []
    for text in texts:
        if text is None:
            out.append("(none)")
            continue
        part = ("..." + text[skip:]) if skip else text
        out.append(part if len(part) <= width else part[:width - 3] + "...")
    return out


SIDES = ("base", "ours", "theirs")


# The script of a virtual base whose scripts `git merge-file` could not merge
# (an input already carries a mark of an earlier internal merge, three or
# more merge bases). Plain text, no NUL: the real merge's text merge reads it
# as a base no side equals and conflicts there unless both sides agree.
INNER_SCRIPT_MARK = ("script-inline: the merge bases' scripts could not be merged;"
                     " the real merge conflicts here unless both sides agree\n")


def script_or_none(cfg):
    """The script of a config, None when it carries none."""
    params = cfg.get("params")
    if isinstance(params, dict) and isinstance(params.get("script_inline"), str):
        return params["script_inline"]
    return None


def without_script(cfg):
    """`cfg` without its script key, every other key where it was."""
    params = cfg.get("params")
    if isinstance(params, dict) and "script_inline" in params:
        return {**cfg, "params": {k: v for k, v in params.items() if k != "script_inline"}}
    return cfg


def merge_scripts_inner(scripts) -> tuple[str, int]:
    """The scripts of the three inputs of the internal merge as text (a side
    without one as the empty text, the way git hands an add/add to a text
    merge); a failed text merge gives the plain mark `INNER_SCRIPT_MARK`."""
    try:
        return merge_texts_inner(*scripts)
    except OSError as exc:
        say(f"{exc}; the virtual base's script is a mark, the real merge conflicts "
            "there unless both sides agree")
        return INNER_SCRIPT_MARK, 1


def merge_unreadable_bases(files, config_path: str, side: str, exc) -> int:
    """The internal merge with a side that is no config with a script -- a merge
    base from before the cell carried one, say, or an empty `%O` (both bases
    added the file, an add/add: read as `{}`). Returning 1 there would leave
    `%A` at one base, the state of one side kept silently (review close-kit m1).
    Three JSON objects merge key by key; their scripts, where any side has one,
    merge as text like every script (a side without one as the empty text), so
    a script the bases added or changed two ways carries `git merge-file`'s
    marks and never becomes a mark value the real text merge would refuse as
    binary (re-check close-kit m1'). Anything else makes the virtual base a mark
    text that no side equals, and the real merge stops at its base and leaves
    the file to a person. Exit 0 either way: git takes the file as the virtual
    base."""
    try:
        raws = [path.read_bytes() for path in files]
        cfgs = [json.loads(raw) if raw.strip() else {} for raw in raws]
        if not all(isinstance(cfg, dict) for cfg in cfgs):
            raise TypeError("a side is no JSON object")
        ensure_ascii = form_of(raws[1], raws[2], raws[0])
    except (OSError, ValueError, TypeError):
        files[1].write_bytes(encode(INNER_MARK + f" the {side} merge base of {config_path} "
                                    "is no JSON object\n"))
        say(f"a merge base of {config_path} is no config ({exc!r}); the virtual base "
            "is a mark, the merge stops at it -- merge the file by hand "
            f"(`git show :2:{config_path}` ours, `:3:` theirs)")
        return 0
    scripts = [script_or_none(cfg) for cfg in cfgs]
    hunks = 0
    if any(script is not None for script in scripts):
        merged_script, hunks = merge_scripts_inner(
            [script if script is not None else "" for script in scripts])
        cfgs = [without_script(cfg) for cfg in cfgs]
    conflicts: list = []
    merged = merge_objects(*cfgs, (), conflicts, inner=True)
    if hunks or any(script is not None for script in scripts):
        if isinstance(merged.get("params"), dict):
            merged["params"] = {**merged["params"], "script_inline": merged_script}
        else:
            merged["params"] = inner_mark(merged.get("params", MISSING), merged_script)
    files[1].write_bytes(render(merged, ensure_ascii))
    say(f"a merge base of {config_path} has no script ({exc!r}); the bases merged key "
        f"by key, {len(conflicts)} key(s) changed two ways carry a mark, "
        f"{hunks} script hunk(s) carry merge-file's marks")
    return 0


def merge(base_file: str, ours_file: str, theirs_file: str, config_path: str,
          inner: bool = False) -> int:
    files = [pathlib.Path(p) for p in (base_file, ours_file, theirs_file)]
    raws, cfgs, scripts = [], [], []
    for side, path in zip(SIDES, files):
        try:
            raws.append(path.read_bytes())
            cfgs.append(json.loads(raws[-1]))
            scripts.append(script_of(cfgs[-1]))
        except (OSError, ValueError, KeyError, TypeError) as exc:
            if inner:
                return merge_unreadable_bases(files, config_path, side, exc)
            # mergedrv M4 b: which side, and what to do about it.
            say(f"the {side} side of {config_path} is not a config with a script "
                f"({exc!r}); the file stays as git left it -- merge it by hand "
                f"(`git show :1:{config_path}` base, `:2:` ours, `:3:` theirs)")
            return 1
    # ours, theirs, base: the side being merged INTO decides first.
    ensure_ascii = form_of(raws[1], raws[2], raws[0])
    if not inner and render(cfgs[1], ensure_ascii) != raws[1]:
        say(f"NOTE {config_path} is not in the tree's form on this side; "
            "the merge writes it in that form")

    try:
        if inner:
            merged_script, hunks = merge_scripts_inner(scripts)
            script_conflict = False
        else:
            merged_script, script_conflict = merge_texts(*scripts)
    except OSError as exc:      # mergedrv M5: one line, not a traceback
        say(f"{exc} -- {config_path} is unchanged")
        return 1
    # The script is settled above: the key merge sees one script on all three
    # sides, so the key stays where this side has it and never conflicts.
    field_conflicts: list = []
    merged = merge_objects(*(with_script(cfg, "") for cfg in cfgs), (), field_conflicts,
                           inner)

    if inner:
        # git's internal merge of the merge bases: the result is the virtual
        # base of the real merge, not a file anybody resolves. What conflicted
        # carries a mark, so the real merge conflicts there unless both sides
        # agree -- never one side's value kept silently (OR-58-146, I1).
        files[1].write_bytes(render(with_script(merged, merged_script), ensure_ascii))
        if hunks or field_conflicts:
            say(f"merge bases of {config_path} merged; {len(field_conflicts)} key(s) and "
                f"{hunks} script hunk(s) changed two ways carry a mark, the merge "
                "conflicts there unless both sides agree")
        return 0

    # On a script conflict the config keeps this side's script: one whole
    # script in a valid line, never a line with marks in it.
    script = scripts[1] if script_conflict else merged_script
    files[1].write_bytes(render(with_script(merged, script), ensure_ascii))

    waiting, fields = conflict_path(config_path), fields_path(config_path)
    if script_conflict:
        waiting.parent.mkdir(parents=True, exist_ok=True)
        waiting.write_bytes(encode(merged_script))
        say(f"the script of {config_path} conflicted. The marks are in {waiting}; "
            "the config keeps this side's script. Resolve the marks there, then run\n"
            f"    python3 scripts/git_merge_script_inline.py --resolve {config_path} {waiting}\n"
            f"and `git add {config_path}`.")
    elif waiting.is_file():
        waiting.unlink()    # a conflict of an earlier attempt, settled by this merge
    if field_conflicts:
        # Not silent: a valid config with this side's value is what one
        # `git add` took without a word (OR-58-146). The file holds every value
        # whole, the message each one shortened.
        table = {}
        for key_path, _, *values in field_conflicts:
            table[where(key_path)] = {side: value for side, value in zip(SIDES, values)
                                      if value is not MISSING}
        fields.parent.mkdir(parents=True, exist_ok=True)
        fields.write_bytes(encode(json.dumps(table, indent=2, ensure_ascii=False) + "\n"))
        say(f"fields of {config_path} conflicted:\n"
            + "".join(f"    {where(key_path)}: "
                      + ", ".join(f"{side} {text}"
                                  for side, text in zip(SIDES, shown(values)))
                      + f" ({what})\n"
                      for key_path, what, *values in field_conflicts)
            + f"Every value is whole in {fields}. The config carries no marks: it keeps "
            "this side's state of each of these keys, everything else is merged. Set them "
            f"by hand in the config, then remove {fields}"
            + (" before the script's `--resolve`." if script_conflict
               else f" and `git add {config_path}`."))
    elif fields.is_file():
        fields.unlink()     # likewise
    return 1 if script_conflict or field_conflicts else 0


# -- the way back ----------------------------------------------------------------

def top_of(path: pathlib.Path):
    """The top of the worktree `path` lies in, or None."""
    done = subprocess.run(["git", "-C", str(path.parent), "rev-parse", "--show-toplevel"],
                          capture_output=True, text=True)
    return pathlib.Path(done.stdout.strip()).resolve() if done.returncode == 0 else None


def stage_raws(top, rel: str) -> list[bytes]:
    """The three sides git still holds for a conflicted path: ours, theirs,
    base (stages 2, 3, 1), whichever exist."""
    raws = []
    for stage in (2, 3, 1):
        done = subprocess.run(["git", "-C", str(top), "show", f":{stage}:{rel}"],
                              capture_output=True)
        if done.returncode == 0:
            raws.append(done.stdout)
    return raws


def resolve(config_file: str, script_file: str) -> int:
    config, source = pathlib.Path(config_file), pathlib.Path(script_file)
    try:
        script = decode(source.read_bytes())
    except (OSError, ValueError) as exc:
        say(f"cannot read the resolved script {source}: {exc}")
        return 1
    # The separator counts as a mark: with the two outer marks deleted by hand
    # it was the one line left of a conflict, and it went into the script
    # (mergedrv M1).
    lines = script.splitlines()
    left = [mark for mark in (OURS_MARK, SEPARATOR, THEIRS_MARK) if mark in lines]
    if left:
        say(f"{source} still carries a conflict mark ({', '.join(left)}) -- "
            f"{config} is unchanged")
        return 1
    try:
        raw = config.read_bytes()
        cfg = json.loads(raw)
        script_of(cfg)
    except (OSError, ValueError, KeyError, TypeError) as exc:
        say(f"{config} is not a config with a script ({exc!r})")
        return 1
    # The conflict files lie under the worktree's top, wherever this runs
    # from (mergedrv M2).
    top = top_of(config.resolve())
    rel = config.resolve().relative_to(top).as_posix() if top is not None else None
    fields = fields_path(rel, top) if rel is not None else None
    if fields is not None and fields.is_file():
        say(f"fields of {config} still conflict: {fields} names them. Set them in the "
            f"config, remove that file, then run this again -- {config} is unchanged")
        return 1
    # The form of the three sides, not only of the config as it stands: this
    # side's file may no longer carry the one character that tells (mergedrv M3).
    sides = stage_raws(top, rel) if rel is not None else []
    config.write_bytes(render(with_script(cfg, script), form_of(raw, *sides)))
    try:
        home = conflict_dir(".", top).resolve() if top is not None \
            else pathlib.Path(CONFLICT_DIR).resolve()
        inside = home in source.resolve().parents
    except OSError:
        inside = False
    if inside:
        source.unlink()     # nothing stale waits for the next conflict
    print(f"{NAME}: the script is back in {config} -- `git add {config}`")
    return 0


# -- the list for .gitattributes -------------------------------------------------

def attribute_lines(repo) -> list[str]:
    """One line per cell config with a script under `ROOTS`, sorted by path."""
    repo = pathlib.Path(repo)
    found = []
    for root in ROOTS:
        base = repo / root
        if not base.is_dir():
            continue
        for path in base.rglob("config.json"):
            try:
                script_of(json.loads(path.read_bytes()))
            except (OSError, ValueError, KeyError, TypeError):
                continue    # no cell, or a cell without a script
            found.append(path.relative_to(repo).as_posix())
    return [f"{rel} merge={NAME}" for rel in sorted(found)]


def write_attributes(repo) -> None:
    """Put the list into `.gitattributes`: between its two marker lines, or as
    a new block at the end. Every other line stays."""
    path = pathlib.Path(repo) / ".gitattributes"
    old = path.read_text(encoding="utf-8") if path.is_file() else ""
    lines = old.splitlines()
    block = [ATTR_BEGIN, *attribute_lines(repo), ATTR_END]
    if ATTR_BEGIN in lines and ATTR_END in lines[lines.index(ATTR_BEGIN):]:
        start = lines.index(ATTR_BEGIN)
        end = lines.index(ATTR_END, start)
        lines[start:end + 1] = block
    else:
        # Lines of this driver outside a block: the block replaces them.
        lines = [line for line in lines if not line.endswith(f" merge={NAME}")]
        if lines and lines[-1] != "":
            lines.append("")
        lines += ATTR_HEAD.splitlines() + block
    new = "\n".join(lines) + "\n"
    if new != old:
        path.write_text(new, encoding="utf-8")


# -- the clone's half ------------------------------------------------------------

def install(repo: str) -> int:
    """Set the driver in the clone's config; a value already right is not written."""
    changed = []
    for key, want in ((f"merge.{NAME}.name", DRIVER_NAME),
                      (f"merge.{NAME}.driver", DRIVER_COMMAND),
                      (f"merge.{NAME}.recursive", INNER_NAME),
                      (f"merge.{INNER_NAME}.name", INNER_DRIVER_NAME),
                      (f"merge.{INNER_NAME}.driver", INNER_COMMAND)):
        got = subprocess.run(["git", "-C", repo, "config", "--local", "--get", key],
                             capture_output=True, text=True)
        if got.returncode == 0 and got.stdout.rstrip("\n") == want:
            continue
        done = subprocess.run(["git", "-C", repo, "config", "--local", key, want],
                              capture_output=True, text=True)
        if done.returncode != 0:
            say(f"could not set {key} in {repo}: {done.stderr.strip()}")
            return 1
        changed.append(key)
    if changed:
        print(f"{NAME}: installed ({', '.join(changed)})")
    else:
        print(f"{NAME}: already installed, nothing changed")
    return 0


USAGE = """\
usage: git_merge_script_inline.py %O %A %B %P
       git_merge_script_inline.py --inner %O %A %B %P
       git_merge_script_inline.py --resolve <config> <resolved-script>
       git_merge_script_inline.py --attributes [--write]
       git_merge_script_inline.py --install [<clone>]"""


def main(argv: list[str]) -> int:
    args = argv[1:]
    here = pathlib.Path(__file__).resolve().parents[1]
    if args[:1] == ["--resolve"] and len(args) == 3:
        return resolve(args[1], args[2])
    if args == ["--attributes"]:
        sys.stdout.write("".join(line + "\n" for line in attribute_lines(here)))
        return 0
    if args == ["--attributes", "--write"]:
        write_attributes(here)
        return 0
    if args[:1] == ["--install"] and len(args) <= 2:
        return install(args[1] if len(args) == 2 else ".")
    if args[:1] == ["--inner"] and len(args) == 5:
        return merge(*args[1:], inner=True)
    if len(args) == 4 and not args[0].startswith("--"):
        return merge(*args)
    print(USAGE, file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv))
