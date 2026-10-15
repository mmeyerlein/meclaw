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
       merge, which takes it on its next call.
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
       in the generated corpus, which is never merged but regenerated.
    4. versions + pin cascade (`versions.assign`, kinds from
       `plans/<wave>/straenge/<S>.json`), the corpus when docs moved, the
       `[Unreleased]` line, `ROADMAP.md` § Now without the strand's issues;
       commit `Merge <S> (#n): <title>` with the version and pin lines.
    5. merge gate on the committed candidate. GREEN -> `git merge --ff-only`
       in the main tree (moves master AND its working tree; never
       `update-ref`, which would leave the checked-out tree behind as a
       backwards diff), `merges` -> `merged`, `merge_done`, lease free, issue
       close (`close`). RED/ABORTED -> candidate discarded, `merge_refused`,
       lease free, the strand gets the finding.

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
import json
import os
import re
import shlex
import shutil
import signal
import subprocess
import sys
import uuid

from . import events, lifted, store, turnaround, versions

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
    res = subprocess.run(["git", "-C", cwd] + list(args), capture_output=True, text=True,
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


def tree_text(root, branch, rel):
    """`rel` as the branch tip carries it, else as the main tree has it, else None.

    Strands commit their report and strand file on the branch; the main tree
    holds them only when `plans/` is not tracked there."""
    if branch:
        res = git(root, "show", "refs/heads/%s:%s" % (branch, rel))
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
    the branch tip (the main tree as fallback), receipts from the main tree."""

    def __init__(self, root, name, wave=None):
        self.root, self.name = root, name
        self.wdir = wave_dir(root, name, wave)
        self.report = os.path.join(self.wdir, "berichte", "%s.md" % name)
        self.archive = os.path.join(self.wdir, "receipts", name)
        rel = os.path.relpath(self.wdir, root)
        report_rel = os.path.join(rel, "berichte", "%s.md" % name)
        default = default_branch(self.wdir, name)
        self.head, self.body = parse_head(tree_text(root, default, report_rel))
        self.branch = (self.head or {}).get("branch") or default
        if self.branch != default:
            self.head, self.body = parse_head(tree_text(root, self.branch, report_rel))
        path = versions.strand_file(self.wdir, name)
        try:
            self.doc = versions.strand_doc(
                tree_text(root, self.branch, os.path.relpath(path, root)), path)
        except versions.VersionError as exc:
            raise Refused("strand_file_invalid", str(exc))
        head = self.head or {}
        self.issues = versions.issues_of(self.doc) or list_field(head.get("issues"))
        self.title = self.doc.get("title") or name
        self.migrations = versions.migrations_of(self.doc)


# --- receipts --------------------------------------------------------------------------

def strand_receipts(archive):
    """Run directories of `archive` that hold a STRAND receipt, newest first."""
    if not os.path.isdir(archive):
        return []
    found = []
    for d in sorted(os.listdir(archive), reverse=True):
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


# The review sections: `## Review` (the scaffold of `strand.sh new`) or the
# form the reports write, `## Fix-Runde Review <sha>` (Z-I 2c (2)).
REVIEW_HEADING = re.compile(r"^## (?:Fix-Runde )?Review\b", re.M)
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
    git error or a malformed declaration is a refusal, never a pass."""
    mb = out(s.root, "merge-base", "master", tip)
    if not mb:
        return "cannot find the merge base of master and %s" % tip[:9]
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
    return True


def _event(db, name, mid, strand, base, **extra):
    data = {"merge": mid, "strand": strand, "base": base}
    data.update({k: v for k, v in extra.items() if v is not None})
    events.emit(db, name, "status", data)


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
    """`git merge --no-ff --no-commit` of `refs`; the corpus is regenerated, never merged."""
    res = git(cand, "merge", "--no-ff", "--no-commit", *refs)
    if res.returncode == 0:
        return
    conflicts = out(cand, "diff", "--name-only", "--diff-filter=U").split()
    if conflicts and len(refs) == 1 and all(c in CORPUS for c in conflicts):
        for c in conflicts:
            git(cand, "checkout", "--ours", "--", c, check=True)
            git(cand, "add", "--", c, check=True)
        return
    git(cand, "merge", "--abort")
    raise Refused("batch_conflict" if len(refs) > 1 else "merge_conflict",
                  "conflict in %s" % (", ".join(conflicts) or res.stderr.strip()[:200]))


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
             versions.version_line(result), versions.pins_line(result)]
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
        merge_into(cand, tips)
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
    custom = os.environ.get("MECLAW_MERGE_GATE_CMD")
    if custom:
        return [w.format(tree=tree, base=ctx.get("gate_base") or ctx["base"],
                         strand=ctx["label"], holder=ctx.get("holder") or "")
                for w in shlex.split(custom)]
    own = ["--strand", ctx["holder"]] if ctx.get("holder") else []
    # GH #1167 (b): the gate plans from `gate_base` -- the strand tip when the
    # strand receipt covers it, else master (the full strand diff).
    base = ctx.get("gate_base") or ctx["base"]
    if ctx.get("host"):
        _lane_target(ctx["host"])  # an unknown lane refuses here (`no_lane`)
        return [os.path.join(tree, "scripts", "gate.sh"), "strand", "--host", ctx["host"],
                "--base", base] + own
    return [os.path.join(tree, "scripts", "gate.sh"), "strand", "--base", base] + own


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
    return {"verdict": verdict,
            "line": line or "gate exit %d without a summary (not judged)" % proc.returncode,
            "secs": secs, "log": log}


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
    any doubt: master -- the full gate."""
    if len(strands) != 1:
        return base, "full: a batch of %d strands" % len(strands)
    s = strands[0]
    rec = ctx["receipts"][s.name]
    tip = out(root, "rev-parse", "%s^{commit}" % s.branch)
    if not tip or rec.get("no_pre"):
        return base, "full: %s" % ("no branch tip" if not tip else "merged with --no-pre")
    return turnaround.merge_gate_base(root, base, tip, rec)


def land_plan_only(db, ctx, s, base):
    """GH #1167 (d): a plan-only branch -- merge commit, no versions, no gate.
    The candidate's own diff is checked again: gate-neutral or refused."""
    root = ctx["root"]
    mid = ctx["mids"][s.name]
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
        hold_tree(db, ctx["repo"], ctx["holder"])
        fast_forward(root, base, sha)
    except Refused as exc:
        return [(s.name, "refused", exc.reason, exc.text)]
    finally:
        close_candidate(root, cand)
    line = "MERGE-PLAN-ONLY %s %s %d files GREEN" % (s.name, sha[:9], len(paths))
    _set_merge(db, mid, state="merged", merge_sha=sha)
    _event(db, "merge_done", mid, s.name, base, merge_sha=sha, plan_only=True)
    write_receipt(s.wdir, "merge-%s" % s.name, sha, {
        "strand": s.name, "base": base, "merge_sha": sha, "plan_only": True,
        "files": paths, "gate": {"verdict": "SKIPPED", "line": "plan-only: gate-neutral"},
        "summary": line})
    ctx["out"].write("%s\n" % line)
    return [(s.name, "merged", None, sha)]


