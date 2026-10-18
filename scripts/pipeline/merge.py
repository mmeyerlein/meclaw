"""meclaw -- `pipeline.py merge` and `close`: the one operation that moves master.

WHY THIS EXISTS
===============
Merges were prose: "tree free" on the coordinator's word, numbers assigned by
hand, pins found by a red scenario twenty-five minutes later (GH #1108), and a
close comment that listed fifty commits of other strands (GH #1107). A merge
is now a task with a resource lease, a version step and a tested candidate
(bors' not-rocket-science rule: master is always green because the CANDIDATE
is tested, not the branch):

    1. lease `tree:master@<repo>` (TTL 2 h). Held by somebody else -> the merge
       is `queued` (exit 3); the release OFFERS the lease to the oldest queued
       merge, which takes it on its next call -- or at once with `--wait`,
       which blocks on the pool's wake file the release rings and says its
       reason and queue place at the start and on each change (`wait_s` in
       the merge receipt, V2-A10). With `--wait` the candidate gate waits for
       its lane too (`gate --wait-admission`, K13); without it a lane it
       cannot have ends the merge at once (ABORTED, no queue place). A merge
       that waits holds no lane (OR-TM-6): each wait round gives a lane lease
       of its gate holder back (`give_back_lanes`; one whose run lives stays
       until the run ends), and the candidate gate takes its lane only under
       the lease, ahead of the waiting strands (K14) -- a `token take` before
       `merge --wait` (workaround K13-3) is superfluous.
    2. preconditions: the newest strand receipt is GREEN on the branch head
       (its base may be older than master -- that is what the merge gate is
       for), the strand's pre stamp is fresh, the report has its head block
       and a `## Review` section whose last verdict line says yes
       (`REVIEW_VERDICT`, `no_review_verdict`), and the line
       `Outsourced findings: none` or `... #<n> (owner-ruling <reference>)`
       (`OUTSOURCED`, `no_outsourced_findings`, GH #1196). Missing ->
       `merge_refused` with the reason.
    3. candidate: a detached worktree of the main tree OUTSIDE `.git` and
       outside the tree (`~/.local/state/meclaw/pipeline/merge/<S>`),
       `git merge --no-ff --no-commit <branch>`; a conflict refuses -- except
       in the generated corpus, which is never merged but regenerated, and
       pure additions under `## [Unreleased]` of CHANGELOG.md (OR-TM-7: a list
       conflict cost a strand a full re-gate): both sides stay, master first,
       a section both open stands once, a heading twice stays two sections
       (`unreleased_union`), and the merge says so in one line --
       `merge: resolved ...`, `resolved` in the receipt and the
       journal. A changed or dropped line, a conflict outside that body or in
       any other file, and any conflict of a batch still refuse. Then every
       released `## [x.y.z]` of CHANGELOG.md must read as on master
       (`keep_released`, OR-TM-12: a strand branched before a release cut
       merged without a conflict, and git set its lines into the released
       section): pure additions there move under `[Unreleased]` (master
       first, into the section of the same heading and occurrence, the same
       resolution line), any other change refuses `released_changed`, naming
       the cause.
    4. versions + pin cascade (`versions.assign`, kinds from
       `plans/<wave>/straenge/<S>.json`), the corpus when docs moved, the
       `[Unreleased]` line, `ROADMAP.md` § Now without the strand's issues;
       commit `Merge <S> (#n): <title>` with the version and pin lines.
       When the merge point's own writes touch the closure of a hash golden
       (`GOLDENS`), the golden's generator runs on the candidate and the
       commit is amended with what it rewrote (step `golden`, V2-A17).
    5. merge gate on the committed candidate. GREEN -> `git merge --ff-only`
       in the main tree (moves master AND its working tree; never
       `update-ref`, which would leave the checked-out tree behind as a
       backwards diff), `merges` -> `merged`, `merge_done`, lease free, issue
       close (`close`). RED/ABORTED -> candidate discarded, `merge_refused`,
       lease free, the strand gets the finding.

Each step ends with a line `merge: step <name> <start> <secs>s` (lease,
preconditions, candidate, gate, ref-update, close; ` RED` after the one that
refused) and the merge receipt lists them as `steps` (`Steps`, V2-A10 h).

`--batch S1,S2` puts several strands into ONE octopus candidate; a red batch
bisects (halves as candidates, at most three levels). This module never
pushes: the export stays `make_export.py`.

THE GATE HOOK
=============
`run_gate(tree, ctx)` is the one narrow interface to the gate: the engine
(`gate.sh strand --base <master>`, with `--host <lane>` on that lane -- the
engine pushes the candidate itself and refuses a foreign overlay, GH #1141),
or `MECLAW_MERGE_GATE_CMD`. Either way the content-keyed cache the strand run
filled answers every station whose inputs the merge point left alone; the
strand runs travel as `MECLAW_GATE_CACHE_FROM` (`ctx["cache_from"]`), and the
plan line counts the hits they answered (`offer=`).
`MECLAW_PIPELINE_FAKE_GATE=green|red|aborted|red-if:<S>,..` answers without
running anything (the tests). Fail closed: a verdict counts only when the
lane push and the `GATE-SUMMARY` line name the candidate's own rev; any other
rev refuses the merge (`gate_foreign_rev`).
"""

import datetime
import importlib.util
import io
import json
import os
import re
import select
import shlex
import shutil
import signal
import sqlite3
import subprocess
import sys
import time
import uuid

from . import events, gitenv, lifted, schedule, store, turnaround, versions
from .lane import GOLDEN_SWITCHES, python_argv
from .register import main_branch

TTL_S = 7200
# An offer is short: a queued caller that does not come back within it (a
# crashed or abandoned run) loses its row instead of locking the tree for TTL_S.
OFFER_TTL_S = 600
REFUSE_EXIT, QUEUED_EXIT = 1, 3
CORPUS = ("templates/builder-librarian/store/seed/docs.jsonl",)
CORPUS_TOOL = "workshop/tools/build_librarian_seed.py"
MAX_BISECT = 3

# The register of owner rulings that open a testing ban (VORGABEN § 5.4).
TESTING_REGISTER = ".config/testing-rulings.toml"


class Refused(Exception):
    """A merge that does not happen; `reason` is the journal's code."""

    def __init__(self, reason, text=""):
        super().__init__(text or reason)
        self.reason = reason
        self.text = text or reason
        self.reasons = [reason]

    @classmethod
    def all_of(cls, missing):
        """ONE refusal for every missing precondition (GH #1167 c): the first
        reason is the journal's code, the text lists them all."""
        if len(missing) == 1:
            return cls(*missing[0])
        exc = cls(missing[0][0], "%d preconditions missing: %s" % (
            len(missing), " | ".join("[%s] %s" % m for m in missing)))
        exc.reasons = [m[0] for m in missing]
        return exc


# --- git and paths ------------------------------------------------------------------

def git(cwd, *args, check=False, env=None):
    """`git -C cwd <args>` of the merge point and `revert`, rerere off (OR-TM-11):
    a merge decides from its own conflict, never from a recorded resolution."""
    res = subprocess.run(gitenv.git_argv(cwd, *args), capture_output=True, text=True,
                         env=env)
    if check and res.returncode != 0:
        raise Refused("git_failed", "git %s: %s" % (" ".join(args[:3]), res.stderr.strip()[:300]))
    return res


def out(cwd, *args):
    res = git(cwd, *args)
    return res.stdout.strip() if res.returncode == 0 else ""


def main_root(start):
    common = out(start, "rev-parse", "--path-format=absolute", "--git-common-dir")
    if not common:
        raise Refused("no_repository", "%s is not in a git repository" % start)
    return os.path.dirname(common.rstrip("/"))


def state_dir():
    base = os.environ.get("XDG_STATE_HOME") or os.path.join(os.path.expanduser("~"), ".local", "state")
    return os.path.join(base, "meclaw", "pipeline")


def wave_dir(root, strand, wave=None):
    """`plans/<wave>` of a strand: given, or the one wave that knows the strand.

    A given wave the main tree lacks is the strand's when its branch
    `<prefix>/<strand>` carries `plans/<wave>/` -- the plan of a new wave
    brings the directory with its merge (Fund V-1, 14.10. 00:27: `no_wave`)."""
    plans = os.path.join(root, "plans")
    if wave:
        path = wave if os.path.isabs(wave) else os.path.join(plans, wave)
        if not os.path.isdir(path) and not (
                not os.path.isabs(wave) and "/" not in wave
                and on_branch(root, default_branch(path, strand), "plans/%s" % wave)):
            raise Refused("no_wave", "no wave directory %s" % path)
        return path
    def knows(d):
        branch = default_branch(d, strand)
        return any(os.path.isfile(os.path.join(plans, d, rel)) or on_branch(
            root, branch, "plans/%s/%s" % (d, rel))
            for rel in ("straenge/%s.json" % strand, "berichte/%s.md" % strand))
    hits = sorted(os.path.join(plans, d) for d in os.listdir(plans) if knows(d)) \
        if os.path.isdir(plans) else []
    if len(hits) != 1:
        raise Refused("no_wave", "%d waves know %s -- pass --wave" % (len(hits), strand))
    return hits[0]


def strand_name(name):
    """`welle-x/S` -> `S`: the merge takes the branch or the strand name (Z-I 2c (1))."""
    return name.rstrip("/").rsplit("/", 1)[-1]


def default_branch(wave, strand):
    """`<wave prefix>/<strand>` -- the branch `strand.sh new` creates."""
    return "%s/%s" % (os.path.basename(wave).rsplit("-", 3)[0], strand)


def on_branch(root, branch, rel):
    return git(root, "cat-file", "-e", "refs/heads/%s:%s" % (branch, rel)).returncode == 0


def tree_text(root, branch, rel, rev=None):
    """`rel` as the branch tip (or the commit `rev`) carries it, else as the
    main tree has it, else None.

    Strands commit their report and strand file on the branch; the main tree
    holds them only when `plans/` is not tracked there. `rev` is the tree pre
    and report grade (`pre.worktree_rev`): the declarations come from the same
    tree as the files (I1 of the v2-r3-bans re-check)."""
    ref = rev or (branch and "refs/heads/%s" % branch)
    if ref:
        res = git(root, "show", "%s:%s" % (ref, rel))
        if res.returncode == 0:
            return res.stdout
    try:
        with open(os.path.join(root, rel), encoding="utf-8") as fh:
            return fh.read()
    except OSError:
        return None


def head_block(report):
    try:
        with open(report, encoding="utf-8") as fh:
            text = fh.read()
    except OSError:
        return None, ""
    return parse_head(text)


def parse_head(text):
    """`(head, body)` of a report text; `(None, "")` without a text."""
    if text is None:
        return None, ""
    m = re.match(r"^---\n(.*?)\n---\n", text, re.S)
    if not m:
        return None, text
    head = {}
    for line in m.group(1).splitlines():
        key, _, value = line.partition(":")
        head[key.strip()] = value.strip()
    return head, text[m.end():]


def list_field(value):
    return [v.strip().strip('"').lstrip("#") for v in (value or "").strip("[]").split(",")
            if v.strip()]


class Strand:
    """Everything the merge reads about one strand: report and strand file from
    the branch tip -- or from `rev`, the worktree commit of pre and report --
    (the main tree as fallback), receipts from the main tree."""

    def __init__(self, root, name, wave=None, rev=None):
        self.root, self.name = root, name
        self.wdir = wave_dir(root, name, wave)
        self.report = os.path.join(self.wdir, "berichte", "%s.md" % name)
        self.archive = os.path.join(self.wdir, "receipts", name)
        rel = os.path.relpath(self.wdir, root)
        report_rel = os.path.join(rel, "berichte", "%s.md" % name)
        default = default_branch(self.wdir, name)
        self.head, self.body = parse_head(tree_text(root, default, report_rel, rev))
        self.branch = (self.head or {}).get("branch") or default
        if self.branch != default and not rev:
            self.head, self.body = parse_head(tree_text(root, self.branch, report_rel))
        path = versions.strand_file(self.wdir, name)
        try:
            self.doc = versions.strand_doc(
                tree_text(root, self.branch, os.path.relpath(path, root), rev), path)
        except versions.VersionError as exc:
            raise Refused("strand_file_invalid", str(exc))
        head = self.head or {}
        self.issues = versions.issues_of(self.doc) or list_field(head.get("issues"))
        self.title = self.doc.get("title") or name
        self.strand_file = path
        self.migrations = versions.migrations_of(self.doc)


# --- receipts --------------------------------------------------------------------------

def strand_receipts(archive):
    """Run directories of `archive` that hold a STRAND receipt, newest first.

    A run id is `<UTC second>-<sha8>`: two runs in one second at two commits
    sort by their hash, not by time. The second orders, and within it the
    later-written directory is the newer run (fix round 2 of v2-r3-kit: report
    read the older receipt as the head's)."""
    if not os.path.isdir(archive):
        return []

    def newest_first(d):
        try:
            written = os.stat(os.path.join(archive, d)).st_mtime_ns
        except OSError:
            written = 0
        return d.split("-", 1)[0], written, d

    found = []
    for d in sorted(os.listdir(archive), key=newest_first, reverse=True):
        path = os.path.join(archive, d)
        if d == "latest" or not os.path.isdir(path) or os.path.islink(path):
            continue
        doc = None
        for fn in ("last-strand.json", "receipt.json"):
            try:
                with open(os.path.join(path, fn), encoding="utf-8") as fh:
                    cand = json.load(fh)
            except (OSError, ValueError):
                continue
            if cand.get("mode", cand.get("stage")) == "strand":
                doc = cand
                break
        if doc is not None:
            found.append((path, doc))
    return found


def summary_line(path, mode="strand"):
    try:
        with open(os.path.join(path, "summary.txt"), encoding="utf-8") as fh:
            lines = [l.strip() for l in fh if l.startswith("GATE-SUMMARY %s " % mode)]
    except OSError:
        return ""
    return lines[-1] if lines else ""


def latest_strand_receipt(archive):
    """`(path, doc, line)` of the newest strand receipt (not the newest file)."""
    found = strand_receipts(archive)
    if not found:
        return None, None, ""
    path, doc = found[0]
    return path, doc, summary_line(path)


# --- preconditions ---------------------------------------------------------------------

def worktree_of(root, branch):
    path = None
    for line in out(root, "worktree", "list", "--porcelain").splitlines():
        if line.startswith("worktree "):
            path = line[len("worktree "):]
        elif line == "branch refs/heads/%s" % branch:
            return path
    return None


