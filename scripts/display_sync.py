#!/usr/bin/env python3
"""Keep the display template's copies in step (GH #669, three places; GH #753, the mark).

`compose/display-dna.css` is the source of the sheet; `compose.py` carries it as
`KIT_CSS`; `compose/config.json` carries `compose.py` as `params.script_inline`.
Run after every edit of the sheet or the script. Idempotent.

Since OR-G0.1 the copies do not carry the sheet's comments. The source keeps every
one of them -- that is the file a person reads, and the reasons belong to the reader
-- and what travels to a screen carries the rules plus the head comment, the sheet's
nameplate and the pointer back here. `display_sheet_strip.py` is that rule, and the
drift lock asks the same file rather than restating it, so the three copies can only
disagree in one place. It bought about 47 kB off every page (#735).

A fourth copy since display@2.5.0: the pass of `compose.py` is the reference model
`compose/scenarios/pass.py` byte for byte (display-hive.md section 0.7). `sync_pass.py`
writes it first, so the script that goes into `config.json` already carries the current
model.

Before any of that, the three scenario copies are fetched from the living description
tree and the commit they came from is written beside them as `SOURCE`: one line,
`meclaw-next <sha> <date>`. The drift lock reads that mark in a strand and resolves
`git show <sha>:23-display/model/<file>` instead of the tree's working copy, so a
strand gate no longer goes red because another session moved the description while the
gate ran -- six full strand gates and 5589 gate seconds went that way in one evening
(`plans/welle-p-2026-09-19/befund/01-zeit.md` section 3.2).

The last two steps -- what `script_inline` holds and how `config.json` is written back
-- live in `set_script_inline` and `dump_config` because the merge driver
`git_merge_display_sync.py` needs the same rule after a merge and must not spell it a
second time."""
import json
import os
import pathlib
import re
import subprocess
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from display_sheet_strip import strip  # noqa: E402

COMPOSE_ROOT = pathlib.Path(__file__).resolve().parents[1] / "templates" / "display" / "compose"
# Where the living description tree lies. The same name and the same default the drift
# lock uses -- one place answers "which tree", for the script and for the lock.
HIVE_ENV = "MECLAW_DISPLAY_HIVE"
HIVE_DEFAULT = "~/projeks/MeClaw/meclaw-next/23-display"
# The copy's name in the template on the left, the model's name on the right: the
# template already owns a `run_display_scenarios.py`, so the model's runner travels
# under a name that says whose runner it is.
COPIES = {
    "scenarios.json": "scenarios.json",
    "pass.py": "pass.py",
    "run_model.py": "run.py",
}


class SourceUnclean(Exception):
    """The living tree's model has edits that are in no commit.

    A mark promises that `git show <sha>:<model>/<file>` yields exactly the bytes
    beside it. Uncommitted bytes cannot be promised, so nothing is copied and no mark
    is written -- commit the description first."""


def living_tree():
    """The description directory as a path, whether or not it exists here."""
    return pathlib.Path(os.path.expanduser(os.environ.get(HIVE_ENV) or HIVE_DEFAULT))


def _git(cwd, *args):
    return subprocess.run(
        ["git", "-C", str(cwd), *args], check=True, capture_output=True, text=True
    ).stdout


def sync_source(hive, dest):
    """Copy the three model files into `dest` and write the mark `SOURCE`.

    `hive` is the description directory (the one holding `model/`). Returns the mark
    line, or `None` when the living tree is not here -- a foreign clone and ci have the
    copies but not their source, and must keep both the copies and the mark that
    travelled with them. Raises `SourceUnclean` when the model carries uncommitted
    edits."""
    model = pathlib.Path(hive) / "model"
    if not model.is_dir():
        return None
    try:
        if _git(model, "status", "--porcelain", "--", ".").strip():
            raise SourceUnclean("%s has uncommitted edits -- commit them first" % model)
        # The commit that last touched the MODEL, not the tree's newest one: the lock
        # resolves the sha against the model's own path, so the mark has to name the
        # commit those bytes belong to.
        head = _git(model, "log", "-1", "--format=%H %cs", "--", ".").split()
    except subprocess.CalledProcessError:
        return None  # there is a directory, but no repository around it
    if len(head) != 2:
        return None
    sha, date = head
    for here, there in COPIES.items():
        (pathlib.Path(dest) / here).write_bytes((model / there).read_bytes())
    mark = "meclaw-next %s %s\n" % (sha, date)
    (pathlib.Path(dest) / "SOURCE").write_text(mark, encoding="utf-8")
    return mark


def read_mark(dest):
    """The sha named by the mark `SOURCE` beside the copies, or `None`.

    Only the FORM is checked here (40 hex digits). Whether a tree knows that
    commit is the caller's question -- a clone that lags behind and a mark that
    was written beside the wrong bytes look identical at this level."""
    try:
        fields = (pathlib.Path(dest) / "SOURCE").read_text(encoding="utf-8").split()
    except OSError:
        return None
    for field in fields:
        if len(field) == 40 and all(c in "0123456789abcdef" for c in field):
            return field
    return None


