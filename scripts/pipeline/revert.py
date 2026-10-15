"""meclaw -- `pipeline.py revert <merge-sha|strand>`: taking a merge back, as an operation.

A revert is a merge in the other direction and goes through the same door:
the tree lease, a candidate outside the main tree (`git revert -m 1` of the
merge commit), the version step, the merge gate, a fast-forward of master.
Two things differ:

  * Versions. Reverting a template change would put the template.json back
    on a number master has already published. Numbers are never given twice
    (development-rules § 4), so every template the revert touches gets a NEW
    third-digit bump on top of master's current version, and the pins follow
    (the cascade of `versions.assign`).
  * Older merges (F2, OR-DT.W9.5). A later merge has moved the same
    version, pin and CHANGELOG lines, so `git revert` conflicts on exactly the
    lines the version step rewrites anyway. When EVERY conflict is such a
    bookkeeping line (both sides equal once the x.y.z numbers are masked, or
    CHANGELOG.md, or a purpose line that differs only by appended `Since`
    sentences, or the generated corpus -- ORCH.18: its chunks shift with
    every Since sentence, and the version step regenerates it from master's
    copy, as a merge does), the revert is redone with `-X ours` -- master
    keeps those lines, everything else is taken back -- and `versions.assign`
    renumbers from master. Any other conflict refuses with the paths and the
    way out.
  * The record. `merges` -> `reverted`, event `revert_done`, the strand's
    issue is reopened with a comment, and a receipt lands in the wave's
    archive under `receipts/revert-<S>/`. Target: <= 600 s with the receipt.
"""

import datetime
import os
import re
import subprocess
import sys
import time

from . import events, merge as m, store, versions


def find(db, root, target, repo="meclaw"):
    """The `merges` row of a merge sha or a strand name (newest merged)."""
    rows = [dict(r) for r in db.execute(
        "SELECT * FROM merges WHERE repo=? AND state='merged' AND (strand=? OR merge_sha LIKE ?)"
        " ORDER BY at DESC", (repo, target, target + "%"))]
    if rows:
        return rows[0]
    sha = m.out(root, "rev-parse", "--verify", "--quiet", "%s^{commit}" % target)
    if sha and len(m.out(root, "rev-list", "--parents", "-n", "1", sha).split()) >= 3:
        subject = m.out(root, "log", "-1", "--format=%s", sha)
        strand = subject.split()[1].rstrip(",") if subject.startswith("Merge ") else target
        return {"id": None, "strand": strand, "merge_sha": sha, "base": sha + "^1"}
    raise m.Refused("no_merge", "no merged strand or merge commit %s" % target)


_NUMBER = re.compile(r"\d+\.\d+\.\d+")
GUIDANCE = ("a later merge changed the same content: revert the later merges first, newest "
            "first, or take the change back by hand in a fix strand")


def _blocks(text):
    """`[(ours, theirs)]` line lists of every conflict block in `text`."""
    blocks, side, ours, theirs = [], None, [], []
    for line in text.splitlines():
        if line.startswith("<<<<<<< "):
            side, ours, theirs = "ours", [], []
        elif side and line.startswith("||||||| "):
            side = "base"
        elif side and line == "=======":
            side = "theirs"
        elif side and line.startswith(">>>>>>> "):
            blocks.append((ours, theirs))
            side = None
        elif side == "ours":
            ours.append(line)
        elif side == "theirs":
            theirs.append(line)
    return blocks


CHANGELOG = "CHANGELOG.md"
_TEMPLATE_VERSIONS = "- **Template versions"
_SECTION_END = re.compile(r"#{1,3} ")


def own_lines(tree, sha, rel=CHANGELOG):
    """`(added, removed)` lines of the merge `sha` on `rel` against its first parent."""
    diff = m.out(tree, "diff", "-U0", "--no-color", "%s^1" % sha, sha, "--", rel).splitlines()
    added = [l[1:] for l in diff if l.startswith("+") and not l.startswith("+++ ")]
    removed = [l[1:] for l in diff if l.startswith("-") and not l.startswith("--- ")]
    return added, removed


def _changelog_ok(blocks, sha, tree):
    """A CHANGELOG conflict is bookkeeping only when the merge just appended
    lines and master's side keeps every line of the other side (template
    version lines aside): `-X ours` then loses nothing, and `drop_own_lines`
    takes the merge's own lines out."""
    if sha is None or own_lines(tree, sha)[1]:
        return False
    for ours, theirs in blocks:
        if any(l.strip() and l not in ours and not l.startswith(_TEMPLATE_VERSIONS)
               for l in theirs):
            return False
    return True