def pre_ok(root, branch, tip, gated=None):
    """The strand's pre stamp: (ok, line). Without a worktree there is no stamp.

    `gated` is the rev of the strand receipt when only a report commit follows
    it: the stamp of that rev counts (the stamp hashes HEAD)."""
    wt = worktree_of(root, branch)
    if not wt:
        return False, "no worktree of %s carries a pre stamp" % branch
    if out(wt, "rev-parse", "HEAD") != tip:
        return False, "the worktree of %s is not at the branch head" % branch
    from . import pre
    ok, why = pre.stamp_status(wt)
    if not ok and gated and gated != tip:
        ok2, why2 = pre.stamp_status(wt, head=gated)
        if ok2:
            return ok2, why2
    return ok, why


# The review sections: `## Review` (the scaffold of `strand.sh new`), the form
# the reports write, `## Fix-Runde Review <sha>` (Z-I 2c (2)), and every other
# `## ... Review ...` heading (Rebase/Mini-Runde/Minor-Runde/Vor-Batch Review):
# reading only the first two passed a later `nein` under another one over and
# merged on the earlier `ja` -- fail-open (kit find, v2-r3-kit2 rebase round).
# `Review` is a word: `## Reviewer-Notizen` is no review section. The word is
# case-blind (re-check Kette Minor 3): `## Fix-Runde review <sha>` is one too.
REVIEW_HEADING = re.compile(r"^## [^\n]*\bReview\b[^\n]*$", re.M | re.I)
# A verdict line (review Z-I gap I1): `Verdikt: ja`, `**Verdikt: JA**`,
# `reviews-cc-xhigh/<file>.md: Verdikt ja` -- `nein`/`no` is a verdict too.
REVIEW_VERDICT = re.compile(r"\b(?:Verdikt|Verdict)\b[\s:*_`]*(ja|yes|nein|no)\b", re.I)


def review_approved(body):
    """True when the LAST verdict line under the LAST review heading says
    `ja`/`yes`. The section runs to the next `## ` heading. An empty
    section, a section without a verdict and a final `nein` are refused:
    the heading alone is no review (`strand.sh new` writes it empty)."""
    heads = list(REVIEW_HEADING.finditer(body))
    if not heads:
        return False
    rest = body[heads[-1].end():]
    nxt = re.search(r"^## ", rest, re.M)
    section = rest[:nxt.start()] if nxt else rest
    verdicts = [m.group(1).lower() for m in REVIEW_VERDICT.finditer(section)]
    return bool(verdicts) and verdicts[-1] in ("ja", "yes")


# GH #1196: the report says what the strand moved into issues -- nothing, or
# each issue with the owner ruling that allows it. Findings moved into fresh
# issues to get a strand green only shift the cost to a later reader. The LAST
# such line decides; the scaffold's placeholder (`<...>`) never passes.
OUTSOURCED = re.compile(r"^[ \t]*(?:[-*][ \t]*)?\**Outsourced findings:\**[ \t]*(.*?)[ \t]*$",
                        re.M | re.I)
OUTSOURCED_ITEM = re.compile(r"#\d+ \(owner-ruling [^()<>\s][^()<>]*\)")


def outsourced_findings_ok(body):
    """True when the last `Outsourced findings:` line says `none` or lists
    only `#<n> (owner-ruling <reference>)` items (comma-separated)."""
    lines = OUTSOURCED.findall(body or "")
    if not lines:
        return False
    value = lines[-1]
    if value.lower() == "none":
        return True
    items = [i.strip() for i in value.split(",")]
    return all(OUTSOURCED_ITEM.fullmatch(i) for i in items)


def new_wave_plan(s, rev):
    """Fund V-1: is `s` the plan strand of a wave `rev` does not hold yet?
    Then plan-only takes every file of plans/<wave>/ but receipts/
    (`turnaround.plan_path`); any other strand, or a wave `rev` holds, keeps
    the allowlist of fix round 7. Fail closed: no rev, no new wave."""
    wave = os.path.basename(s.wdir)
    return (s.name == turnaround.PLAN_STRAND and s.branch == default_branch(s.wdir, s.name)
            and bool(rev)
            and git(s.root, "cat-file", "-e", "%s:plans/%s" % (rev, wave)).returncode != 0)


def _neutral(s, rev):
    """`(allowlisted, offending kwargs, code_equal_revs kwargs)` for `s` at `rev`."""
    if new_wave_plan(s, rev):
        return (turnaround.plan_allowlisted, {"allow": turnaround.plan_allowlisted},
                {"rule": turnaround.plan_path})
    return turnaround.allowlisted, {}, {}


def review_first(s, base, tip, no_pre=None):
    """K-R3: the review preconditions BEFORE anything that costs -- the
    reader rules of `plan_only` (minutes over a large diff), the migration
    scan, the merge gate's plan. A refusal there cost a whole merge
    preparation (16 min) and a gate cycle. A review that is there and not
    `Verdikt: ja` refuses always (plan-only or not); a branch without a
    report or review merges only plan-only, and one path outside the
    gate-neutral allowlist (`turnaround.allowlisted`, a string match) rules
    that out without reading a file. Such a refusal is `preconditions`',
    with every other missing precondition beside it (GH #1167 c); what stays
    open, `preconditions` and the plan-only branch decide as before."""
    if s.head is not None and REVIEW_HEADING.search(s.body or ""):
        if not review_approved(s.body):
            preconditions(s, tip, no_pre)
        return
    mb = out(s.root, "merge-base", base, tip)
    try:
        paths = turnaround.changed(s.root, mb, tip) if mb else []
    except ValueError:
        return
    wave = os.path.basename(s.wdir)
    allowed = _neutral(s, mb)[0]
    if any(not allowed(p, wave) for p in paths):
        preconditions(s, tip, no_pre)


def title_ok(s):
    """Runde 3 R3 (3): the strand file names the merge commit (`Merge <S> (#n): <title>`).
    `strand.sh new` writes `"title": ""` and the merge filled the gap with the strand's
    name -- "Merge v2-r2-pre (#1199, #1130, #1112): v2-r2-pre" and nine more on master by
    15.10. An empty title, one of blanks, or the bare strand name is no title."""
    title = (s.doc or {}).get("title")
    return isinstance(title, str) and title.strip() not in ("", s.name)


def preconditions(s, tip, no_pre=None):
    """Every failing precondition in ONE `Refused` (GH #1167 c); returns the receipt.

    The strand receipt holds for the CODE tree (GH #1167 a/h): commits after it
    that touch only regular report and strand files of the strand's own wave
    keep it (`turnaround.code_equal_revs`, by construction: no symlink or mode
    change on the path, in the gated rev, the tip or master, and no committed
    reader); any other path refuses with the path named. The commits of `master..tip` must be the owner's (f)."""
    wave = os.path.basename(s.wdir)
    missing = []
    if s.head is None:
        missing.append(("no_report", "%s has no report with a head block" % s.report))
    elif not REVIEW_HEADING.search(s.body):
        missing.append(("no_review", "%s has no `## Review` section" % s.report))
    elif not review_approved(s.body):
        missing.append(("no_review_verdict", "%s: the last verdict line under the last review "
                        "heading is not `Verdikt: ja` (regex %s)" % (s.report,
                                                                     REVIEW_VERDICT.pattern)))
    if not title_ok(s):
        missing.append(("strand_title_missing", "%s has no title (empty, or only the strand name "
                        "%s) -- write the English one-line title of the merge commit" % (
                            s.strand_file, s.name)))
    if s.head is not None and not outsourced_findings_ok(s.body):
        missing.append(("no_outsourced_findings", "%s has no line `Outsourced findings: none` or "
                        "`Outsourced findings: #<n> (owner-ruling <reference>)`" % s.report))
    path, doc, line = latest_strand_receipt(s.archive)
    gated = None
    if doc is None:
        missing.append(("no_strand_receipt", "no strand receipt under %s" % s.archive))
    elif doc.get("verdict") != "GREEN" or not line.endswith(" GREEN"):
        missing.append(("strand_gate_not_green", "newest strand receipt is %s, its summary %s" % (
            doc.get("verdict") or "?", line or "missing")))
    else:
        gated = doc.get("rev")
        if gated != tip:
            same, bad = turnaround.code_equal_revs(s.root, gated, tip, wave)
            if not same:
                missing.append(("receipt_not_branch_head",
                                "strand receipt on %s, branch head %s; code changed since: %s" % (
                                    (gated or "?")[:9], tip[:9], turnaround.describe(bad))))
    if no_pre is None:
        ok, why = pre_ok(s.root, s.branch, tip, gated)
        if not ok:
            missing.append(("pre_stamp", why))
    verdict, why = turnaround.owner_check(s.root, "master..%s" % tip)
    if verdict != "GREEN":
        missing.append(("foreign_commits", why))
    # VORGABEN § 5.4 (B4): the testing-rulings register is written by the owner
    # or the coordinator on master alone; a strand that touches it is refused.
    # Fail-closed: a git error is a refusal, not an empty diff (re-check tv-p0 B1).
    reg = git(s.root, "diff", "--name-only", "master...%s" % tip, "--", TESTING_REGISTER)
    if reg.returncode != 0:
        missing.append(("testing_register", "cannot diff %s against master (git exit %d: %s)" % (
            TESTING_REGISTER, reg.returncode, (reg.stderr or "").strip()[:200])))
    elif reg.stdout.strip():
        missing.append(("testing_register", "the branch changes %s -- only the owner or the "
                        "coordinator commits the register, on master" % TESTING_REGISTER))
    why = lifted_problem(s, tip)
    if why:
        missing.append(("lifted_bans", why))
    if missing:
        raise Refused.all_of(missing)
    # `run`: the store run of a v2 receipt (the `run` the cache rows carry,
    # GH #1141), else the run directory of a legacy one.
    return {"path": path, "rev": doc.get("rev"), "line": line,
            "run": doc.get("run") or os.path.basename(path), "no_pre": no_pre,
            "base": doc.get("base"),
            # B2 of the re-check: only a full strand gate on a clean tree
            # lets the queue plan from the tip (`turnaround.merge_gate_base`).
            "partial": turnaround.partial_receipt(doc, _plan_doc(path))}


def lifted_problem(s, tip):
    """V2-A6 (GH #1199): why the strand's lifted or FLAKY-fixed files fail the
    testing bans over the whole file (`lifted.check`), or None. Fail-closed: a
    git error or a malformed declaration is a refusal, never a pass.

    The base is the merge base with the tree's main branch, resolved as the
    register does (`register.main_branch`: `main` or `master`, whichever
    exists): a fixed `master` refused every strand of a tree whose main branch
    is `main` with "cannot find the merge base" (GH #1265)."""
    ref = main_branch(s.root)
    if not ref:
        return "cannot find the merge base of %s: no main or master branch" % tip[:9]
    mb = out(s.root, "merge-base", ref, tip)
    if not mb:
        return "cannot find the merge base of %s and %s" % (ref, tip[:9])
    try:
        found, paths, notes = lifted.check(s.root, mb, tip, s.doc, s.head)
    except (ValueError, OSError, subprocess.CalledProcessError) as exc:
        return "cannot grade the lifted files: %s" % exc
    if not found:
        return None
    return ("lifted or FLAKY-fixed files must meet the testing bans over the whole file "
            "(docs/development-rules.md § 11); %d hit(s) in %s: %s%s" % (
                len(found), ", ".join(paths), "; ".join(found),
                (" [%s]" % "; ".join(notes)) if notes else ""))


def _plan_doc(run_dir):
    """The `plan.json` beside a strand receipt, or None."""
    try:
        with open(os.path.join(run_dir, "plan.json"), encoding="utf-8") as fh:
            got = json.load(fh)
    except (OSError, ValueError):
        return None
    return got if isinstance(got, dict) else None


def plan_only(s, base, tip):
    """GH #1167 (d): the branch touches gate-neutral paths only -- it merges
    without gate and without report. Fail closed: no diff, a failing git or
    one path outside the neutral set is no plan-only branch. The neutral set
    is the receipt reuse's (`code_equal_revs`) AND the reader rules'
    (`offending`): a plan-only merge has no gate behind it."""
    mb = out(s.root, "merge-base", base, tip)
    if not mb:
        return False
    try:
        paths = turnaround.changed(s.root, mb, tip)
    except ValueError:
        return False
    wave = os.path.basename(s.wdir)
    _allowed, off_kw, reuse_kw = _neutral(s, mb)
    # V2-A10 (h): one path outside the rule decides it -- the merge of
    # v2-fix-b held tree:master 826 s before its gate (journal seq 116541 ->
    # 116542) for the reader greps of 310 re-check files, with 53 code paths
    # beside them. The string rule first, the greps only for a candidate.
    rule = reuse_kw.get("rule") or turnaround.reuse_path
    if any(rule(p, wave) for p in paths):
        return False
    return (bool(paths) and turnaround.code_equal_revs(s.root, mb, tip, wave, **reuse_kw)[0]
            and not turnaround.offending(paths, s.root, wave, **off_kw))


def open_migrations(root, base):
    """`{strand: [n, ...]}` of every strand whose branch is not merged into
    `base` and whose strand file names store migrations (OR-DT.store-migrations)."""
    unmerged = set(out(root, "branch", "--no-merged", base,
                       "--format=%(refname:short)").split())
    plans = os.path.join(root, "plans")
    found = {}
    for d in sorted(os.listdir(plans)) if os.path.isdir(plans) else []:
        prefix = default_branch(d, "")
        names = {b[len(prefix):] for b in unmerged if b.startswith(prefix)}
        sdir = os.path.join(plans, d, "straenge")
        if os.path.isdir(sdir):
            names |= {f[:-5] for f in os.listdir(sdir) if f.endswith(".json")}
        for name in sorted(names):
            branch = default_branch(d, name)
            if branch not in unmerged:
                continue
            rel = "plans/%s/straenge/%s.json" % (d, name)
            try:
                nums = versions.migrations_of(versions.strand_doc(
                    tree_text(root, branch, rel), rel))
            except versions.VersionError:
                continue  # refused on its own merge, never here
            if nums:
                found[name] = nums
    return found


def migration_order(name, mine, others):
    """Raise `Refused` when another open strand shares a migration number with
    `mine` or carries a smaller one (it merges first)."""
    if not mine:
        return
    for other, nums in sorted(others.items()):
        common = sorted(set(mine) & set(nums))
        if common:
            raise Refused("migration_collision", "%s and %s both carry store migration %s"
                          " -- renumber one (OR-DT.store-migrations)" % (name, other, common))
    for other, nums in sorted(others.items()):
        if min(nums) < min(mine):
            raise Refused("migration_order", "%s carries store migration %d, %s only %d --"
                          " merge %s first (OR-DT.store-migrations)"
                          % (other, min(nums), name, min(mine), other))


# --- the lease --------------------------------------------------------------------------