def check_source(hive, dest):
    """The copies against the mark's commit: a list of drift lines, empty when
    they agree.

    The cheap half of the drift lock (GH #753), so the station
    `scenarios:display` can ask the question in seconds without compiling
    Rust. It is a NOTE and never a verdict -- a strand that goes red because
    another session moved the living description while its gate ran is exactly
    what this strand removed (six strand gates, 5589 s,
    `plans/welle-p-2026-09-19/befund/01-zeit.md` section 3.2).

    Two silences, and only two: no living tree here, and a tree that does not
    know the mark's commit yet. Both are a clone, not a drift. A MISSING mark
    is not one of them -- it travels with the copies, so its absence is a
    defect of this repository."""
    model = pathlib.Path(hive) / "model"
    sha = read_mark(dest)
    if sha is None:
        return ["%s/SOURCE names no commit -- run `python3 scripts/display_sync.py`"
                % dest]
    if not model.is_dir():
        return []
    try:
        _git(model, "cat-file", "-e", "%s^{commit}" % sha)
    except subprocess.CalledProcessError:
        return []
    drift = []
    for here, there in COPIES.items():
        try:
            want = subprocess.run(
                ["git", "-C", str(model), "show", "%s:./%s" % (sha, there)],
                check=True, capture_output=True,
            ).stdout
        except subprocess.CalledProcessError:
            drift.append("%s: the commit %s carries no %s" % (here, sha, there))
            continue
        if (pathlib.Path(dest) / here).read_bytes() != want:
            drift.append("%s differs from the model at %s" % (here, sha))
    return drift


def embed_sheet(py: str, travelling_sheet: str) -> str:
    """Put the travelling sheet into `compose.py`'s `KIT_CSS` literal.

    The sheet is embedded as a raw triple-quoted string: `KIT_CSS = r\"\"\"<sheet>\"\"\"`.
    """
    return re.sub(
        r'KIT_CSS = r"""(.*?)"""',
        lambda m: 'KIT_CSS = r"""' + travelling_sheet + '"""',
        py,
        count=1,
        flags=re.S,
    )


def set_script_inline(cfg: dict, compose_py: str) -> dict:
    """`config.json` carries the whole of `compose.py`, verbatim, as one string."""
    cfg["params"]["script_inline"] = compose_py
    return cfg


def dump_config(cfg: dict) -> str:
    """The one spelling of `config.json` on disk -- any other reformats the file."""
    return json.dumps(cfg, indent=2, ensure_ascii=False) + "\n"


def main(root: pathlib.Path = COMPOSE_ROOT) -> None:
    mark = sync_source(living_tree(), root / "scenarios")
    print("source: %s" % (mark.strip() if mark else "the living tree is not here, copies kept"))
    subprocess.run([sys.executable, str(root / "scenarios" / "sync_pass.py")], check=True)
    sheet = (root / "display-dna.css").read_text(encoding="utf-8")
    travels = strip(sheet)
    py_path = root / "compose.py"
    py = py_path.read_text(encoding="utf-8")
    new_py = embed_sheet(py, travels)
    if new_py != py:
        py_path.write_text(new_py, encoding="utf-8")
    cfg_path = root / "config.json"
    cfg = set_script_inline(json.loads(cfg_path.read_text(encoding="utf-8")), new_py)
    cfg_path.write_text(dump_config(cfg), encoding="utf-8")
    print("synced: sheet %d B, travelling %d B, script %d B"
          % (len(sheet), len(travels), len(new_py)))


def check_cli(dest):
    """`--check-source`: print the drift as NOTE lines, and exit 0 either way."""
    drift = check_source(living_tree(), dest)
    if not drift:
        print("source: the copies agree with their mark (or there is no tree to ask)")
        return 0
    for line in drift:
        print("NOTE: %s" % line)
    return 0


def cli(argv):
    """`display_sync.py` syncs; `display_sync.py --check-source [dir]` asks.

    Anything else is a usage error and exits 2 -- it does NOT fall through to
    the sync. Measured reason (wave P, fix round 1): while `--check-source` was
    still unimplemented, a test that called the flag ran the full sync instead,
    against the real template and with a throw-away tree as its source. An
    unknown argument must do nothing, not something."""
    if not argv:
        main()
        return 0
    if argv[0] == "--check-source" and len(argv) <= 2:
        return check_cli(pathlib.Path(argv[1]) if len(argv) == 2
                         else COMPOSE_ROOT / "scenarios")
    print("usage: display_sync.py [--check-source [<scenarios dir>]]", file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(cli(sys.argv[1:]))