def drop_own_lines(tree, sha):
    """Take the merge's own CHANGELOG entries (`- ` lines it added, and a
    `### ` heading of its own left empty) out of the tree -- `-X ours` kept
    them where a later merge appended next to them."""
    path = os.path.join(tree, CHANGELOG)
    with open(path, encoding="utf-8") as fh:
        lines = fh.read().split("\n")
    added = own_lines(tree, sha)[0]
    drop = set()
    for line in added:
        if line.startswith("- "):
            k = next((k for k, l in enumerate(lines) if l == line and k not in drop), None)
            if k is not None:
                drop.add(k)
    # N1: a `### ` heading the merge added goes too once nothing but blank
    # lines stand under it; one that carries someone else's entry stays.
    # Review M1: only `#`..`###` ends a section -- CHANGELOG.md carries foreign
    # `#### ` subsections under a `### ` heading -- and the heading is the one
    # above the merge's own entries, not the first equal line in the file.
    for k in sorted(drop):
        i = k - 1
        while i >= 0 and not _SECTION_END.match(lines[i]):
            i -= 1
        if i < 0 or i in drop or not lines[i].startswith("### ") or lines[i] not in added:
            continue
        j = i + 1
        while j < len(lines) and (j in drop or not lines[j].strip()):
            j += 1
        if j == len(lines) or _SECTION_END.match(lines[j]):
            drop.update(range(i, j) if j < len(lines) else (i,))
    lines = [l for k, l in enumerate(lines) if k not in drop]
    with open(path, "w", encoding="utf-8") as fh:
        fh.write("\n".join(lines))


def bookkeeping_only(tree, paths, sha=None):
    """True when every conflict in `paths` is a line the version step rewrites
    (in CHANGELOG.md: see `_changelog_ok`; `sha` is the merge taken back)."""
    for rel in paths:
        try:
            with open(os.path.join(tree, rel), encoding="utf-8") as fh:
                blocks = _blocks(fh.read())
        except (OSError, UnicodeDecodeError):
            return False
        if not blocks:
            return False
        if rel == CHANGELOG:
            if not _changelog_ok(blocks, sha, tree):
                return False
            continue
        if rel in m.CORPUS:
            continue
        for ours, theirs in blocks:
            ours = [_NUMBER.sub("X", l) for l in ours]
            theirs = [_NUMBER.sub("X", l) for l in theirs]
            if ours != theirs and not _history_only(ours, theirs):
                return False
    return True


def _history_only(ours, theirs):
    """Lines that differ only by `Since x.y.z (` sentences one side
    appended (F5: a later merge's history on the same purpose line). Master's
    side keeps them: they are what each published number shipped with."""
    return len(ours) == len(theirs) and all(
        o == t or _appended_history(o, t) for o, t in zip(ours, theirs))


def _appended_history(one, other):
    a, b = sorted((one, other), key=len)
    pre = 0
    while pre < len(a) and a[pre] == b[pre]:
        pre += 1
    post = 0
    while post < len(a) - pre and a[-1 - post] == b[-1 - post]:
        post += 1
    return b[pre:len(b) - post].startswith(" Since X (") and pre + post == len(a)


def take_back(cand, sha):
    """`git revert -m 1 --no-commit` of the merge in the candidate (F2 above)."""
    res = m.git(cand, "revert", "-m", "1", "--no-commit", sha)
    if res.returncode == 0:
        return
    paths = m.out(cand, "diff", "--name-only", "--diff-filter=U").split()
    ok = bool(paths) and bookkeeping_only(cand, paths, sha)
    m.git(cand, "revert", "--abort")
    if not ok:
        raise m.Refused("revert_conflict", "%s -- %s" % (", ".join(paths) or res.stderr.strip()[:200],
                                                         GUIDANCE))
    res = m.git(cand, "revert", "-m", "1", "-X", "ours", "--no-commit", sha)
    if res.returncode != 0:
        m.git(cand, "revert", "--abort")
        raise m.Refused("revert_conflict", "%s -- %s" % (res.stderr.strip()[:200], GUIDANCE))
    # The corpus is regenerated, never reverted: master's copy is what the
    # version step regenerates from (`merge.regenerate_corpus`).
    for c in paths:
        if c in m.CORPUS:
            m.git(cand, "checkout", "HEAD", "--", c, check=True)
    if CHANGELOG in paths:
        drop_own_lines(cand, sha)