def resource(repo):
    return "tree:master@%s" % repo


def _merge_row(db, mid):
    row = db.execute("SELECT * FROM merges WHERE id=?", (mid,)).fetchone()
    return dict(row) if row else None


def _set_merge(db, mid, **cols):
    with store.tx(db):
        sets = ", ".join("%s=?" % k for k in cols)
        db.execute("UPDATE merges SET %s WHERE id=?" % sets,
                   [store._dump(v) if isinstance(v, (dict, list)) else v
                    for v in cols.values()] + [mid])


def _new_merge(db, repo, strand, base, state, at):
    mid = "mg-%s" % uuid.uuid4().hex[:12]
    with store.tx(db):
        db.execute("INSERT INTO merges (id, repo, strand, base, state, at) VALUES (?,?,?,?,?,?)",
                   (mid, repo, strand, base, state, at))
    return mid


def _queued(db, repo):
    return [dict(r) for r in db.execute(
        "SELECT * FROM merges WHERE repo=? AND state='queued' ORDER BY at, rowid", (repo,))]


def _members(holder):
    """The strands a holder speaks for: `S1+S2` (a batch) is S1 and S2 -- the
    queue rows and the offer name single strands, never the batch label."""
    return {holder} | set(holder.split("+"))


def take_tree(db, repo, holder, at=None):
    """True when `holder` holds the tree lease now; False = wait in the queue."""
    at = at or store.now()
    res = resource(repo)
    mine = _members(holder)
    with store.tx(db):
        row = db.execute("SELECT * FROM leases WHERE resource=?", (res,)).fetchone()
        live = row is not None and row["holder"] != "" and not store._expired(row, at)
        if live and _members(row["holder"]) & mine:
            # The same strand twice would share the candidate path, and the
            # first release would free the lease under the second.
            raise Refused("already_merging", "%s is being merged right now (lease holder %s)"
                          % (holder, row["holder"]))
        if live:
            return False
        offered = row["offered_to"] if row is not None and row["holder"] == "" else None
        if offered and offered not in mine:
            if not store._expired(row, at):
                return False
            lapsed = _abandon(db, repo, offered)
        else:
            lapsed = []
        # The queue speaks only through the offer: a free tree without a live
        # offer is first come -- queued rows nobody will wake (parallel callers
        # that queued each other, a lapsed offer) must not block it for good.
        db.execute("INSERT OR REPLACE INTO leases VALUES (?,?,?,?,?,NULL)",
                   (res, holder, None, at, TTL_S))
    for mid, base in lapsed:
        _event(db, "merge_abandoned", mid, offered, base, reason="offer_lapsed")
    events.emit(db, "lease_taken", "note", {"resource": res, "holder": holder, "run": None,
                                            "ttl_s": TTL_S}, at=at)
    # V2-A10: a new holder moves the waiting merges up the queue -- they say so.
    _ring(db)
    return True


def hold_tree(db, repo, holder, at=None):
    """Before each gate run and right before the fast-forward: the lease is
    still `holder`'s and live -- renewed then; `Refused("lease_lost")` otherwise."""
    at = at or store.now()
    res = resource(repo)
    with store.tx(db):
        row = db.execute("SELECT * FROM leases WHERE resource=?", (res,)).fetchone()
        if row is None or row["holder"] != holder or store._expired(row, at):
            raise Refused("lease_lost", "the lease tree:master is no longer %s's (%s)" % (
                holder, (row["holder"] or "offered") if row is not None else "free"))
        db.execute("UPDATE leases SET taken_at=? WHERE resource=?", (at, res))


def trap_term():
    """SIGTERM ends the run through its `finally` (the lease is released), not
    mid-air with the lease standing for TTL_S. Returns the handler to restore."""
    def stop(signum, _frame):
        raise SystemExit(128 + signum)
    try:
        return signal.signal(signal.SIGTERM, stop)
    except ValueError:  # not the main thread: nothing to trap
        return None


def untrap_term(prev):
    if prev is not None:
        signal.signal(signal.SIGTERM, prev)


def _abandon(db, repo, strand):
    """Mark the queued rows of `strand` abandoned (inside the caller's tx); their ids."""
    rows = [(r["id"], r["base"]) for r in db.execute(
        "SELECT id, base FROM merges WHERE repo=? AND strand=? AND state='queued'",
        (repo, strand))]
    for mid, _ in rows:
        db.execute("UPDATE merges SET state='abandoned' WHERE id=?", (mid,))
    return rows


def dequeue(db, repo, strand, at=None):
    """`merge --dequeue`: drop the queued rows of `strand`; an offer to it passes
    on to the next queued merge (or the lease goes). Returns the rows dropped."""
    at = at or store.now()
    res = resource(repo)
    with store.tx(db):
        ids = _abandon(db, repo, strand)
        row = db.execute("SELECT * FROM leases WHERE resource=?", (res,)).fetchone()
        nxt = None
        if row is not None and row["holder"] == "" and row["offered_to"] == strand:
            nxt = next((q["strand"] for q in _queued(db, repo)), None)
            if nxt:
                db.execute("UPDATE leases SET taken_at=?, ttl_s=?, offered_to=? "
                           "WHERE resource=?", (at, OFFER_TTL_S, nxt, res))
            else:
                db.execute("DELETE FROM leases WHERE resource=?", (res,))
    for mid, base in ids:
        _event(db, "merge_abandoned", mid, strand, base, reason="dequeued")
    if nxt:
        events.emit(db, "lease_offered", "note", {"resource": res, "holder": nxt, "run": None,
                                                  "ttl_s": OFFER_TTL_S}, at=at)
    if ids or nxt:
        # V2-A10: a dropped row moves every merge behind it up the queue, and
        # an offer passed on moves the next one to the head (re-check f+g).
        _ring(db)
    return len(ids)


def release_tree(db, repo, holder, at=None):
    """Free the lease; the oldest queued merge gets it OFFERED (no poll wakes it)."""
    at = at or store.now()
    res = resource(repo)
    nxt = next((q["strand"] for q in _queued(db, repo) if q["strand"] not in _members(holder)),
               None)
    with store.tx(db):
        row = db.execute("SELECT * FROM leases WHERE resource=?", (res,)).fetchone()
        if row is None or row["holder"] != holder:
            return False
        if nxt:
            db.execute("UPDATE leases SET holder='', run=NULL, taken_at=?, ttl_s=?, "
                       "offered_to=? WHERE resource=?", (at, OFFER_TTL_S, nxt, res))
        else:
            db.execute("DELETE FROM leases WHERE resource=?", (res,))
    events.emit(db, "lease_released", "note", {"resource": res, "holder": holder, "run": None,
                                               "ttl_s": TTL_S}, at=at)
    if nxt:
        events.emit(db, "lease_offered", "note", {"resource": res, "holder": nxt, "run": None,
                                                  "ttl_s": OFFER_TTL_S}, at=at)
    _ring(db)
    return True


def _ring(db):
    """Wake every `merge --wait` (and every other waiter of the pool) after
    the commit: the tree lease is free or offered now (V2-P1196-3)."""
    schedule._wake(schedule._db_path(db))


def wait_state(db, repo, holder):
    """`(who, position)` of a merge waiting for the tree lease (V2-A10): who
    holds the lease (`held by X`, `offered to X`) and the merge's place among
    the queued rows that wait behind it -- the holder's own rows do not count
    (they stay `queued` until its candidate is gated)."""
    who = store.lease_who(db, resource(repo)) or {}
    if who.get("holder"):
        state, ahead_of = "held by %s" % who["holder"], _members(who["holder"])
    elif who.get("offered_to"):
        state, ahead_of = "offered to %s" % who["offered_to"], set()
    else:
        state, ahead_of = "free", set()
    mine = _members(holder)
    queue = [q["strand"] for q in _queued(db, repo) if q["strand"] not in ahead_of]
    return state, next((i + 1 for i, n in enumerate(queue) if n in mine), len(queue) + 1)


def wait_line(repo, holder, state):
    return "merge: waiting -- the merge lease %s is %s; %s is #%d in the queue (queued, --wait)" % (
        resource(repo), state[0], holder, state[1])


def take_tree_waiting(db, repo, holder, on_queued=None, clock=None):
    """`take_tree` until `holder` holds the lease (`merge --wait`, V2-P1196-3).

    Wave validation 2: two merges at once ended with exit 3 for the second and
    a retry by hand -- master moves before the lease is free, and a ref move
    wakes nobody. This is the wait of `gate --wait-admission` and `token take
    --wait` (`schedule.admit_waiting`, R-AG-1, no retry clock): the pool's wake
    file is opened before each round and blocked on after a queued one;
    `release_tree` and `dequeue` ring it (`_ring`). One watchdog per round
    (`MECLAW_TOKEN_WAIT_SECS`, 30 s) covers what rings nothing (a TTL or an
    offer running out); the whole wait ends after `MECLAW_ADMISSION_WAIT_SECS`
    (4 h) -- False, still queued. `on_queued(first)` hears every queued
    round (V2-A10: the merge says its wait and each change of it -- a round
    follows a ring, or the watchdog); a `Refused` from it ends the wait as it
    is. `clock` is the kit clock of the take (`store.now`). A locked store
    holds the wait up to its limit (`schedule.while_locked`, tm-orch4)."""
    secs = schedule.wait_secs(os.environ, "MECLAW_TOKEN_WAIT_SECS", "30")
    limit = schedule.wait_secs(os.environ, "MECLAW_ADMISSION_WAIT_SECS", "14400")
    end, path, said = time.monotonic() + limit, schedule._db_path(db), False
    clock = clock or store.now

    def take():
        at = clock()
        try:
            return take_tree(db, repo, holder, at=at)
        except (sqlite3.OperationalError, store.StoreWriteError) as exc:
            # refused after the lease committed (its events): the lease is ours,
            # a second take would read `already_merging` from it
            if schedule.store_locked(exc) and _took(db, repo, holder, at):
                _ring(db)
                return True
            raise
    while True:
        fd = schedule._wake_open(path)
        try:
            if schedule.while_locked(take, end=end, reset=lambda: schedule._rollback(db)):
                return True
            if on_queued:
                schedule.while_locked(lambda: on_queued(not said), end=end,
                                      reset=lambda: schedule._rollback(db))
            said = True
            left = end - time.monotonic()
            if left <= 0:
                return False
            select.select([fd] if fd is not None else [], [], [], min(secs, left))
        finally:
            if fd is not None:
                os.close(fd)


def lane_holders(strands):
    """The lane leases the merge gate of `strands` runs on: its holder
    (`gate_holder`) and, for a batch `<wave>/S1+S2`, each member's lease the
    admission speaks for (`schedule._batch_speaker`)."""
    key = gate_holder(strands)
    prefix, _, names = key.rpartition("/")
    if "+" not in names:
        return [key]
    return [key] + ["%s/%s" % (prefix, n) if prefix else n for n in names.split("+")]


def give_back_lanes(db, repo, strands, err, said):
    """OR-TM-6 (K15, 16.10. 20:03): a merge that waits for `tree:master` holds
    no lane. The lane lease of its gate holder -- the `token take` before
    `merge --wait` (workaround K13-3) or one the strand kept -- stood leased and
    idle while the merge waited: two of four lanes, strand gates in the queue.
    Each wait round gives such a lease back (`schedule.lease_release`, no
    force: a lease whose run lives stays until that run ends, GH #1228) and
    rings the pool; the candidate gate takes a lane only under the merge lease,
    ahead of the waiting strands (`gate --wait-admission`, K14). `said` keeps
    each line to once."""
    if not schedule._armed(db):
        return
    for holder in lane_holders(strands):
        lane = schedule.lease_lane(db, holder).lane
        if not lane:
            continue
        res = schedule.lease_release(db, holder, branch=holder)
        if res.code == 0:
            _ring(db)
            line = "merge: lane %s of %s given back while the merge waits for %s -- the " \
                   "candidate gate takes a lane under the merge lease" % (lane, holder,
                                                                         resource(repo))
        else:
            why = (res.err or res.out).strip().splitlines()
            line = "merge: lane %s of %s stays while the merge waits (%s)" % (
                lane, holder, why[-1] if why else "release exit %d" % res.code)
        if line not in said:
            said.append(line)
            err.write(line + "\n")
            err.flush()


def _took(db, repo, holder, at):
    """Did the take at `at` commit `holder`'s tree lease? (`take_tree` writes `at`.)

    A refused COMMIT leaves its transaction open, so it is rolled back first
    and only the committed state is read (re-check Kette m1, both COMMITs):
    the lease's own COMMIT refused -- the row was a phantom no other
    connection sees, now gone: not taken, the next round takes again (lease
    re-check m1); the journal's COMMIT refused after the lease's went through
    -- the committed lease is ours: taken, no second take that reads
    `already_merging` from it."""
    schedule._rollback(db)
    row = store.lease_who(db, resource(repo)) or {}
    return row.get("holder") == holder and row.get("taken_at") == at


def lease_resolves(strands, base, no_pre):
    """`merge --wait` waits only when the lease can change the end: a strand
    whose branch is gone, whose review check refuses (K-R3, `review_first`)
    or whose own preconditions fail (strand receipt missing, RED or not on
    the head, pre stamp, report) is refused after any wait too -- re-check
    v2-fix-c I1: with `Verdikt: ja` a strand without receipt waited the
    holder's whole merge (rc 3 at the limit) instead of `no_strand_receipt`.
    A plan-only branch needs no receipt (GH #1167 d), so its refusal counts
    only off plan-only, as under the lease. When that holds for every
    strand, the refusal is raised here -- exit 1 at once, as without `--wait`."""
    missing = []
    for s in strands:
        try:
            tip = out(s.root, "rev-parse", "--verify", "--quiet", "%s^{commit}" % s.branch)
            if not tip:
                raise Refused("no_branch", "no branch %s" % s.branch)
            review_first(s, base, tip, no_pre)
            try:
                preconditions(s, tip, no_pre)
            except Refused:
                if not plan_only(s, base, tip):
                    raise
            return
        except Refused as exc:
            missing.append((exc.reason, exc.text))
    raise Refused.all_of(missing)


def _event(db, name, mid, strand, base, **extra):
    data = {"merge": mid, "strand": strand, "base": base}
    data.update({k: v for k, v in extra.items() if v is not None})
    events.emit(db, name, "status", data)


def _events(db, name, rows, base):
    """`_event` for each `(mid, strand)` of `rows` in one write transaction: a
    caller that repeats after a locked store (`schedule.while_locked`, whose
    `fn` must be safe to repeat) finds all of them written or none -- one
    transaction per event journaled the first member twice when the second
    found the store locked (re-check stack fr m2)."""
    with store.tx(db):
        for mid, strand in rows:
            _event(db, name, mid, strand, base)


