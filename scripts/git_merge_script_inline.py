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
  deleted a key the other changed. The message names each such key by its
  path (`params.keep_recent`); the config never carries a mark -- it stays one
  valid document with this side's state at each of these keys and everything
  else merged. Set the keys by hand, then `git add` the config.

Three more commands:

    --attributes [--write]   the `.gitattributes` lines: every cell config with a
                             script under the roots in `ROOTS` (a root that does
                             not exist yet is taken up the day it appears);
                             `--write` puts them into `.gitattributes`
    --install [<clone>]      the two `git config` lines of this driver in the
                             clone; a second run changes nothing

`.gitattributes` asks for the driver, the `git config` half is local on purpose:
git never runs a driver a repository merely asks for. `strand.sh new` installs
it for every new strand. The display's driver and its lines are not touched.
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

# The templates whose code cells several strands edit side by side.
# `templates/lead/` does not exist yet; the walk skips a missing root.
ROOTS = ("templates/curator", "templates/talky", "templates/collector",
         "templates/assistant", "templates/objects", "templates/lead")

CONFLICT_DIR = "target/script-inline-conflicts"
OURS_MARK, THEIRS_MARK = "<<<<<<< ours", ">>>>>>> theirs"

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

def merge_texts(base: str, ours: str, theirs: str) -> tuple[str, bool]:
    """The three texts merged the way git merges a file; (result, conflicted)."""
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
                               "-L", "ours", "-L", "base", "-L", "theirs", *paths],
                              capture_output=True)
    if done.returncode < 0 or done.returncode > 127:
        raise OSError(f"git merge-file failed: {decode(done.stderr).strip()}")
    return decode(done.stdout), done.returncode != 0


def conflict_path(config_path: str) -> pathlib.Path:
    """Where the script of `config_path` waits with its marks: under `target/`,
    mirroring the config's directory, named so an editor reads it as a script."""
    return pathlib.Path(CONFLICT_DIR) / pathlib.PurePosixPath(config_path).parent \
        / "script_inline.py"


def same(one, other) -> bool:
    """Equal as JSON text: Python's `1 == 1.0 == True` would call a changed
    spelling unchanged, and the tree's form is judged by the byte."""
    return json.dumps(one) == json.dumps(other)


def merge_objects(base: dict, ours: dict, theirs: dict, path: tuple,
                  conflicts: list[str]) -> dict:
    """Three-way merge of one JSON object, key by key. Objects recurse, every
    other value is one whole. A conflict keeps this side's state of the key and
    adds a line with the key's path to `conflicts`."""
    def where(key) -> str:
        return ".".join((*path, key))

    out = {}
    for key, mine in ours.items():
        if key not in theirs:
            if key not in base:
                out[key] = mine                 # added here
            elif not same(base[key], mine):
                out[key] = mine
                conflicts.append(f"{where(key)}: changed on this side, deleted on the other")
            continue                            # else: deleted there, untouched here
        other = theirs[key]
        if isinstance(mine, dict) and isinstance(other, dict) \
                and isinstance(base.get(key, {}), dict):
            out[key] = merge_objects(base.get(key, {}), mine, other, (*path, key), conflicts)
        elif same(mine, other) or (key in base and same(base[key], other)):
            out[key] = mine
        elif key in base and same(base[key], mine):
            out[key] = other
        else:
            out[key] = mine
            conflicts.append(f"{where(key)}: " + ("changed" if key in base else "added")
                             + " on both sides with two different values")
    for key, other in theirs.items():
        if key in ours:
            continue
        if key not in base:
            out[key] = other                    # added there
        elif not same(base[key], other):
            conflicts.append(f"{where(key)}: deleted on this side, changed on the other")
    return out


def merge(base_file: str, ours_file: str, theirs_file: str, config_path: str) -> int:
    files = [pathlib.Path(p) for p in (base_file, ours_file, theirs_file)]
    try:
        raws = [p.read_bytes() for p in files]
        cfgs = [json.loads(raw) for raw in raws]
        scripts = [script_of(cfg) for cfg in cfgs]
    except (OSError, ValueError, KeyError, TypeError) as exc:
        say(f"a side of {config_path} is not a config with a script: {exc!r}")
        return 1
    # ours, theirs, base: the side being merged INTO decides first.
    ensure_ascii = form_of(raws[1], raws[2], raws[0])
    if render(cfgs[1], ensure_ascii) != raws[1]:
        say(f"NOTE {config_path} is not in the tree's form on this side; "
            "the merge writes it in that form")

    merged_script, script_conflict = merge_texts(*scripts)
    # The script is settled above: the key merge sees one script on all three
    # sides, so the key stays where this side has it and never conflicts.
    field_conflicts: list[str] = []
    merged = merge_objects(*(with_script(cfg, "") for cfg in cfgs), (), field_conflicts)

    # On a script conflict the config keeps this side's script: one whole
    # script in a valid line, never a line with marks in it.
    script = scripts[1] if script_conflict else merged_script
    files[1].write_bytes(render(with_script(merged, script), ensure_ascii))

    waiting = conflict_path(config_path)
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
        say(f"fields of {config_path} conflicted:\n"
            + "".join(f"    {line}\n" for line in field_conflicts)
            + "The config carries no marks: it keeps this side's state of each of these "
            "keys, everything else is merged. Set them by hand in the config"
            + (" first." if script_conflict else f", then `git add {config_path}`."))
    return 1 if script_conflict or field_conflicts else 0


# -- the way back ----------------------------------------------------------------

def resolve(config_file: str, script_file: str) -> int:
    config, source = pathlib.Path(config_file), pathlib.Path(script_file)
    try:
        script = decode(source.read_bytes())
    except (OSError, ValueError) as exc:
        say(f"cannot read the resolved script {source}: {exc}")
        return 1
    left = [mark for mark in (OURS_MARK, THEIRS_MARK) if mark in script.splitlines()]
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
    config.write_bytes(render(with_script(cfg, script), form_of(raw)))
    try:
        inside = pathlib.Path(CONFLICT_DIR).resolve() in source.resolve().parents
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
                      (f"merge.{NAME}.driver", DRIVER_COMMAND)):
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
    if len(args) == 4 and not args[0].startswith("--"):
        return merge(*args)
    print(USAGE, file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv))