def revert(target, reason, repo="meclaw", start=None, wave=None, host=None, db=None,
           reopen=True, out_=None, err=None, clock=time.monotonic):
    out_, err = out_ or sys.stdout, err or sys.stderr
    t0 = clock()
    root = m.main_root(start or os.getcwd())
    own_db = db is None
    db = db or store.open()
    row = find(db, root, target, repo)
    strand = row["strand"]
    holder = "revert:%s" % strand
    try:
        taken = m.take_tree(db, repo, holder)
    except m.Refused as exc:
        err.write("revert: refused (%s): %s\n" % (exc.reason, exc.text))
        taken = None
    if not taken:
        if taken is False:
            err.write("revert: tree:master is held -- queued behind it\n")
        if own_db:
            store.close(db)
        return m.QUEUED_EXIT if taken is False else m.REFUSE_EXIT
    base = m.out(root, "rev-parse", "master")
    cand = None
    term = m.trap_term()
    try:
        s = m.Strand(root, strand, wave)
        label = "revert-%s" % strand
        cand = m.open_candidate(root, base, label)
        take_back(cand, row["merge_sha"])
        changed = m.out(cand, "diff", "--name-only", "--cached", base).split()
        # Every touched template gets a fix kind, numbered by the merge point's
        # rule (third digit once per release, else held at master's number) --
        # the revert's own template.json lines would renumber.
        result = versions.assign(cand, base, changed, {}, repo=root,
                                 before_round=m.regenerate_corpus,
                                 history={"issues": s.issues, "revert": True})
        line = versions.changelog_line(result, s.issues)
        versions.add_changelog(cand, line and line.replace("Template versions", "Template versions, revert"))
        m.git(cand, "add", "-u", check=True)
        msg = "Revert %s (%s)\n\n%s\n%s\n" % (strand, reason, versions.version_line(result),
                                              versions.pins_line(result))
        m.git(cand, "commit", "-q", "-m", msg, check=True)
        sha = m.out(cand, "rev-parse", "HEAD")
        verdict = m.run_gate(cand, {"rev": sha, "base": base, "label": label,
                                    "strands": [strand], "cache_from": [], "host": host})
        out_.write("%s\n" % verdict["line"])
        if verdict["verdict"] != "GREEN":
            raise m.Refused("gate_red" if verdict["verdict"] == "RED" else "gate_aborted",
                            verdict["line"])
        m.hold_tree(db, repo, holder)
        m.fast_forward(root, base, sha)
        secs = round(clock() - t0, 1)
        if row.get("id"):
            m._set_merge(db, row["id"], state="reverted")
        events.emit(db, "revert_done", "status", {
            "merge": row.get("id") or row["merge_sha"], "strand": strand, "base": base,
            "merge_sha": sha, "versions": result["versions"], "pins": result["pins"],
            "reason": reason})
        comment = ("Reverted by %s (%s): %s. %s." % (sha[:9], reason, versions.version_line(result),
                                                     "the merge was %s" % row["merge_sha"][:9]))
        if reopen:
            gh = os.environ.get("MECLAW_GH", "gh")
            for issue in s.issues:
                subprocess.run([gh, "issue", "reopen", issue, "--comment", comment])
        d = m.write_receipt(s.wdir, label, sha, {
            "strand": strand, "reverted": row["merge_sha"], "revert_sha": sha, "base": base,
            "reason": reason, "versions": result["versions"], "pins": result["pins"],
            "gate": verdict, "secs": secs, "issues": s.issues, "comment": comment,
            "at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
            "summary": "REVERT %s %s %ss GREEN" % (strand, sha[:9], secs)})
        out_.write("REVERTED %s %s %ss receipt=%s\n" % (strand, sha[:9], secs, d))
        return 0
    except (m.Refused, versions.VersionError) as exc:
        code = getattr(exc, "reason", "version_conflict")
        err.write("revert: refused (%s): %s\n" % (code, exc))
        return m.REFUSE_EXIT
    finally:
        if cand:
            m.close_candidate(root, cand)
        m.release_tree(db, repo, holder)
        m.untrap_term(term)
        if own_db:
            store.close(db)


def _cmd_revert(args):
    try:
        return revert(args.target, args.reason, repo=args.repo, wave=args.wave, host=args.host,
                      db=store.open(args.db) if args.db else None, reopen=not args.no_reopen)
    except m.Refused as exc:
        print("revert: refused (%s): %s" % (exc.reason, exc.text), file=sys.stderr)
        return m.REFUSE_EXIT


def register(sub):
    p = sub.add_parser("revert", help="revert a merge over the tree lease (new bump, receipt)")
    p.add_argument("target", help="the merge sha or the strand")
    p.add_argument("--reason", required=True)
    p.add_argument("--wave")
    p.add_argument("--repo", default="meclaw")
    p.add_argument("--host")
    p.add_argument("--no-reopen", action="store_true")
    p.add_argument("--db")
    p.set_defaults(func=_cmd_revert)