# --- the candidate ----------------------------------------------------------------------

def candidate_path(label):
    return os.path.join(state_dir(), "merge", label)


def open_candidate(root, base, label):
    path = candidate_path(label)
    if os.path.exists(path):
        git(root, "worktree", "remove", "--force", path)
        if os.path.exists(path):
            shutil.rmtree(path)
    git(root, "worktree", "prune")
    os.makedirs(os.path.dirname(path), exist_ok=True)
    git(root, "worktree", "add", "--detach", path, base, check=True)
    return path


def close_candidate(root, path):
    git(root, "worktree", "remove", "--force", path)
    if os.path.exists(path):
        shutil.rmtree(path, ignore_errors=True)
    git(root, "worktree", "prune")


def merge_into(cand, refs):
    """`git merge --no-ff --no-commit` of `refs`; the corpus is regenerated, never merged,
    a conflict of pure additions under `## [Unreleased]` of CHANGELOG.md keeps both
    sides (`unreleased_union`, OR-TM-7), and every released `## [x.y.z]` stays master's
    (`keep_released`, OR-TM-12). Returns the resolution line, or None."""
    res = git(cand, "merge", "--no-ff", "--no-commit", *refs)
    if res.returncode == 0:
        return keep_released(cand)
    conflicts = out(cand, "diff", "--name-only", "--diff-filter=U").split()
    union = None
    if conflicts and len(refs) == 1 and CHANGELOG in conflicts:
        union = unreleased_union(*(_stage(cand, n, CHANGELOG) for n in (1, 2, 3)))
    if (conflicts and len(refs) == 1 and all(c in CORPUS or c == CHANGELOG for c in conflicts)
            and (union or CHANGELOG not in conflicts)):
        for c in conflicts:
            if c == CHANGELOG:
                with open(os.path.join(cand, c), "w", encoding="utf-8") as fh:
                    fh.write(union[0])
            else:
                git(cand, "checkout", "--ours", "--", c, check=True)
            git(cand, "add", "--", c, check=True)
        moved = keep_released(cand)
        return "; ".join(x for x in ((union or (None, None))[1], moved) if x) or None
    git(cand, "merge", "--abort")
    raise Refused("batch_conflict" if len(refs) > 1 else "merge_conflict",
                  "conflict in %s" % (", ".join(conflicts) or res.stderr.strip()[:200]))


CHANGELOG = "CHANGELOG.md"
UNRELEASED = "## [Unreleased]"


def _stage(cand, n, rel):
    """The text of `rel` at index stage `n` (1 base, 2 ours, 3 theirs), or None."""
    res = git(cand, "show", ":%d:%s" % (n, rel))
    return res.stdout if res.returncode == 0 else None


def _unreleased(text):
    """`(head, body, tail)` lines around the body of the one `## [Unreleased]`; the body
    ends at the next `## ` heading. None without exactly one such section."""
    if text is None:
        return None
    lines = text.split("\n")
    at = [i for i, line in enumerate(lines) if line.startswith(UNRELEASED)]
    if len(at) != 1:
        return None
    end = next((i for i in range(at[0] + 1, len(lines)) if lines[i].startswith("## ")),
               len(lines))
    return lines[:at[0] + 1], lines[at[0] + 1:end], lines[end:]


def _trim(lines):
    lines = list(lines)
    while lines and not lines[0].strip():
        lines.pop(0)
    while lines and not lines[-1].strip():
        lines.pop()
    return lines


def _parse(body):
    """`_sections` without the write-back check."""
    pre, secs, seen = [], [], {}
    for line in body:
        if line.startswith("### "):
            seen[line] = seen.get(line, -1) + 1
            secs.append(((line, seen[line]), []))
        else:
            (secs[-1][1] if secs else pre).append(line)
    return (_trim(pre), [(key, _trim(lines)) for key, lines in secs])


def _sections(body):
    """`(preamble, [((heading, n), lines)])` of an `[Unreleased]` body (`### `
    headings; `#### ` stays content): a section is keyed by its heading and the
    heading's occurrence index `n`, so a heading twice keeps both sections apart
    (OR-V2.v2-r3-unreldup.1). None for a body this form would not write back byte
    for byte -- the union never reformats what it keeps."""
    doc = _parse(body)
    return doc if _body(doc) == body else None


def _body(doc):
    pre, secs = doc
    body = [""] + (pre + [""] if pre else [])
    for (heading, _), lines in secs:
        body += [heading, ""] + lines + [""]
    return body


def _gaps(base, side):
    """What `side` adds before each line of `base` and after its last
    (`len(base) + 1` lists), or None when `side` changes or drops a base line."""
    gaps, k = [[] for _ in range(len(base) + 1)], 0
    for line in side:
        if k < len(base) and line == base[k]:
            k += 1
        else:
            gaps[k].append(line)
    return gaps if k == len(base) else None


def _join(base, ours, theirs):
    """`base` with each gap's additions, master's first, then the strand's (an
    identical addition once); `(lines, ours_added, theirs_added)`, or None when a
    side is not purely additive."""
    og, tg = _gaps(base, ours), _gaps(base, theirs)
    if og is None or tg is None:
        return None
    lines = []
    for k in range(len(base) + 1):
        lines += og[k] + (tg[k] if tg[k] != og[k] else [])
        lines += base[k:k + 1]
    return lines, sum(og, []), sum(tg, [])


def _entries(lines):
    return sum(1 for line in lines if line.startswith(("- ", "* ")))


def unreleased_union(base, ours, theirs):
    """`(text, line)` of CHANGELOG.md with both sides' additions under
    `## [Unreleased]` (OR-TM-7, K15: a list conflict cost a full strand re-gate),
    or None. Everything outside that body must be untouched by one side; each
    side keeps every line of the base body in its `### ` section, a section
    being a heading and its occurrence (`_sections`: a heading twice is two
    sections, none lost); a section both add stands once. `line` names the file
    and the entries each side added."""
    parts = [_unreleased(t) for t in (base, ours, theirs)]
    if None in parts:
        return None
    (bh, bb, bt), (oh, ob, ot), (th, tb, tt) = parts
    outside = [(bh, bt), (oh, ot), (th, tt)]
    if outside[1] != outside[0] and outside[2] != outside[0] and outside[1] != outside[2]:
        return None
    head, tail = outside[1] if outside[2] == outside[0] else outside[2]
    docs = [_sections(b) for b in (bb, ob, tb)]
    if None in docs:
        return None
    (bp, bs), (op, os_), (tp, ts) = docs
    known = dict(bs)
    keys = _join([k for k, _ in bs], [k for k, _ in os_], [k for k, _ in ts])
    pre = _join(bp, op, tp)
    if keys is None or pre is None:
        return None
    ours_d, theirs_d = dict(os_), dict(ts)
    secs, added = [], [pre[1], pre[2]]
    for k in dict.fromkeys(keys[0]):
        joined = _join(known.get(k, []), ours_d.get(k, known.get(k, [])),
                       theirs_d.get(k, known.get(k, [])))
        if joined is None:
            return None
        secs.append((k, joined[0]))
        added[0], added[1] = added[0] + joined[1], added[1] + joined[2]
    text = "\n".join(head + _body((pre[0], secs)) + tail)
    return text, "%s [Unreleased]: both sides kept, master +%d, strand +%d entries" % (
        CHANGELOG, _entries(added[0]), _entries(added[1]))


RELEASED_CHANGED = "released_changed"


def _chunks(lines):
    """The lines before the first `## ` heading, then one list per `## ` section."""
    chunks = [[]]
    for line in lines:
        if line.startswith("## "):
            chunks.append([])
        chunks[-1].append(line)
    return chunks


def _released_key(chunk):
    """The version of a released `## [x.y.z]` section, else None."""
    if not chunk or not chunk[0].startswith("## [") or chunk[0].startswith(UNRELEASED):
        return None
    end = chunk[0].find("]")
    return chunk[0][4:end] if end > 4 else None


def _released(text):
    """`{version: lines}` of every released section, or None for a version twice."""
    secs = {}
    for chunk in _chunks(text.split("\n"))[1:]:
        key = _released_key(chunk)
        if key is not None:
            if key in secs:
                return None
            secs[key] = chunk
    return secs


def _added(master, cand):
    """`[(key, lines)]` a released section `cand` adds to `master` (`key` a
    `_sections` key -- heading and occurrence --, None for the lines above its
    first `### `), or None for any other change: a changed heading line, a
    changed or dropped line or `### ` heading, a form `_sections` would not write
    back byte for byte."""
    if master[0] != cand[0]:
        return None
    docs = _sections(master[1:]), _sections(cand[1:])
    if None in docs:
        return None
    (mp, ms), (cp, cs) = docs
    heads, pre = _gaps([h for h, _ in ms], [h for h, _ in cs]), _gaps(mp, cp)
    if heads is None or pre is None:
        return None
    known, added = dict(ms), [(None, sum(pre, []))]
    for heading, lines in cs:
        gaps = _gaps(known.get(heading, []), lines)
        if gaps is None:
            return None
        added.append((heading, sum(gaps, [])))
    return [(h, lines) for h, lines in added if lines]


def keep_released(cand):
    """OR-TM-12 (K17, H4 merge c3f784b25): after the git merge, before the commit,
    every released `## [x.y.z]` of CHANGELOG.md is byte for byte master's (`HEAD`).
    A strand branched before a release cut merges without a conflict, and git sets
    its `[Unreleased]` lines into the released section that heading became: pure
    additions there move under `## [Unreleased]` (master first, into the section
    of the same heading and occurrence -- `_sections`, as the union keys them --,
    a new one at the end, an identical entry once) and the section is master's
    again; the resolution line, or None when nothing moved. Any other change of a
    released section of master (changed, dropped, a heading line, a form the
    resolver would not write back) refuses `released_changed`, the merge aborted;
    so does an `[Unreleased]` that cannot take the additions (none, twice, a form
    the merge would not write back), the refusal naming it with its line. A
    released section master does not have is the strand's own release cut."""
    head = git(cand, "show", "HEAD:%s" % CHANGELOG)
    if head.returncode != 0:
        return None
    master, path = head.stdout, os.path.join(cand, CHANGELOG)
    try:
        with open(path, encoding="utf-8") as fh:
            text = fh.read()
    except FileNotFoundError:
        text = None
    if text == master:
        return None
    ours = _released(master)
    theirs = _released(text) if text is not None else None
    if ours is not None and theirs is not None and all(
            theirs.get(v) == lines for v, lines in ours.items()):
        return None

    def refuse(what, why=None):
        git(cand, "merge", "--abort")
        raise Refused(RELEASED_CHANGED, "%s %s differs from master; a merge moves only pure "
                      "additions of a released section to [Unreleased]" % (CHANGELOG, what)
                      if why is None else "%s %s: the strand's additions cannot move, %s"
                      % (CHANGELOG, what, why))

    if ours is None or theirs is None:
        refuse("(gone)" if text is None else "(a released version twice)")
    moved, versions_ = [], []
    for version, lines in ours.items():
        if theirs.get(version) == lines:
            continue
        added = _added(lines, theirs[version]) if version in theirs else None
        if added is None:
            refuse("[%s]" % version)
        moved += added
        versions_.append(version)
    chunks = _chunks(text.split("\n"))
    at = [i for i, c in enumerate(chunks) if c[0:1] and c[0].startswith(UNRELEASED)]
    doc = _sections(chunks[at[0]][1:]) if len(at) == 1 else None
    if doc is None:
        refuse("[%s]" % "], [".join(versions_), _no_target(chunks, at))
    pre, secs = doc[0], [(h, list(lines)) for h, lines in doc[1]]
    for heading, lines in moved:
        into = pre if heading is None else next((x for h, x in secs if h == heading), None)
        if into is None:
            into = []
            secs.append((heading, into))
        into += [line for line in lines if not line.strip() or line not in into]
    chunks[at[0]] = chunks[at[0]][:1] + _body((pre, secs))
    chunks = [ours[_released_key(c)] if _released_key(c) in versions_ else c for c in chunks]
    with open(path, "w", encoding="utf-8") as fh:
        fh.write("\n".join(sum(chunks, [])))
    git(cand, "add", "--", CHANGELOG, check=True)
    return "%s [%s]: strand +%d entries moved to [Unreleased]" % (
        CHANGELOG, "], [".join(versions_), _entries(sum((x for _, x in moved), [])))


def _no_target(chunks, at):
    """Why the `## [Unreleased]` of `chunks` (at the chunk indexes `at`) takes no
    moved line: none, twice (each heading's line), or the first line of its body
    the `_sections` form would not write back."""
    starts = [sum(len(c) for c in chunks[:i]) + 1 for i in at]
    if not at:
        return "no [Unreleased] to take them"
    if len(at) > 1:
        return "[Unreleased] twice (%s:%s)" % (CHANGELOG, ", :".join(map(str, starts)))
    body = chunks[at[0]][1:]
    back = _body(_parse(body))
    n = next((k for k, (a, b) in enumerate(zip(body, back)) if a != b),
             min(len(body), len(back)))
    return "[Unreleased] at %s:%d (`%s`) is a form the merge would not write back byte for byte" % (
        CHANGELOG, starts[0] + 1 + n, body[n] if n < len(body) else "")


def regenerate_corpus(cand):
    """Rewrite the corpus when its sources moved; returns the paths it changed."""
    tool = os.path.join(cand, CORPUS_TOOL)
    if os.environ.get("MECLAW_MERGE_CORPUS") == "skip" or not os.path.isfile(tool):
        return []
    check = subprocess.run([sys.executable, tool, "--check"], cwd=cand, capture_output=True)
    if check.returncode == 0:
        return []
    regen = subprocess.run([sys.executable, tool], cwd=cand, capture_output=True, text=True)
    if regen.returncode != 0:
        raise Refused("corpus_failed", "%s exit %d: %s" % (
            CORPUS_TOOL, regen.returncode, regen.stderr.strip()[-300:]))
    again = subprocess.run([sys.executable, tool, "--check"], cwd=cand, capture_output=True,
                           text=True)
    if again.returncode != 0:
        raise Refused("corpus_failed", "%s --check still fails after the regeneration: %s"
                      % (CORPUS_TOOL, (again.stdout + again.stderr).strip()[-300:]))
    return [p for p in out(cand, "diff", "--name-only").split() if p in CORPUS]


