"""Git's repository variables and the guard of the repository config (pd-1155).

A caller that set `GIT_DIR`/`GIT_WORK_TREE` (`strand.sh` without `cd`) made
every git call below it act on that repository: the test repositories of the
Python suite ran `git init` (core.worktree), `git config user.name t` and
`git commit` against the shared config and the branch of the tree under test.
Stations and the unittest runs drop these variables, and the runs compare the
repository config before and after -- any difference is RED.
"""

import collections
import os
import re
import subprocess
import sys

# `git rev-parse --local-env-vars`
GIT_REPO_VARS = ("GIT_ALTERNATE_OBJECT_DIRECTORIES", "GIT_CONFIG", "GIT_CONFIG_PARAMETERS",
                 "GIT_CONFIG_COUNT", "GIT_OBJECT_DIRECTORY", "GIT_DIR", "GIT_WORK_TREE",
                 "GIT_IMPLICIT_WORK_TREE", "GIT_GRAFT_FILE", "GIT_INDEX_FILE",
                 "GIT_NO_REPLACE_OBJECTS", "GIT_REPLACE_REF_BASE", "GIT_PREFIX",
                 "GIT_SHALLOW_FILE", "GIT_COMMON_DIR")


def without_repo_env(env):
    """`env` without git's repository variables: a run's git is its tree's."""
    return {k: v for k, v in env.items() if k not in GIT_REPO_VARS}


def enter_dirs(dirs, prog):
    """`os.chdir` through `dirs` in order, as `git -C` does with each `-C`:
    a relative path from the directory before. A directory that is not one is
    `<prog>: -C: no such directory: <dir>` and exit 2 (GH #1199 (b)).

    Why: an agent shell resets its cwd on every call, and a kit entry that
    reads its tree from the cwd was reachable from there only through a
    wrapper script (wave validation 2, strand v2-p1078)."""
    for path in dirs or ():
        if not os.path.isdir(path):
            sys.stderr.write("%s: -C: no such directory: %s\n" % (prog, path))
            raise SystemExit(2)
        try:
            os.chdir(path)
        except OSError as exc:
            sys.stderr.write("%s: -C: cannot enter %s: %s\n" % (prog, path, exc.strerror))
            raise SystemExit(2)


def take_leading_dirs(argv, prog):
    """`argv` without its leading `-C <dir>` pairs, after entering them
    (`enter_dirs`) -- the form of an entry with subcommands, where `-C`
    stands before the command (`pipeline.py -C <dir> <cmd> ...`)."""
    argv, dirs = list(argv), []
    while argv and argv[0] == "-C":
        if len(argv) < 2:
            sys.stderr.write("%s: -C needs a directory\n" % prog)
            raise SystemExit(2)
        dirs.append(argv[1])
        argv = argv[2:]
    enter_dirs(dirs, prog)
    return argv


# THE STRAND OF A CALL (GH #1199 f). The strand came from the branch alone in
# every entry that binds a token, a lease or a receipt -- in a detached tree
# (no branch) `pipeline.py test --strand S` was refused with exit 3 although
# its lease was held, and strand S2 of wave validation 2 made a branch by hand
# (OR-V2.S2.2). One chain for every entry: `--strand` > the branch
# `<wave>/<name>` > a refusal that names the way. Its shell twin is
# `strand.sh strand_of_branch`, with the same two texts.
DETACHED = "detached HEAD: pass --strand <S>"
NO_STRAND = "cannot tell the strand from the branch -- pass --strand"

Strand = collections.namedtuple("Strand", "name key why")


def strand_of(branch, strand="", wave=""):
    """`Strand(name, key, why)` of a call: `name` the strand (`<S>`), `key` its
    lease holder `<wave>/<S>` (the bare `<S>` when no wave is known), `why`
    the refusal when neither `--strand` nor the branch names one ("" else).

    `branch` is the tree's branch as the caller read it: "HEAD" (`rev-parse
    --abbrev-ref` on a detached HEAD) and "" count as detached, None as "no
    branch known"."""
    branch = "" if branch == "HEAD" else branch
    if strand:
        if "/" in strand:
            return Strand(strand.rsplit("/", 1)[-1], strand, "")
        if wave:
            w = re.sub(r"-\d{4}-\d{2}-\d{2}$", "", wave.rstrip("/").split("/")[-1])
        elif branch and "/" in branch:
            w = branch.split("/", 1)[0]
        else:
            return Strand(strand, strand, "")
        return Strand(strand, "%s/%s" % (w, strand), "")
    if branch and "/" in branch:
        return Strand(branch.rsplit("/", 1)[-1], branch, "")
    return Strand("", "", DETACHED if branch == "" else NO_STRAND)


