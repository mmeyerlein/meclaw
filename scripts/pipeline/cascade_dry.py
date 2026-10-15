"""meclaw -- `cascade-dry`: the merge point's version cascade, run dry in `pre` (GH #1173).

WHY THIS EXISTS
===============
The version cascade (`versions.assign`) only runs inside the merge queue.
The strand gate and `pre` never saw what it leaves behind, so a candidate
went red in the queue only: a queue run on a candidate of strand tb-s8 failed
7 tests for two reasons, with the strand gate and `pre` green --

* a version literal in a `.rs` test next to a template path (`"2.10.8"`
  asserted for `templates/display`) stayed at the old version after the bump;
* a pin the cascade regex does not match (an escaped pin inside a JSON
  string, `\\"display@2.10.8\\"`) stayed at the old version.

WHAT IT DOES
============
When the diff touches `templates/` (the only paths the merge point numbers;
the rule is fail-closed: any such path counts, a template or not), the check
copies the working tree's cascade surfaces (`versions.SURFACES` and
`versions.CODE_SURFACES`, tracked and untracked-not-ignored files) into a
throwaway directory, runs `versions.assign` there against the base exactly
like the queue (no commit, no history sentences, no corpus) and reports

* every `<moved name>@<old version>` left in a file the cascade owns or a
  template file, outside the history records (`docs/superpowers/plans/`,
  Markdown under `workshop/`, the generated corpus, a sentence that starts
  `Since <x.y.z> (`, a Rust `//` comment);
* every string literal `"<old version>"` of a moved template in a `.rs`
  file that also names the template's path (`templates/<dir>`).

Each finding is RED with one line. A diff without a `templates/` path is
GREEN at no cost (`skipped`). The tree itself is never written.
"""

import os
import re
import shutil
import subprocess
import tempfile

from . import locks, versions

CHECK = "cascade-dry"
TOPS = tuple(dict.fromkeys(versions.SURFACES + versions.CODE_SURFACES))
HISTORY_PREFIXES = versions.FROZEN
GENERATED = ("docs.jsonl",)
_SINCE = re.compile(r"Since [0-9]+\.[0-9]+\.[0-9]+ \(")
_SENTENCE_END = re.compile(r"[.!?]\s")


def relevant(paths):
    """Fail-closed: every diff path under `templates/` may move a number."""
    return any(p.startswith("templates/") for p in paths)


def _git_files(repo, tops):
    res = subprocess.run(["git", "-C", repo, "ls-files", "-z", "-c", "-o", "--exclude-standard",
                          "--"] + list(tops), capture_output=True)
    if res.returncode != 0:
        raise OSError("git ls-files failed: %s" % res.stderr.decode(errors="replace").strip())
    return sorted({p for p in res.stdout.decode("utf-8", "replace").split("\0") if p})


def candidate(repo, into):
    """Copy the cascade surfaces of `repo`'s working tree into `into`."""
    for rel in _git_files(repo, TOPS):
        src = os.path.join(repo, rel)
        if not os.path.isfile(src) or os.path.islink(src):
            continue
        dst = os.path.join(into, rel)
        os.makedirs(os.path.dirname(dst), exist_ok=True)
        shutil.copyfile(src, dst)


def _scanned(rel):
    """The files a leftover pin matters in: the cascade's own classes, and
    every file of a template (a script there renders the pin)."""
    if rel.startswith(HISTORY_PREFIXES) or os.path.basename(rel) in GENERATED:
        return False
    top = rel.split("/", 1)[0]
    if top == "templates":
        return True
    if rel.endswith(".json"):
        return top in versions.SURFACES
    if rel.endswith(".rs"):
        return top in versions.CODE_SURFACES
    if rel.endswith(".md"):
        return top in versions.PROSE_SURFACES
    return False


def _history(line, start, rel):
    if rel.endswith(".rs") and "//" in line[:start]:
        return True
    head = line[:start]
    ends = [m.end() for m in _SENTENCE_END.finditer(head)]
    return bool(_SINCE.search(head[ends[-1] if ends else 0:]))


def leftovers(tree, moved, dirs):
    """Findings in the cascaded `tree`; `moved` = `{name: (old, new)}`,
    `dirs` = `{dir: name}`."""
    if not moved:
        return []
    alt = "|".join(re.escape(n) for n in sorted(moved, key=len, reverse=True))
    pin_rx = re.compile(r"(?<![A-Za-z0-9_-])(%s)@(%s)(?![0-9.]*[0-9])" % (alt, locks._VERSION))
    by_name = {n: d for d, n in dirs.items()}
    out = []
    for rel in _git_tree_files(tree):
        if not _scanned(rel):
            continue
        try:
            with open(os.path.join(tree, rel), encoding="utf-8") as fh:
                lines = fh.read().split("\n")
        except (OSError, UnicodeDecodeError):
            continue
        text = "\n".join(lines)
        named = [n for n in moved if rel.endswith(".rs")
                 and "templates/%s" % by_name.get(n, n) in text]
        for no, line in enumerate(lines, 1):
            for m in pin_rx.finditer(line):
                name, ver = m.group(1), m.group(2)
                if ver != moved[name][0] or _history(line, m.start(), rel):
                    continue
                out.append(locks.Finding(
                    "RED", CHECK, rel, no,
                    "%s@%s stays at the old version after the merge point's cascade "
                    "(-> %s@%s)" % (name, ver, name, moved[name][1]),
                    "write the pin in a form the cascade raises (`\"%s@x.y.z\"`) "
                    "or read the version from the template" % name))
            for name in named:
                lit = '"%s"' % moved[name][0]
                if lit in line and not _history(line, line.index(lit), rel):
                    out.append(locks.Finding(
                        "RED", CHECK, rel, no,
                        "literal %s next to templates/%s stays after the merge point "
                        "numbers %s %s -> %s" % (lit, by_name.get(name, name), name,
                                                  moved[name][0], moved[name][1]),
                        "read the version from templates/%s/template.json instead"
                        % by_name.get(name, name)))
    return out


def _git_tree_files(tree):
    for top in TOPS:
        root = os.path.join(tree, top)
        for dirpath, dirnames, filenames in os.walk(root):
            dirnames[:] = sorted(d for d in dirnames if d not in locks.SKIP_DIRS)
            for fn in sorted(filenames):
                yield os.path.relpath(os.path.join(dirpath, fn), tree).replace(os.sep, "/")


def check(paths, repo, ctx=None):
    """`(findings, note)`: the dry cascade of `repo`'s diff `paths` against
    `ctx["base"]`. `note` is the skip reason, else None."""
    if not relevant(paths):
        return [], "skipped: no templates/ path in the diff"
    base = (ctx or {}).get("base") or locks.merge_base(repo)
    if not base:
        return [locks.Finding("RED", CHECK, "", 0, "no merge base to number against",
                              "fetch master, then run pre again")], None
    with tempfile.TemporaryDirectory(prefix="cascade-dry-") as tmp:
        try:
            candidate(repo, tmp)
            res = versions.assign(tmp, base, list(paths), {}, repo=repo)
        except (OSError, versions.VersionError) as exc:
            return [locks.Finding("RED", CHECK, "", 0, "the dry cascade failed: %s" % exc,
                                  "fix the cause; the merge queue would fail the same way")], None
        moved = {n: (v["from"], v["to"]) for n, v in res["versions"].items()}
        return leftovers(tmp, moved, versions.template_dirs(tmp)), None