# --- the goldens that follow the cascade (V2-A17) ---------------------------------------
#
# A committed golden that HASHES template trees cannot follow the cascade: the
# cascade raises pin strings, never a hash of the file that holds them. On
# 15.10. the merge point raised `collector@5.1.1 -> 5.1.2` in
# `templates/{talky,cogny}/collector/config.json` and the candidate gate went
# RED in the two gh277 byte-identity tests; no strand can write those goldens
# right beforehand (rule 6: numbers only at the merge point). So the merge
# point runs the goldens' own generator on the candidate whenever its own
# writes touched a golden's closure, and the rewritten goldens go into the
# merge commit before the gate compares byte for byte.
#
# One row per golden directory: the generator (a nextest filterset of the
# `#[ignore]`d writer and the nextest words it needs -- one package built,
# ignored tests only) and the switch it refuses to run without. The closure
# is read from the tree, never listed here: every `<name><suffix>` in `dir`
# names `templates/<name>`, plus every template a JSON pin under a closure
# directory names, transitively (talky -> collector, curator, dispatcher, ...).
GOLDENS = (
    {"dir": "plans/composition-reference/golden", "suffix": ".manifest",
     "switch": "GH277_WRITE_GOLDEN",
     "filter": "test(/(^|::)write_golden_manifests$/)",
     "args": ["-p", "meclaw-colony", "--run-ignored", "only"]},
)

_JSON_PIN = re.compile(r'"([a-z][a-z0-9-]*)@[0-9]+\.[0-9]+\.[0-9]+"')


def golden_closure(tree, spec):
    """`["templates/<dir>/", ...]`: the template trees the goldens of `spec` stage."""
    gdir = os.path.join(tree, spec["dir"])
    try:
        names = sorted(n[:-len(spec["suffix"])] for n in os.listdir(gdir)
                       if n.endswith(spec["suffix"]))
    except OSError:
        return []
    by_name = {n: d for d, n in versions.template_dirs(tree).items()}
    seen, todo = set(), [by_name[n] for n in names if n in by_name]
    while todo:
        d = todo.pop()
        if d in seen:
            continue
        seen.add(d)
        for dirpath, _dirs, files in os.walk(os.path.join(tree, "templates", d)):
            for fn in files:
                if not fn.endswith(".json"):
                    continue
                try:
                    with open(os.path.join(dirpath, fn), encoding="utf-8") as fh:
                        text = fh.read()
                except (OSError, UnicodeDecodeError):
                    continue
                todo += [by_name[m] for m in _JSON_PIN.findall(text) if m in by_name]
    return sorted("templates/%s/" % d for d in seen)


def golden_hits(tree, paths):
    """`[(spec, [path, ...])]`: the goldens whose closure a path of `paths` lies in."""
    hits = []
    for spec in GOLDENS:
        closure = golden_closure(tree, spec)
        inside = sorted(p for p in paths if any(p.startswith(c) for c in closure))
        if inside:
            hits.append((spec, inside))
    return hits


def golden_argv(tree, ctx, spec):
    """The generator run: the lane test of the candidate, as the gate would run
    it (`pipeline.py test`, the gate's holder and host), the ignored writer
    only; or `MECLAW_MERGE_GOLDEN_CMD` (words {tree} {holder} {host} {filter}
    {switch})."""
    custom = os.environ.get("MECLAW_MERGE_GOLDEN_CMD")
    if custom:
        return [w.format(tree=tree, holder=ctx.get("holder") or "", host=ctx.get("host") or "",
                         filter=spec["filter"], switch=spec["switch"])
                for w in shlex.split(custom)]
    # the kit's one Python (lane.PYTHON, `-I`): the narrow point refuses any other
    argv = python_argv(os.path.join(tree, "scripts", "pipeline.py"), "test")
    if ctx.get("holder"):
        argv += ["--strand", ctx["holder"]]
    if ctx.get("host"):
        argv += ["--host", ctx["host"]]
    return argv + [spec["filter"]] + list(spec.get("args") or [])


def golden_speaker(db, ctx):
    """The lease the generator runs under: the gate holder, for a batch
    `<wave>/S1+S2` the member lease it speaks for -- the one the merge gate's
    admission resolves too (schedule `_batch_speaker`, re-check RC-GK2 I2)."""
    holder = ctx.get("holder") or ""
    if db is None or not holder or not schedule._armed(db):
        return holder
    return schedule.lease_speaker(db, holder, ctx.get("host") or "")


def golden_lane(db, ctx):
    """The lane lease the generator runs under (#1199 V2-A17 follow-up, live
    merge G-a 12:15): `pipeline.py test` binds to a held lease of the gate
    holder, and the merge holds none -- its gate gets its lane from the
    admission. So the merge point leases one for the generator from the same
    armed pool (`--host`, else a lane the pool leaves it), with no queue place
    (as its gate, kit find (h)), never past the token (no
    MECLAW_STRAND_TOKEN_SKIP). Returns the holder to give back after the merge
    gate (`golden_give_back`); None when the holder -- or the member a batch
    speaks for (`golden_speaker`) -- already holds its lane (the strand's own
    lease stays its own) or the pool is not armed (the generator's own check
    speaks)."""
    holder = ctx.get("holder") or ""
    if db is None or not holder or not schedule._armed(db) \
            or schedule.lease_lane(db, golden_speaker(db, ctx)).lane:
        return None
    # pid: the merge process uses the lease from the generator through the
    # merge gate -- its life is the lease's; no `tree`: the candidate tree is
    # the merge's own and goes away, a lane tree it is not (re-check RC-GK2 M2).
    res = schedule.lease_take(db, holder, lane=ctx.get("host") or None, pid=os.getpid(),
                              queue=False)
    if res.code != 0:
        raise Refused("golden_failed", "no lane for the generator (not judged): %s"
                      % (res.err or res.out).strip())
    return holder


def golden_give_back(db, ctx, err):
    """Give the generator's lease (`ctx["golden_lease"]`) back, once: after the
    merge gate, or in any exit before it."""
    taken = ctx.pop("golden_lease", None)
    if not taken:
        return
    res = schedule.lease_release(db, taken)
    if res.code != 0:
        err.write("merge: the generator's lease of %s stays: %s\n"
                  % (taken, (res.err or res.out).strip()))


def follow_goldens(cand, ctx, hits, err, db=None):
    """Run the generator of each hit golden in `cand` and amend the candidate
    commit with what it rewrote. Returns `(sha, [path, ...])`; a generator that
    fails refuses the merge (`golden_failed`) -- fail closed, never a guess.
    The generator runs on the lane `golden_lane` leases. The lease stays with
    the holder through the merge gate (`ctx["golden_lease"]`, given back by
    `golden_give_back` after it): given back here, a strand that queued while
    the generator ran got the lane offered and the merge gate, which takes no
    queue place, ended without one (re-check RC-GK2 I1). A failing step gives
    it back at once."""
    ctx["golden_lease"] = golden_lane(db, ctx)
    try:
        written = _run_generators(cand, dict(ctx, holder=golden_speaker(db, ctx)), hits, err)
        if written:
            git(cand, "add", "--", *written, check=True)
            body = out(cand, "log", "-1", "--format=%B").rstrip("\n")
            msg = "%s\nGoldens: %s rewritten after the cascade (%s)\n" % (
                body, ", ".join(written), ", ".join(spec["switch"] for spec, _ in hits))
            # The secret guard (pre-commit hook) runs on the amended commit too.
            git(cand, "commit", "-q", "--amend", "-m", msg, check=True)
            err.write("merge: goldens rewritten after the cascade: %s\n" % ", ".join(written))
            err.flush()
    except BaseException:
        golden_give_back(db, ctx, err)
        raise
    return out(cand, "rev-parse", "HEAD"), written


def _run_generators(cand, ctx, hits, err):
    """The generator of each hit golden, in `cand`, with its own switch only;
    returns the paths it changed under the golden directory."""
    written = []
    for spec, inside in hits:
        err.write("merge: the cascade moved %d file(s) in the closure of %s/ (%s) -- running "
                  "its generator (%s=1)\n" % (len(inside), spec["dir"], ", ".join(inside[:4])
                                              + (", ..." if len(inside) > 4 else ""),
                                              spec["switch"]))
        err.flush()
        env = {k: v for k, v in os.environ.items() if k not in GOLDEN_SWITCHES}
        env[spec["switch"]] = "1"
        proc = subprocess.run(golden_argv(cand, ctx, spec), cwd=cand, env=env,
                              capture_output=True, text=True)
        if proc.returncode != 0:
            raise Refused("golden_failed", "the generator of %s/ (%s=1) exit %d: %s" % (
                spec["dir"], spec["switch"], proc.returncode,
                (proc.stdout + proc.stderr).strip()[-400:]))
        changed = [l[3:] for l in git(cand, "status", "--porcelain", "--untracked-files=all",
                                      "--", spec["dir"]).stdout.splitlines() if len(l) > 3]
        written += sorted(changed)
    return written


def clean_roadmap(cand, issues):
    """`ROADMAP.md` § Now without the lines that name a closed issue (PREAMBLE § 5)."""
    path = os.path.join(cand, "ROADMAP.md")
    if not issues or not os.path.isfile(path):
        return False
    with open(path, encoding="utf-8") as fh:
        lines = fh.read().split("\n")
    keep, now, changed = [], False, False
    rx = re.compile(r"#(%s)\b" % "|".join(re.escape(i) for i in issues))
    for line in lines:
        if line.startswith("## "):
            now = line.strip() == "## Now"
        if now and line.lstrip().startswith(("- ", "* ")) and rx.search(line):
            changed = True
            continue
        keep.append(line)
    if changed:
        with open(path, "w", encoding="utf-8") as fh:
            fh.write("\n".join(keep))
    return changed


def message(strands, result, satellites=()):
    names = ", ".join(s.name for s in strands)
    issues = ", ".join("#%s" % i for s in strands for i in s.issues)
    title = strands[0].title if len(strands) == 1 else "batch"
    lines = ["Merge %s (%s): %s" % (names, issues, title), "",
             versions.version_line(result), versions.pins_line(result),
             versions.left_line(result)]
    if satellites:
        lines.append("Satellite pins needed: %s" % ", ".join(
            "%s %s@%s" % s for s in satellites))
    return "\n".join(lines) + "\n"


def candidate_versions(cand):
    """The `versions` module that numbers `cand`: the candidate's own.

    GH #1169 (queue RED of tb-s8, gate c58bcf1 9/11): the strand taught the
    cascade AND the pin-lock a new pin form (the escaped pin of an inlined
    script); the merge point numbered the candidate with the kit that runs the
    merge (master's cascade, blind to the form) and the merge gate judged it
    with the candidate's pin-lock -- `display@2.10.8 not provided -- raise to
    2.10.9`. The gate tests the candidate with the candidate's code, so the
    numbers come from the same code: when the candidate's
    `scripts/pipeline/versions.py` or `locks.py` differs from the running kit's,
    both load from the candidate under a private package name (stdlib-only,
    they import each other and nothing else). Otherwise the running module."""
    here = os.path.dirname(os.path.abspath(__file__))
    pkg = os.path.join(cand, "scripts", "pipeline")

    def same(fn):
        try:
            with open(os.path.join(pkg, fn), "rb") as a, open(os.path.join(here, fn), "rb") as b:
                return a.read() == b.read()
        except OSError:
            return None
    verdicts = [same(fn) for fn in ("versions.py", "locks.py")]
    if None in verdicts or all(verdicts):
        return versions
    name = "_merge_candidate_%s" % uuid.uuid4().hex
    spec = importlib.util.spec_from_file_location(
        name, os.path.join(pkg, "__init__.py"), submodule_search_locations=[pkg])
    package = importlib.util.module_from_spec(spec)
    sys.modules[name] = package
    try:
        spec.loader.exec_module(package)
        for sub in ("locks", "versions"):
            subspec = importlib.util.spec_from_file_location(
                "%s.%s" % (name, sub), os.path.join(pkg, sub + ".py"))
            mod = importlib.util.module_from_spec(subspec)
            sys.modules[subspec.name] = mod
            subspec.loader.exec_module(mod)
            setattr(package, sub, mod)
    except Exception as exc:  # noqa: BLE001 -- a broken kit refuses its own merge
        raise versions.VersionError("the candidate's versions module does not load: %s" % exc)
    return sys.modules["%s.versions" % name]


def build_candidate(root, base, strands, label, extra_kinds=None):
    """Merge, number, cascade and commit; returns `(path, sha, result, satellites)`."""
    cand = open_candidate(root, base, label)
    try:
        tips = [out(root, "rev-parse", s.branch) for s in strands]
        resolved = merge_into(cand, tips)
        changed = out(cand, "diff", "--name-only", "--cached", base).split()
        kinds = {}
        for s in strands:
            for name, kind in s.doc["kind"].items():
                rank = versions.KINDS.index
                if name not in kinds or rank(kind) > rank(kinds[name]):
                    kinds[name] = kind
        kinds.update(extra_kinds or {})
        own = candidate_versions(cand)
        try:
            result = own.assign(cand, base, changed, kinds, repo=root,
                                before_round=regenerate_corpus,
                                history={"issues": [i for s in strands for i in s.issues]})
            repos = [p for p in os.environ.get("MECLAW_SATELLITE_REPOS", "").split(os.pathsep)
                     if p]
            moved = {n: (v["from"], v["to"]) for n, v in result["versions"].items()}
            satellites = own.satellite_pins(repos, moved)
            own.add_changelog(cand, own.changelog_line(
                result, [i for s in strands for i in s.issues]))
        except own.VersionError as exc:
            if own is versions:
                raise
            # the caller knows the running kit's class, not the candidate's
            raise versions.VersionError(str(exc)) from exc
        clean_roadmap(cand, [i for s in strands for i in s.issues])
        if resolved:
            result["resolved"] = resolved
        # V2-A17: what the merge point itself wrote (numbers, cascade, corpus,
        # changelog) -- the working tree against the merged index.
        result["merge_point"] = out(cand, "diff", "--name-only").split()
        git(cand, "add", "-u", check=True)
        # The secret guard (pre-commit hook) runs on the candidate commit too.
        git(cand, "commit", "-q", "-m", message(strands, result, satellites), check=True)
        return cand, out(cand, "rev-parse", "HEAD"), result, satellites
    except BaseException:
        close_candidate(root, cand)
        raise


# --- the gate ---------------------------------------------------------------------------

def _fake_gate(spec, ctx):
    log = os.environ.get("MECLAW_PIPELINE_FAKE_GATE_LOG")
    if log:
        with open(log, "a", encoding="utf-8") as fh:
            fh.write(json.dumps(ctx, sort_keys=True) + "\n")
    verdict = spec.upper()
    if spec.startswith("red-if:"):
        bad = set(spec[len("red-if:"):].split(","))
        verdict = "RED" if bad & set(ctx["strands"]) else "GREEN"
    if verdict not in ("GREEN", "RED", "ABORTED"):
        raise Refused("gate_unknown", "MECLAW_PIPELINE_FAKE_GATE=%s" % spec)
    return {"verdict": verdict, "line": "GATE-SUMMARY merge %s 1/1 0s %s" % (
        ctx["rev"][:7], verdict), "secs": 0.0}