def enter_tree():
    """Neutralize an inherited repository once, at the kit's entry.

    With `GIT_DIR`/`GIT_WORK_TREE` set (`strand.sh` without `cd`), every
    `git -C <tree>` below loses against them: the merge's candidate ran its
    `merge` and `commit` in the caller's tree (re-check pd-legacy M-1). The
    tree they name becomes the working directory, then they are dropped.
    Returns that tree, or None when nothing was set or it has no work tree."""
    if not any(name in os.environ for name in GIT_REPO_VARS):
        return None
    res = subprocess.run(["git", "rev-parse", "--show-toplevel"], capture_output=True,
                         text=True)
    top = res.stdout.strip() if res.returncode == 0 else ""
    if top:
        os.chdir(top)
    for name in GIT_REPO_VARS:
        os.environ.pop(name, None)
    return top or None


def config_path(root):
    """The shared config file of the repository at `root`, or None."""
    res = subprocess.run(["git", "-C", str(root), "rev-parse", "--path-format=absolute",
                          "--git-common-dir"], capture_output=True, text=True,
                         env=without_repo_env(os.environ))
    if res.returncode != 0 or not res.stdout.strip():
        return None
    return os.path.join(res.stdout.strip(), "config")


def config_entries(path):
    """The `key=value` lines of a config file (`git config --list`), or None."""
    if not path or not os.path.isfile(path):
        return None
    res = subprocess.run(["git", "config", "--file", path, "--list"], capture_output=True,
                         text=True, env=without_repo_env(os.environ))
    return res.stdout.splitlines() if res.returncode == 0 else ["<unreadable>"]


# Keys a legitimate parallel git writes into the config all worktrees share
# (`git branch -u`, `push -u`, `remote set-url`): a change there is a NOTE
# with its key, never RED for a foreign run (re-check pd-legacy m-4).
NOTE_KEYS = ("branch.", "remote.")


def _noted(entry):
    return entry.startswith(NOTE_KEYS)


def config_drift(path, before, noted=False):
    """'' when the config at `path` still lists `before`, else the reason:
    `the suite changed the repository config <path>: +k=v, -k=v`. Only the
    keys outside `NOTE_KEYS`, or with `noted` only those inside."""
    after = config_entries(path)
    if before is None or after == before:
        return ""
    old = [e for e in before if _noted(e) == noted]
    now = [e for e in (after or []) if _noted(e) == noted]
    if now == old:
        return ""
    change = ", ".join(["+" + e for e in now if e not in old]
                       + ["-" + e for e in old if e not in now]) or "order"
    if noted:
        return "the repository config %s changed beside the run: %s" % (path, change)
    return "the suite changed the repository config %s: %s" % (path, change)


def _rev(root, *args):
    res = subprocess.run(["git", "-C", str(root)] + list(args), capture_output=True, text=True,
                         env=without_repo_env(os.environ))
    return res.stdout.strip() if res.returncode == 0 else ""


def snapshot(root):
    """What a run must leave as it was: the shared config, and the commit and
    the branch of the tree at `root` (re-check pd-legacy m-3: a test commit
    "first" landed on the strand's branch). Only this tree's HEAD, never all
    refs -- parallel strands move theirs."""
    path = config_path(root)
    return {"root": str(root), "path": path, "config": config_entries(path),
            "head": _rev(root, "rev-parse", "-q", "--verify", "HEAD"),
            "ref": _rev(root, "symbolic-ref", "-q", "HEAD")}


def drift(snap):
    """(red, note) of a run against its `snapshot`: `red` names what the run
    changed in the repository (the config outside `NOTE_KEYS`, the commit or
    the branch of the tree), `note` the changed keys a parallel git may write;
    '' each when nothing moved."""
    red = [config_drift(snap["path"], snap["config"])]
    head = _rev(snap["root"], "rev-parse", "-q", "--verify", "HEAD")
    ref = _rev(snap["root"], "symbolic-ref", "-q", "HEAD")
    if head != snap["head"]:
        red.append("the suite moved HEAD of %s: %s -> %s" % (snap["root"], snap["head"][:12] or "none",
                                                           head[:12] or "none"))
    if ref != snap["ref"]:
        red.append("the suite switched the branch of %s: %s -> %s" % (
            snap["root"], snap["ref"] or "detached", ref or "detached"))
    return "; ".join(r for r in red if r), config_drift(snap["path"], snap["config"], noted=True)