def land(db, ctx, strands, base, depth=0):
    """Candidate -> gate -> fast-forward for `strands`; bisects a red batch.

    Returns `[(strand, state, reason)]`."""
    root, repo = ctx["root"], ctx["repo"]
    label = "+".join(s.name for s in strands)
    mids = {s.name: ctx["mids"][s.name] for s in strands}
    try:
        cand, sha, result, sats = build_candidate(root, base, strands, label)
    except Refused as exc:
        if exc.reason == "batch_conflict" or len(strands) == 1:
            return [(s.name, "refused", exc.reason, exc.text) for s in strands]
        raise
    except versions.VersionError as exc:
        return [(s.name, "refused", "version_conflict", str(exc)) for s in strands]
    gctx = {"rev": sha, "base": base, "label": label, "strands": [s.name for s in strands],
            "cache_from": [ctx["receipts"][s.name]["run"] for s in strands],
            "strand_run": (ctx["receipts"][strands[0].name]["run"] if len(strands) == 1
                           else None),
            "host": ctx.get("host"), "wave": os.path.basename(strands[0].wdir.rstrip("/")),
            "holder": gate_holder(strands)}
    gctx["gate_base"], gctx["gate_reuse"] = gate_base(root, base, strands, ctx)
    for s in strands:
        _set_merge(db, mids[s.name], state="gating", versions=result["versions"],
                   pins=result["pins"])
        _event(db, "merge_gated", mids[s.name], s.name, base, merge_sha=sha,
               versions=result["versions"])
    try:
        # N3: every gate run starts on a renewed lease -- a bisection runs
        # several gates under one lease, each up to TTL_S long.
        hold_tree(db, repo, ctx["holder"])
        verdict = run_gate(cand, gctx)
        ctx["out"].write("%s\n" % verdict["line"])
        if verdict["verdict"] != "GREEN":
            reason = "gate_aborted" if verdict["verdict"] == "ABORTED" else "gate_red"
            if len(strands) > 1 and depth < MAX_BISECT and verdict["verdict"] == "RED":
                close_candidate(root, cand)
                cand = None
                half = len(strands) // 2
                done = land(db, ctx, strands[:half], base, depth + 1)
                now = out(root, "rev-parse", "master")
                return done + land(db, ctx, strands[half:], now, depth + 1)
            return [(s.name, "refused", reason, verdict["line"]) for s in strands]
        hold_tree(db, repo, ctx["holder"])
        fast_forward(root, base, sha)
    except Refused as exc:
        return [(s.name, "refused", exc.reason, exc.text) for s in strands]
    finally:
        if cand:
            close_candidate(root, cand)
    for s in strands:
        _set_merge(db, mids[s.name], state="merged", merge_sha=sha)
        _event(db, "merge_done", mids[s.name], s.name, base, merge_sha=sha,
               versions=result["versions"], pins=result["pins"])
        for repo_name, template, version in sats:
            events.emit(db, "satellite_pin_needed", "finding", {
                "merge": mids[s.name], "repo": repo_name, "template": template,
                "version": version})
        write_receipt(s.wdir, "merge-%s" % s.name, sha, {
            "strand": s.name, "base": base, "merge_sha": sha, "versions": result["versions"],
            "workspace": result["workspace"], "pins": result["pins"], "gate": verdict,
            "summary": "MERGE %s %s %s GREEN" % (s.name, sha[:9], versions.version_line(result))})
    ctx["out"].write("%s\n" % versions.version_line(result))
    ctx["out"].write("%s\n" % versions.pins_line(result))
    return [(s.name, "merged", None, sha) for s in strands]