# The host-file words a lane run reads from its environment (as `strand.sh
# lane_settings` exports them).
_LANE_KV = {"threads": "MECLAW_LANE_THREADS", "class": "MECLAW_LANE_CLASS",
            "slots": "MECLAW_LANE_SLOTS", "jobs": "MECLAW_LANE_JOBS",
            "watch": "MECLAW_LANE_WATCH", "sccache": "MECLAW_LANE_SCCACHE",
            "linker": "MECLAW_LANE_LINKER"}


def _lane(lane):
    """`(ssh-target, env)` of a lane in the host file, env as `strand.sh` sets it."""
    path = os.environ.get("MECLAW_LANES_FILE") or os.path.expanduser("~/.config/meclaw/lanes")
    try:
        with open(path, encoding="utf-8") as fh:
            for line in fh:
                words = line.split("#", 1)[0].split()
                if len(words) >= 2 and words[0] == lane:
                    env = {"MECLAW_LANE_NAME": lane, "MECLAW_LANE_CARGO": "1"}
                    env.update({v: "" for v in _LANE_KV.values()})
                    for w in words[2:]:
                        key, _, val = w.partition("=")
                        if key in _LANE_KV and val:
                            env[_LANE_KV[key]] = val
                        elif key == "root" and val and words[1] == "local":
                            env["MECLAW_LANE_ROOT"] = val
                    if words[1] == "local" and "MECLAW_LANE_ROOT" not in env:
                        if len(words) >= 3 and words[2].startswith("/"):
                            env["MECLAW_LANE_ROOT"] = words[2]
                    return words[1], env
    except OSError:
        pass
    raise Refused("no_lane", "lane %s is not in the host file" % lane)


def _lane_target(lane):
    return _lane(lane)[0]


# What the gate says about its lane (V2-A10): its queue line on stderr
# (`gate.py _run_lane`) and the `lock-wait` NOTE row (`executor._take_lock`).
LANE_QUEUED = re.compile(r"^gate: queued -- (.*)$")
LOCK_WAIT = re.compile(r"^GATE lock-wait \[[^\]]*\] (\d+(?:\.\d+)?)s NOTE\b")
# Re-check v2-r3-lease m2: under `merge --wait` the gate waits in the admission
# (`--wait-admission`) and says it on stderr when the wait ends (`gate.py _run_lane`).
ADMISSION_WAIT = re.compile(r"^gate: admission wait (\d+)s(?: \((.*)\))?$")


def _summary_rev(line):
    words = line.split()
    return words[2] if len(words) >= 3 else ""


MIN_REV = 12


def _full_rev(text):
    """The full rev the gate prints (`gate: rev = <sha>`), or ''."""
    revs = [l.split("=", 1)[1].strip() for l in text.splitlines()
            if l.startswith("gate: rev = ")]
    return revs[-1] if revs else ""


def _check_rev(ctx, rev, what):
    """Fail closed: a gate verdict counts only for the candidate's own commit,
    named by at least MIN_REV characters (a short prefix names many commits)."""
    if len(rev or "") < min(MIN_REV, len(ctx["rev"])) or not ctx["rev"].startswith(rev):
        raise Refused("gate_foreign_rev", "%s names rev %s, the candidate is %s" % (
            what, rev or "?", ctx["rev"][:9]))


def gate_holder(strands):
    """The lease holder the merge gate names (`--strand`): the strand's own lease
    `<wave>/<S>` -- for a batch `<wave>/S1+S2`, which speaks for its members
    (schedule `_batch_speaker`). Kit find (h), ledger 03:35: without it the gate
    in the detached merge tree had the holder "" -- refused on the strand's own
    lane, and its refusal left a waiter nobody takes. A single strand names its
    branch, the lease `strand.sh token take` takes without `--strand`; a lease
    taken with `--strand S1` from a branch `<wave>/S1-fix` is not found -- the
    gate then waits or takes its own lane (exit 3), it never takes a foreign
    lease (re-check fix5 H2: liveness only, fail-closed)."""
    first = strands[0].branch or ""
    if len(strands) == 1 and "/" in first:
        return first   # re-check fix4 H3: the lease `strand.sh token` takes is the branch
    if "/" in first:
        wave = first.split("/", 1)[0]
    else:
        wave = re.sub(r"-\d{4}-\d{2}-\d{2}$", "", os.path.basename(strands[0].wdir.rstrip("/")))
    return "%s/%s" % (wave, "+".join(s.name for s in strands))


def gate_argv(tree, ctx):
    # K13 (16.10.): `merge --wait` waits for the tree lease and so for the lane
    # of its candidate run too -- the gate queues (`--wait-admission`, a queue
    # place it comes back for); without `--wait` it takes none (kit find (h)).
    wait = ["--wait-admission"] if ctx.get("wait") else []
    custom = os.environ.get("MECLAW_MERGE_GATE_CMD")
    if custom:
        words = [w.format(tree=tree, base=ctx.get("gate_base") or ctx["base"],
                          strand=ctx["label"], holder=ctx.get("holder") or "",
                          wait=" ".join(wait))
                 for w in shlex.split(custom)]
        return [w for w in words if w]
    own = ["--strand", ctx["holder"]] if ctx.get("holder") else []
    # GH #1167 (b): the gate plans from `gate_base` -- the strand tip when the
    # strand receipt covers it, else master (the full strand diff).
    base = ctx.get("gate_base") or ctx["base"]
    if ctx.get("host"):
        _lane_target(ctx["host"])  # an unknown lane refuses here (`no_lane`)
        return [os.path.join(tree, "scripts", "gate.sh"), "strand", "--host", ctx["host"],
                "--base", base] + own + wait
    return [os.path.join(tree, "scripts", "gate.sh"), "strand", "--base", base] + own + wait


def run_gate(tree, ctx):
    """The merge gate on the committed candidate: `{verdict, line, secs}`."""
    fake = os.environ.get("MECLAW_PIPELINE_FAKE_GATE")
    if fake:
        return _fake_gate(fake, ctx)
    # The merge plans unnarrowed (review Z-I1 C2): MECLAW_GATE_NARROW=0 -- except
    # a merge of ONE strand that names its GREEN strand run: the gate's
    # `narrow.merge_decision` checks it, fail-closed (welle-tb S1 part B, #1159).
    env = dict(os.environ, MECLAW_GATE_MODE="merge",
               MECLAW_ALLOW_BUMP="merge point of %s" % ctx["label"],
               MECLAW_GATE_CACHE_FROM=",".join(ctx.get("cache_from") or []))
    env.pop("MECLAW_GATE_STRAND_RUN", None)
    env.pop("MECLAW_GATE_WAVE", None)
    if ctx.get("wave"):  # the detached merge tree names its wave: full_pass (#1165)
        env["MECLAW_GATE_WAVE"] = ctx["wave"]
    if ctx.get("strand_run") and len(ctx.get("strands") or []) == 1:
        env["MECLAW_GATE_STRAND_RUN"] = ctx["strand_run"]
    else:
        env["MECLAW_GATE_NARROW"] = "0"
    t0 = datetime.datetime.now()
    proc = subprocess.run(gate_argv(tree, ctx), cwd=tree, env=env, capture_output=True, text=True)
    secs = (datetime.datetime.now() - t0).total_seconds()
    lines = [l for l in proc.stdout.splitlines() if l.startswith("GATE-SUMMARY ")]
    line = lines[-1] if lines else ""
    # No summary, no verdict: the engine's exit 3 (lane waits, lease lock), 5
    # (build path busy) and 6 (store error at the admission) gated nothing,
    # and exit 1 without a summary is a crash, never a red verdict (the engine
    # prints one with every RED, GH #1154) -- aborted, not judged, no bisection.
    verdict = line.split()[-1] if line else "ABORTED"
    if verdict not in ("GREEN", "RED", "ABORTED", "ASK"):
        verdict = "RED"
    log = os.path.join(state_dir(), "merge", "%s.gate.log" % ctx["label"])
    with open(log, "w", encoding="utf-8") as fh:
        fh.write(proc.stdout + proc.stderr)
    if line:
        _check_rev(ctx, _full_rev(proc.stdout) or _summary_rev(line), "the gate summary")
    # V2-A10: the candidate run's wait for its lane. Without `--wait` the merge
    # gate takes no queue place (kit find h): a lane it cannot have ends it at once (exit 3,
    # `gate: queued -- <reason>` went to the gate log only); a lane whose cargo
    # lock other runs hold makes it wait -- the gate's `lock-wait` row.
    lane = [m.group(1) for m in map(LANE_QUEUED.match, proc.stderr.splitlines()) if m]
    waits = [float(m.group(1)) for m in map(LOCK_WAIT.match, proc.stdout.splitlines()) if m]
    admitted = [m for m in map(ADMISSION_WAIT.match, proc.stderr.splitlines()) if m]
    adm_s = sum(int(m.group(1)) for m in admitted)
    if not line and lane:
        text = "gate exit %d without a summary (not judged): no lane free for the candidate " \
               "run: %s" % (proc.returncode, lane[-1])
    elif not line:
        text = "gate exit %d without a summary (not judged)" % proc.returncode
    else:
        text = line
    return {"verdict": verdict, "line": text, "secs": secs, "log": log,
            "lane_wait_s": int(sum(waits)) + adm_s, "admission_wait_s": adm_s,
            "admission_why": (admitted[-1].group(2) or "") if admitted else ""}


# --- the operation ----------------------------------------------------------------------

def fast_forward(root, base, sha):
    """`git merge --ff-only` in the main tree -- master and its working tree move."""
    if out(root, "symbolic-ref", "--short", "HEAD") != "master":
        raise Refused("main_not_master", "the main tree has no master checked out")
    if out(root, "rev-parse", "master") != base:
        raise Refused("master_moved", "master moved under the lease")
    aside = set_aside(root, base, sha)
    res = git(root, "merge", "--ff-only", "-q", sha)
    if res.returncode != 0:
        for src, dst in aside:
            shutil.move(dst, src)
        raise Refused("main_tree_dirty", res.stderr.strip()[:300])


def set_aside(root, base, sha):
    """Move untracked main-tree files the merge adds BYTE FOR BYTE to the state
    dir, so `--ff-only` does not trip over a copy of a strand's own report.
    A differing file stays and the fast-forward refuses. Returns `[(src, dst)]`."""
    added = out(root, "diff", "--name-only", "--no-renames", "--diff-filter=A", base, sha)
    stamp = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    moved = []
    for rel in (p for p in added.splitlines() if p):
        src = os.path.join(root, rel)
        if not os.path.isfile(src) or os.path.islink(src) or git(
                root, "ls-files", "--error-unmatch", "--", rel).returncode == 0:
            continue
        blob = subprocess.run(["git", "-C", root, "cat-file", "blob", "%s:%s" % (sha, rel)],
                              capture_output=True)
        with open(src, "rb") as fh:
            if blob.returncode != 0 or fh.read() != blob.stdout:
                continue
        dst = os.path.join(state_dir(), "aside", "%s-%s" % (stamp, sha[:8]), rel)
        os.makedirs(os.path.dirname(dst), exist_ok=True)
        shutil.move(src, dst)
        moved.append((src, dst))
    return moved


def wait_s(ctx, name):
    """The receipt's `wait_s` (V2-A10): the strand's wait for the tree lease
    plus the candidate runs' waits for their lane; 0 without any."""
    return (ctx.get("wait_s") or {}).get(name, 0) + (ctx.get("lane_wait_s") or 0)


class Steps:
    """The steps of one merge (V2-A10 h, wave validation 2): the merge of
    v2-fix-b held tree:master 826 s (journal seq 116541 lease_taken
    00:03:20Z -> 116542 merge_gated 00:17:06Z) without a word -- which step
    ate the time stood nowhere. Each step says `merge: step <name> <start>
    <secs>s` when it ends (` RED` after a step that refused) and the merge
    receipt lists them as `steps`. Contiguous: a step starts where the one
    before ended, on the kit clock of the merge (`merge(clock=...)`)."""

    def __init__(self, clock, err):
        self.clock, self.err = clock, err
        self.rows, self.current, self.mark = [], None, clock()

    def at(self, name):
        self.current = name

    def done(self, verdict=None):
        if self.current is None:
            return
        end = self.clock()
        secs = int((store.parse_at(end) - store.parse_at(self.mark)).total_seconds())
        row = {"name": self.current, "start": self.mark, "secs": secs}
        if verdict:
            row["verdict"] = verdict
        self.rows.append(row)
        self.err.write("merge: step %s %s %ds%s\n" % (self.current, self.mark, secs,
                                                       " %s" % verdict if verdict else ""))
        self.err.flush()
        self.current, self.mark = None, end

    def fail(self):
        self.done("RED")


def receipt_steps(path, rows):
    """Set the receipt's `steps` once the merge ends (the close step runs after
    the receipt is written)."""
    with open(path, encoding="utf-8") as fh:
        doc = json.load(fh)
    doc["steps"] = list(rows)
    with open(path, "w", encoding="utf-8") as fh:
        json.dump(doc, fh, indent=1, sort_keys=True)
        fh.write("\n")


def merge_receipt(ctx, wdir, label, sha, doc):
    """`write_receipt` with the steps so far; remembers the file for `receipt_steps`."""
    doc["steps"] = list(ctx["steps"].rows) if ctx.get("steps") else []
    d = write_receipt(wdir, label, sha, doc)
    ctx.setdefault("receipt_files", []).append(
        os.path.join(d, "%s.json" % label.split("-", 1)[0]))
    return d


def steps_of(ctx):
    """The merge's `Steps`, or a silent stand-in for callers without one."""
    return ctx.get("steps") or Steps(lambda: "1970-01-01T00:00:00Z", io.StringIO())


def write_receipt(wdir, label, sha, doc):
    at = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    d = os.path.join(wdir, "receipts", label, "%s-%s" % (at, (sha or "none")[:8]))
    os.makedirs(d, exist_ok=True)
    with open(os.path.join(d, "%s.json" % label.split("-", 1)[0]), "w", encoding="utf-8") as fh:
        json.dump(doc, fh, indent=1, sort_keys=True)
        fh.write("\n")
    with open(os.path.join(d, "summary.txt"), "w", encoding="utf-8") as fh:
        fh.write("%s\n" % doc.get("summary", ""))
    return d


def gate_base(root, base, strands, ctx):
    """GH #1167 (b): `(rev, why)` the merge gate plans from. One strand whose
    receipt covers its tip and whose base lies on master: the tip. A batch or
    any doubt: master -- the full gate. OR-TM-9: a strand whose gate planned no
    cargo station plans from master too -- the candidate's diff is the strand's
    (plus the merge point's edits), its plan the strand gate's; from the tip
    it was master's move since the strand base (K-APPID 4bb6d28c1: 132 files
    of code, 23/23 stations for AGENTS.md and plans/), or nothing, which the
    resolver floors to `build` + t0."""
    if len(strands) != 1:
        return base, "full: a batch of %d strands" % len(strands)
    s = strands[0]
    rec = ctx["receipts"][s.name]
    tip = out(root, "rev-parse", "%s^{commit}" % s.branch)
    if not tip or rec.get("no_pre"):
        return base, "full: %s" % ("no branch tip" if not tip else "merged with --no-pre")
    if _cargo_free(rec):
        return base, "strand diff: the strand gate planned no cargo station (OR-TM-9)"
    return turnaround.merge_gate_base(root, base, tip, rec)


def _cargo_free(rec):
    """Did the strand gate of receipt `rec` plan stations, none of them a cargo
    station (`schedule._needs`, the gate's `cargo=` rule)? Its `plan.json`
    decides. Fail closed: none, one without stations, or a station without its
    `needs` (a plan that does not say what it books) is no answer (False)."""
    doc = _plan_doc(rec["path"]) if rec.get("path") else None
    stations = (doc or {}).get("stations")
    if not isinstance(stations, list) or not stations:
        return False
    return all(isinstance(st, dict) and isinstance(st.get("needs"), dict)
               and schedule._needs(st)["cargo"] < 1 for st in stations)


def land_plan_only(db, ctx, s, base):
    """GH #1167 (d): a plan-only branch -- merge commit, no versions, no gate.
    The candidate's own diff is checked again: gate-neutral or refused."""
    root = ctx["root"]
    mid = ctx["mids"][s.name]
    steps = steps_of(ctx)
    steps.at("candidate")
    cand = open_candidate(root, base, s.name)
    try:
        tip = out(root, "rev-parse", s.branch)
        merge_into(cand, [tip])
        paths = out(cand, "diff", "--name-only", "--no-renames", "--cached", base).split()
        bad = turnaround.offending(paths, root, os.path.basename(s.wdir), **_neutral(s, base)[1])
        if not paths or bad:
            raise Refused("plan_only_not_neutral", "the merge of %s touches %s" % (
                s.name, turnaround.describe(bad) if bad else "nothing"))
        issues = ", ".join("#%s" % i for i in s.issues)
        git(cand, "commit", "-q", "-m", "Merge %s%s: plan-only, no gate (%d gate-neutral files)\n"
            % (s.name, " (%s)" % issues if issues else "", len(paths)), check=True)
        sha = out(cand, "rev-parse", "HEAD")
        steps.done()
        steps.at("ref-update")
        hold_tree(db, ctx["repo"], ctx["holder"])
        fast_forward(root, base, sha)
        steps.done()
    except Refused as exc:
        steps.fail()
        return [(s.name, "refused", exc.reason, exc.text)]
    finally:
        close_candidate(root, cand)
    line = "MERGE-PLAN-ONLY %s %s %d files GREEN" % (s.name, sha[:9], len(paths))
    _set_merge(db, mid, state="merged", merge_sha=sha)
    _event(db, "merge_done", mid, s.name, base, merge_sha=sha, plan_only=True)
    merge_receipt(ctx, s.wdir, "merge-%s" % s.name, sha, {
        "strand": s.name, "base": base, "merge_sha": sha, "plan_only": True,
        "files": paths, "gate": {"verdict": "SKIPPED", "line": "plan-only: gate-neutral"},
        "wait_s": wait_s(ctx, s.name), "summary": line})
    ctx["out"].write("%s\n" % line)
    return [(s.name, "merged", None, sha)]


def land(db, ctx, strands, base, depth=0):
    """Candidate -> gate -> fast-forward for `strands`; bisects a red batch.

    Returns `[(strand, state, reason)]`."""
    root, repo = ctx["root"], ctx["repo"]
    label = "+".join(s.name for s in strands)
    mids = {s.name: ctx["mids"][s.name] for s in strands}
    steps = steps_of(ctx)
    steps.at("candidate")
    try:
        cand, sha, result, sats = build_candidate(root, base, strands, label)
    except Refused as exc:
        steps.fail()
        if exc.reason == "batch_conflict" or len(strands) == 1:
            return [(s.name, "refused", exc.reason, exc.text) for s in strands]
        raise
    except versions.VersionError as exc:
        steps.fail()
        return [(s.name, "refused", "version_conflict", str(exc)) for s in strands]
    resolved = result.get("resolved")
    if resolved:
        ctx["err"].write("merge: resolved %s\n" % resolved)
    gctx = {"rev": sha, "base": base, "label": label, "strands": [s.name for s in strands],
            "cache_from": [ctx["receipts"][s.name]["run"] for s in strands],
            "strand_run": (ctx["receipts"][strands[0].name]["run"] if len(strands) == 1
                           else None),
            "host": ctx.get("host"), "wave": os.path.basename(strands[0].wdir.rstrip("/")),
            "holder": gate_holder(strands), "wait": bool(ctx.get("wait"))}
    hits = golden_hits(cand, result.get("merge_point") or [])
    goldens = None
    if hits:
        # V2-A17: the goldens follow the merge point's own writes, in the
        # merge commit, before the gate judges it byte for byte.
        steps.done()
        steps.at("golden")
        try:
            hold_tree(db, repo, ctx["holder"])
            sha, goldens = follow_goldens(cand, gctx, hits, ctx["err"], db=db)
        except Refused as exc:
            steps.fail()
            close_candidate(root, cand)
            return [(s.name, "refused", exc.reason, exc.text) for s in strands]
        gctx["rev"] = sha
    try:
        gctx["gate_base"], gctx["gate_reuse"] = gate_base(root, base, strands, ctx)
        for s in strands:
            _set_merge(db, mids[s.name], state="gating", versions=result["versions"],
                       pins=result["pins"])
            _event(db, "merge_gated", mids[s.name], s.name, base, merge_sha=sha,
                   versions=result["versions"], resolved=resolved)
        steps.done()
        steps.at("gate")
        # N3: every gate run starts on a renewed lease -- a bisection runs
        # several gates under one lease, each up to TTL_S long.
        hold_tree(db, repo, ctx["holder"])
        try:
            # RC-GK2 I1: the generator's lane lease spans the gate -- its
            # admission takes the held lane -- and goes back before a bisection.
            verdict = run_gate(cand, gctx)
        finally:
            golden_give_back(db, gctx, ctx["err"])
        ctx["out"].write("%s\n" % verdict["line"])
        lane_s = verdict.get("lane_wait_s") or 0
        adm_s = verdict.get("admission_wait_s") or 0
        if lane_s:
            ctx["lane_wait_s"] = ctx.get("lane_wait_s", 0) + lane_s
        if adm_s:
            why = verdict.get("admission_why") or ""
            ctx["err"].write("merge: waited %ds for the admission of the candidate run%s\n"
                             % (adm_s, " (%s)" % why if why else ""))
        if lane_s - adm_s:
            ctx["err"].write("merge: waited %ds for the lane of the candidate run (behind other "
                             "runs' cargo lock)\n" % (lane_s - adm_s))
        if verdict["verdict"] != "GREEN":
            steps.done(verdict["verdict"])
            reason = "gate_aborted" if verdict["verdict"] == "ABORTED" else "gate_red"
            if len(strands) > 1 and depth < MAX_BISECT and verdict["verdict"] == "RED":
                close_candidate(root, cand)
                cand = None
                half = len(strands) // 2
                done = land(db, ctx, strands[:half], base, depth + 1)
                now = out(root, "rev-parse", "master")
                return done + land(db, ctx, strands[half:], now, depth + 1)
            return [(s.name, "refused", reason, verdict["line"]) for s in strands]
        steps.done()
        steps.at("ref-update")
        hold_tree(db, repo, ctx["holder"])
        fast_forward(root, base, sha)
        steps.done()
    except Refused as exc:
        steps.fail()
        return [(s.name, "refused", exc.reason, exc.text) for s in strands]
    finally:
        golden_give_back(db, gctx, ctx["err"])
        if cand:
            close_candidate(root, cand)
    for s in strands:
        _set_merge(db, mids[s.name], state="merged", merge_sha=sha)
        _event(db, "merge_done", mids[s.name], s.name, base, merge_sha=sha,
               versions=result["versions"], pins=result["pins"], resolved=resolved)
        for repo_name, template, version in sats:
            events.emit(db, "satellite_pin_needed", "finding", {
                "merge": mids[s.name], "repo": repo_name, "template": template,
                "version": version})
        doc = {
            "strand": s.name, "base": base, "merge_sha": sha, "versions": result["versions"],
            "workspace": result["workspace"], "pins": result["pins"], "gate": verdict,
            "wait_s": wait_s(ctx, s.name),
            "summary": "MERGE %s %s %s GREEN" % (s.name, sha[:9], versions.version_line(result))}
        if goldens is not None:
            doc["goldens"] = goldens
        if resolved:
            doc["resolved"] = resolved
        merge_receipt(ctx, s.wdir, "merge-%s" % s.name, sha, doc)
    ctx["out"].write("%s\n" % versions.version_line(result))
    ctx["out"].write("%s\n" % versions.pins_line(result))
    ctx["out"].write("%s\n" % versions.left_line(result))
    return [(s.name, "merged", None, sha) for s in strands]


def waited(db, repo, mids, fresh):
    """`{strand: seconds}` between the queue row and the tree lease just taken
    (V2-A10, `wait_s` of the receipt): the store's own time stamps, the kit
    clock. A row this call made and took at once (`fresh`) never waited: 0."""
    taken = (store.lease_who(db, resource(repo)) or {}).get("taken_at")
    out_ = {}
    for name, mid in mids.items():
        row = _merge_row(db, mid)
        if name in fresh or not taken or not row:
            out_[name] = 0
        else:
            out_[name] = max(0, int((store.parse_at(taken)
                                     - store.parse_at(row["at"])).total_seconds()))
    return out_


def merge(names, repo="meclaw", start=None, wave=None, host=None, no_pre=None, close=True,
          db=None, out_=None, err=None, wait=False, clock=None):
    """`pipeline.py merge`: returns the exit code (0 merged, 1 refused, 3 queued).

    `wait` (`--wait`) blocks in the queue instead of exit 3 (`take_tree_waiting`).
    `clock` is the kit clock (`store.now`) of the queue row and the lease take:
    the receipt's `wait_s` is the time between them (V2-A10)."""
    out_, err = out_ or sys.stdout, err or sys.stderr
    clock = clock or store.now
    root = main_root(start or os.getcwd())
    own_db = db is None
    db = db or store.open()
    names = [strand_name(n) for n in names]
    holder = "+".join(names)
    strands, mids, created = [], {}, set()
    base = out(root, "rev-parse", "master")
    term = trap_term()
    taken = False
    steps = None
    try:
        strands = [Strand(root, n, wave) for n in names]
        for s in strands:
            row = db.execute("SELECT id FROM merges WHERE repo=? AND strand=? AND state='queued'"
                             " ORDER BY at DESC LIMIT 1", (repo, s.name)).fetchone()
            if row:
                mids[s.name] = row["id"]
            else:
                mids[s.name] = _new_merge(db, repo, s.name, base, "queued", clock())
                created.add(s.name)
        def queued():
            for s in strands:
                _event(db, "merge_queued", mids[s.name], s.name, base)
            who = store.lease_who(db, resource(repo)) or {}
            err.write("merge: tree:master is held by %s -- queued, %s\n" % (
                who.get("holder") or who.get("offered_to"),
                "waiting for its release (--wait)" if wait
                else "the release offers it to the oldest queued merge"))

        said, lanes_said = [], []

        def waits(first):
            # V2-A10 (wave validation 2): the merge of v2-fix-b waited 14 min
            # without a word. One line when the wait starts, one per change of
            # holder or position -- a round follows a ring, never a clock.
            # A strand refused before any wait (PREAMBLE § 6 rule 5) keeps its
            # lane: `lease_resolves` only reads, so it goes first (re-check
            # stack fr I1). OR-TM-6: no lane while the merge waits -- before
            # its wait line and the first round's events (re-check stack m1):
            # `while_locked` repeats a round its write found locked, the
            # give-back is idempotent (after it the holder has no lane), and
            # the round's events are one transaction (`_events`) -- a member's
            # event that found the store locked leaves none of them (re-check
            # stack fr m2).
            if first:
                lease_resolves(strands, base, no_pre)
            give_back_lanes(db, repo, strands, err, lanes_said)
            if first:
                _events(db, "merge_queued", [(mids[s.name], s.name) for s in strands], base)
            state = wait_state(db, repo, holder)
            if not said or said[-1] != state:
                said.append(state)
                err.write("%s\n" % wait_line(repo, holder, state))
                err.flush()
        steps = Steps(clock, err)
        steps.at("lease")
        try:
            if wait:
                taken = take_tree_waiting(db, repo, holder, on_queued=waits, clock=clock)
            else:
                taken = take_tree(db, repo, holder, at=clock())
        except Refused as exc:
            steps.fail()
            if exc.reason != "already_merging":
                raise
            # N2: before 'gating' the running merge's row is still 'queued' and
            # was picked up above -- it belongs to the run that holds the
            # lease. Only the rows this call created are refused.
            mids = {n: m for n, m in mids.items() if n in created}
            raise
        steps.done(None if taken else "QUEUED")
        if not taken:
            if wait:
                err.write("merge: no tree lease within %g s (MECLAW_ADMISSION_WAIT_SECS) -- "
                          "still queued\n" % schedule.wait_secs(
                              os.environ, "MECLAW_ADMISSION_WAIT_SECS", "14400"))
            else:
                queued()
            return QUEUED_EXIT
        # master as it stands under the lease -- the previous holder may have
        # fast-forwarded it between the first read and the take.
        base = out(root, "rev-parse", "master")
        ctx = {"root": root, "repo": repo, "host": host, "mids": mids, "out": out_,
               "receipts": {}, "holder": holder, "err": err, "steps": steps, "wait": wait,
               "wait_s": waited(db, repo, mids, created if not said else set())}
        ready, plans_only, tips = [], [], {}
        steps.at("preconditions")
        for s in strands:
            _set_merge(db, mids[s.name], base=base)
            try:
                tip = out(root, "rev-parse", "--verify", "--quiet", "%s^{commit}" % s.branch)
                if not tip:
                    raise Refused("no_branch", "no branch %s" % s.branch)
                review_first(s, base, tip, no_pre)
                tips[s.name] = tip
            except Refused as exc:
                _refuse(db, mids[s.name], s.name, base, exc.reason, exc.text, err)
        migrating = open_migrations(root, base) if tips else {}
        for s in strands:
            if s.name not in tips:
                continue
            try:
                tip = tips[s.name]
                if plan_only(s, base, tip):
                    # GH #1167 (d): no report, no receipt, no gate -- the
                    # owner check (f) still holds, and a review that is
                    # there and not `Verdikt: ja` refuses (re-check H2 m6).
                    if s.head is not None and REVIEW_HEADING.search(s.body or "") \
                            and not review_approved(s.body):
                        raise Refused("no_review_verdict", "%s: a plan-only branch whose "
                                      "report carries a review without `Verdikt: ja`"
                                      % s.report)
                    verdict, why = turnaround.owner_check(s.root, "%s..%s" % (base, tip))
                    if verdict != "GREEN":
                        raise Refused("foreign_commits", why)
                    plans_only.append(s)
                    continue
                ctx["receipts"][s.name] = preconditions(s, tip, no_pre)
                earlier = {r.name for r in ready}
                migration_order(s.name, s.migrations, {
                    n: m for n, m in migrating.items() if n != s.name and n not in earlier})
                ready.append(s)
            except Refused as exc:
                _refuse(db, mids[s.name], s.name, base, exc.reason, exc.text, err)
        steps.done(None if len(ready) + len(plans_only) == len(strands) else "RED")
        results = []
        for s in plans_only:
            results += land_plan_only(db, ctx, s, base)
            base = out(root, "rev-parse", "master")
        results += land(db, ctx, ready, base) if ready else []
        done = len(ready) + len(plans_only)
        rc = 0 if done and done == len(strands) else REFUSE_EXIT
        if close and any(r[1] == "merged" for r in results):
            steps.at("close")
        for name, state, reason, text in results:
            if state == "refused":
                _refuse(db, mids[name], name, base, reason, text, err)
                rc = REFUSE_EXIT
            else:
                out_.write("MERGED %s %s\n" % (name, text[:9]))
                if close:
                    # master has moved: a close that fails is a note for
                    # `strand.sh close`, never a refusal of a done merge.
                    s = next(x for x in strands if x.name == name)
                    try:
                        if close_issues(s, do=True, out_=out_, err=err):
                            err.write("merge: %s merged, the issue close failed -- run "
                                      "strand.sh close\n" % name)
                    except Refused as exc:
                        err.write("merge: %s merged, close refused (%s): %s\n"
                                  % (name, exc.reason, exc.text))
        steps.done()
        for path in ctx.get("receipt_files", ()):
            receipt_steps(path, steps.rows)
        return rc
    except Refused as exc:
        if steps:
            steps.fail()
        err.write("merge: refused (%s): %s\n" % (exc.reason, exc.text))
        for s in strands:
            _refuse(db, mids.get(s.name), s.name, base, exc.reason, exc.text, err, quiet=True)
        return REFUSE_EXIT
    finally:
        if taken:
            release_tree(db, repo, holder)
        untrap_term(term)
        if own_db:
            store.close(db)