def merge(names, repo="meclaw", start=None, wave=None, host=None, no_pre=None, close=True,
          db=None, out_=None, err=None):
    """`pipeline.py merge`: returns the exit code (0 merged, 1 refused, 3 queued)."""
    out_, err = out_ or sys.stdout, err or sys.stderr
    root = main_root(start or os.getcwd())
    own_db = db is None
    db = db or store.open()
    names = [strand_name(n) for n in names]
    holder = "+".join(names)
    strands, mids, created = [], {}, set()
    base = out(root, "rev-parse", "master")
    term = trap_term()
    taken = False
    try:
        strands = [Strand(root, n, wave) for n in names]
        for s in strands:
            row = db.execute("SELECT id FROM merges WHERE repo=? AND strand=? AND state='queued'"
                             " ORDER BY at DESC LIMIT 1", (repo, s.name)).fetchone()
            if row:
                mids[s.name] = row["id"]
            else:
                mids[s.name] = _new_merge(db, repo, s.name, base, "queued", store.now())
                created.add(s.name)
        try:
            taken = take_tree(db, repo, holder)
        except Refused as exc:
            if exc.reason != "already_merging":
                raise
            # N2: before 'gating' the running merge's row is still 'queued' and
            # was picked up above -- it belongs to the run that holds the
            # lease. Only the rows this call created are refused.
            mids = {n: m for n, m in mids.items() if n in created}
            raise
        if not taken:
            for s in strands:
                _event(db, "merge_queued", mids[s.name], s.name, base)
            who = store.lease_who(db, resource(repo)) or {}
            err.write("merge: tree:master is held by %s -- queued, the release offers it "
                      "to the oldest queued merge\n" % (who.get("holder") or who.get("offered_to")))
            return QUEUED_EXIT
        # master as it stands under the lease -- the previous holder may have
        # fast-forwarded it between the first read and the take.
        base = out(root, "rev-parse", "master")
        ctx = {"root": root, "repo": repo, "host": host, "mids": mids, "out": out_,
               "receipts": {}, "holder": holder}
        ready, plans_only, tips = [], [], {}
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
        results = []
        for s in plans_only:
            results += land_plan_only(db, ctx, s, base)
            base = out(root, "rev-parse", "master")
        results += land(db, ctx, ready, base) if ready else []
        done = len(ready) + len(plans_only)
        rc = 0 if done and done == len(strands) else REFUSE_EXIT
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
        return rc
    except Refused as exc:
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
    """The merge commit on master that carries the strand, or ''."""
    for line in out(root, "log", "--merges", "--first-parent", "--format=%H %P%x09%s",
                    "master").splitlines():
        shas, _, subject = line.partition("\t")
        parts = shas.split()
        if (tip and tip in parts[2:]) or branch in subject \
                or re.match(r"Merge (\S+, )*%s[ ,]" % re.escape(name), subject):
            return parts[0]
    return ""


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
    tip = out(root, "rev-parse", "--verify", "--quiet", "%s^{commit}" % branch) if branch else ""
    merged = find_merge(root, branch, tip, name)
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


def wave_issues(root, wdir, name):
    """`{issue: other strand}` of the issues another strand file of the wave
    carries too -- the wave's (acceptance) issue, which no strand closes
    (Z-I 2c (5)). Read from the main tree's `plans/<wave>/straenge/`."""
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
            for issue in versions.issues_of(doc):
                found.setdefault(issue, other)
    return found


def close_issues(s, do=False, out_=None, err=None, gh=None):
    out_, err = out_ or sys.stdout, err or sys.stderr
    issues, comment, _ = close_info(s.root, s.report, s.archive, s.name)
    gh = gh or os.environ.get("MECLAW_GH", "gh")
    shared = wave_issues(s.root, s.wdir, s.name)
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
                     close=not args.no_close, db=store.open(args.db) if args.db else None)
    except Refused as exc:
        print("merge: refused (%s): %s" % (exc.reason, exc.text), file=sys.stderr)
        return REFUSE_EXIT


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
    p.add_argument("--db")
    p.set_defaults(func=_cmd_merge)
    c = sub.add_parser("close", help="the issue's English close comment (GH #1107)")
    c.add_argument("--strand")
    c.add_argument("--wave")
    c.add_argument("--report")
    c.add_argument("--do", action="store_true")
    c.set_defaults(func=_cmd_close)