def _refuse(db, mid, strand, base, reason, text, err, quiet=False):
    if mid:
        _set_merge(db, mid, state="refused")
        _event(db, "merge_refused", mid, strand, base, reason=reason)
    if not quiet:
        err.write("merge: %s refused (%s): %s\n" % (strand, reason, text))


# --- close (GH #1107) -------------------------------------------------------------------

def find_merge(root, branch, tip, name):
    """The merge commit on master that carries the strand, or ''. An empty
    `branch` matches no subject ("" is in every one, fix round 1 M2)."""
    for line in out(root, "log", "--merges", "--first-parent", "--format=%H %P%x09%s",
                    "master").splitlines():
        shas, _, subject = line.partition("\t")
        parts = shas.split()
        named = branch in subject if branch else False
        if (tip and tip in parts[2:]) or named \
                or re.match(r"Merge (\S+, )*%s[ ,]" % re.escape(name), subject):
            return parts[0]
    return ""


def strand_merge(root, branch, name):
    """`(tip, merge commit)` of the strand on `branch`; each '' when absent."""
    tip = out(root, "rev-parse", "--verify", "--quiet", "%s^{commit}" % branch) if branch else ""
    return tip, find_merge(root, branch, tip, name)


def merge_parents(root, report, name):
    """The parents of the merge commit that brought the strand of `report` to
    master, the first parent (master before the merge) first; [] before it."""
    head, _ = head_block(report)
    _, merged = strand_merge(root, (head or {}).get("branch", ""), name)
    return out(root, "rev-list", "--parents", "-n", "1", merged).split()[1:] if merged else []


class _NoAnswer(Exception):
    """A `git merge-base --is-ancestor` that said neither 0 nor 1."""


def came_with(root, parents, branch, err=None):
    """True when the head of `branch` came to master with the merge commit of
    `parents`: an ancestor of a later parent, not of the first one (OR-TM-13).
    A missing branch, or a head that moved on after the merge, did not --
    the fail-safe keeps such a strand a carrier of its issues. So does a
    check without an answer: `git merge-base --is-ancestor` says 0 (an
    ancestor) or 1 (not one); any other exit (128, a signal) keeps the
    strand a carrier, with one line naming it on `err` (fix round 1, I1)."""
    if len(parents) < 2:
        return False
    head = out(root, "rev-parse", "--verify", "--quiet", "refs/heads/%s^{commit}" % branch)
    if not head:
        return False

    def below(rev):
        rc = git(root, "merge-base", "--is-ancestor", head, rev).returncode
        if rc not in (0, 1):
            raise _NoAnswer("git merge-base --is-ancestor %s %s exited rc %d"
                            % (head[:9], rev[:9], rc))
        return rc == 0

    try:
        return not below(parents[0]) and any(below(p) for p in parents[1:])
    except _NoAnswer as exc:
        (err or sys.stderr).write("close: %s kept as a carrier -- %s\n" % (branch, exc))
        return False


def close_info(root, report, archive, name):
    """The English close comment and the issues; raises `Refused`."""
    head, _ = head_block(report)
    if head is None:
        raise Refused("no_report", "%s has no header block" % report)
    branch = head.get("branch", "")
    issues = list_field(head.get("issues"))
    path, doc, line = latest_strand_receipt(archive)
    if doc is None or not line:
        raise Refused("no_strand_receipt", "no strand receipt with a summary under %s" % archive)
    if not line.endswith(" GREEN"):
        raise Refused("strand_gate_not_green", "the newest strand gate is %s" % line.split()[-1])
    tip, merged = strand_merge(root, branch, name)
    if not tip and merged:
        parents = out(root, "rev-list", "--parents", "-n", "1", merged).split()[2:]
        tip = parents[0] if parents else ""
    if not tip:
        raise Refused("no_branch", "neither the branch %s nor a merge of it is here" % branch)
    point = "%s^1" % merged if merged else out(root, "merge-base", "master", tip)
    point = out(root, "rev-parse", point)
    commits = out(root, "log", "--no-merges", "--reverse", "--abbrev=8", "--format=%h",
                  "%s..%s" % (point, tip)).split()
    foreign = [c for c in list_field(head.get("commits"))
               if git(root, "merge-base", "--is-ancestor", c, point).returncode == 0]
    if foreign:
        raise Refused("foreign_commits", "the head block lists %d commit(s) reachable from "
                      "master before the branch point (%s) -- run strand.sh report again"
                      % (len(foreign), ", ".join(foreign[:5])))
    changed = [p for p in out(root, "diff", "--name-only", "%s...%s" % (point, tip)).split() if p]
    lines = ["Built on `%s` from %s." % (branch, point[:8]), "",
             "Gate: `%s`" % line, "Commits: %s" % ", ".join(commits)]
    if changed:
        lines.append("Changed: %s" % ", ".join("`%s`" % c for c in changed))
    if merged:
        lines.append("Merged as %s." % merged[:9])
    return issues, "\n".join(lines), commits


def wave_issues(root, wdir, name, parents=(), mine=None, err=None):
    """`{issue: other strand}` of the issues another strand file of the wave
    carries too -- the wave's (acceptance) issue, which no strand closes
    (Z-I 2c (5)). Read from the main tree's `plans/<wave>/straenge/`.

    `parents` are those of the merge commit of `name` (`merge_parents`). A
    strand whose branch head came with that merge (`came_with`) -- a component
    of a batch, whose strand file lands on master with it -- carries nothing
    foreign: its issues close with this merge (OR-TM-13, T2B left six open).
    The branch is the one `strand.sh new` named (`default_branch`). `mine`
    limits the answer, and the git look-ups, to these issues; `err` gets the
    line of a git check without an answer (`came_with`).

    The limit: a component that landed in the batch as a copy (rebase,
    cherry-pick) instead of a merge of its head stays a foreign carrier and
    its issues stay open -- fail-safe; the standard batch form is merge
    commits of the heads (OR-V2.v2-r3-close.2)."""
    sdir = os.path.join(root, "plans", os.path.basename(wdir.rstrip("/")), "straenge")
    found = {}
    try:
        files = sorted(f for f in os.listdir(sdir) if f.endswith(".json"))
    except OSError:
        return found
    for fn in files:
        other = fn[:-len(".json")]
        if other == name:
            continue
        try:
            with open(os.path.join(sdir, fn), encoding="utf-8") as fh:
                doc = json.load(fh)
        except (OSError, ValueError):
            continue
        if isinstance(doc, dict):
            carried = [i for i in versions.issues_of(doc) if mine is None or i in mine]
            if carried and not came_with(root, parents, default_branch(wdir, other), err):
                for issue in carried:
                    found.setdefault(issue, other)
    return found


def close_issues(s, do=False, out_=None, err=None, gh=None):
    """Close the strand's issues on GitHub (`do`) or print the `gh` lines.

    An issue another strand of the wave carries too stays open ("left open --
    the wave issue"); a strand that came to master with this same merge is no
    such carrier (`wave_issues`)."""
    out_, err = out_ or sys.stdout, err or sys.stderr
    issues, comment, _ = close_info(s.root, s.report, s.archive, s.name)
    gh = gh or os.environ.get("MECLAW_GH", "gh")
    shared = wave_issues(s.root, s.wdir, s.name, merge_parents(s.root, s.report, s.name),
                         issues, err)
    # `"close_issue": false` in the strand file: a carrier issue (V2-A1) --
    # the same English comment, the issue stays open.
    close = versions.close_issue_of(getattr(s, "doc", None) or {})
    for issue in issues:
        if issue in shared:
            out_.write("close: #%s left open -- the wave issue (strand %s carries it too)\n"
                       % (issue, shared[issue]))
            continue
        if close:
            argv = [gh, "issue", "close", issue, "--comment", comment]
        else:
            argv = [gh, "issue", "comment", issue, "--body", comment]
        if do:
            if subprocess.run(argv).returncode != 0:
                err.write("close: gh failed for #%s\n" % issue)
                return 1
            if not close:
                out_.write("close: #%s left open -- commented only (close_issue false)\n"
                           % issue)
        else:
            out_.write("%s\n" % " ".join(shlex.quote(a) if a == comment else a
                                         for a in ["gh"] + argv[1:]))
    return 0


# --- CLI --------------------------------------------------------------------------------

def _cmd_merge(args):
    names = [strand_name(n) for n in (args.batch or args.strand or "").split(",") if n]
    if not names:
        print("merge: name the strand (--strand <S>) or the batch (--batch <S1,S2>)",
              file=sys.stderr)
        return 2
    if args.dequeue:
        db = store.open(args.db) if args.db else store.open()
        try:
            for name in names:
                print("DEQUEUED %s %d" % (name, dequeue(db, args.repo, name)))
        finally:
            store.close(db)
        return 0
    no_pre = args.reason if args.no_pre else None
    if args.no_pre and not args.reason:
        print("merge: --no-pre needs --reason", file=sys.stderr)
        return 2
    try:
        return merge(names, repo=args.repo, wave=args.wave, host=args.host, no_pre=no_pre,
                     close=not args.no_close, db=store.open(args.db) if args.db else None,
                     wait=args.wait)
    except Refused as exc:
        print("merge: refused (%s): %s" % (exc.reason, exc.text), file=sys.stderr)
        return REFUSE_EXIT
    except schedule.ScheduleError as exc:   # a broken MECLAW_*_WAIT_SECS
        print("merge: %s" % exc, file=sys.stderr)
        return exc.code


def _cmd_close(args):
    try:
        root = main_root(os.getcwd())
        if args.report:
            name = os.path.splitext(os.path.basename(args.report))[0]
            wdir = os.path.dirname(os.path.dirname(os.path.abspath(args.report)))
            s = Strand.__new__(Strand)
            s.root, s.name, s.wdir = root, name, wdir
            s.report = os.path.abspath(args.report)
            # Receipts from the main tree, also for a report in a strand tree
            # (review Z-A R3 M2), as `Strand` reads them.
            s.archive = os.path.join(root, "plans", os.path.basename(wdir), "receipts", name)
            try:
                s.doc = versions.read_strand(versions.strand_file(wdir, name))
            except versions.VersionError as exc:
                raise Refused("strand_file_invalid", str(exc))
        else:
            s = Strand(root, args.strand, args.wave)
        return close_issues(s, do=args.do)
    except Refused as exc:
        print("strand: %s" % exc.text, file=sys.stderr)
        return 1


def register(sub):
    p = sub.add_parser("merge", help="merge a strand (or a batch) into master over the tree lease")
    p.add_argument("--strand")
    p.add_argument("--batch", help="S1,S2,... in one candidate; a red batch bisects")
    p.add_argument("--wave")
    p.add_argument("--repo", default="meclaw")
    p.add_argument("--host", help="the lane the merge gate runs on")
    p.add_argument("--no-pre", action="store_true")
    p.add_argument("--reason")
    p.add_argument("--no-close", action="store_true")
    p.add_argument("--dequeue", action="store_true",
                   help="drop the strand's queued rows; an offer to it passes on")
    p.add_argument("--wait", action="store_true",
                   help="queued: block until the tree lease is released (no exit 3); "
                        "MECLAW_ADMISSION_WAIT_SECS ends it")
    p.add_argument("--db")
    p.set_defaults(func=_cmd_merge)
    c = sub.add_parser("close", help="the issue's English close comment (GH #1107)")
    c.add_argument("--strand")
    c.add_argument("--wave")
    c.add_argument("--report")
    c.add_argument("--do", action="store_true")
    c.set_defaults(func=_cmd_close)
