#!/usr/bin/env bash
# meclaw -- the strand kit. One command for the four steps every strand of a
# wave repeats by hand.
#
#     scripts/strand.sh new <name> --issue <nr> [options]
#     scripts/strand.sh gate [<mode>] [gate.sh options]
#     scripts/strand.sh report [--strand <name>]
#     scripts/strand.sh close [--strand <name>] [--do]
#
# WHY THIS EXISTS
# ===============
# A wave is five to eight strands, and every one of them was set up, gated,
# written up and closed by hand. Three measurements say what that cost:
#
#   * The dispatch prompts repeated the machine rules instead of pointing at
#     them. Over five waves 15.6 % of all prompt words were sentences that
#     stood in three or more other prompts; the one wave with a preamble
#     repeated 2.8 % and not a single sentence of it word for word
#     (`plans/welle-p-2026-09-19/befund/04-struktur.md` section 7.2 / 7.4).
#     So `new` prints a prompt that POINTS at the rule file. It never copies a
#     rule, and the self-test fails if it does.
#   * Waiting for a gate was done by re-reading its log. One agent spent
#     2 112 `Read` calls on gate logs and task outputs, each one a full turn
#     over a 400-700k context (`befund/02-token.md` section 3.4). So `gate`
#     BLOCKS until the run is over and prints one summary line plus the red
#     stations -- there is nothing to poll. An agent starts it in the
#     BACKGROUND and waits for the notification: a strand of a wave queues
#     behind the other strands for the cargo lock, 20-30 minutes of it, and a
#     foreground tool call is killed long before that. Two builders polled
#     anyway in the wave that wrote that rule down (448 reads of a task output,
#     1332 idle echoes, 19.09.2026), so on the owner's machine a PreToolUse
#     hook next to the env guard -- `~/.claude/hooks/poll-guard.py` -- now
#     refuses `sleep`, wait loops and `tail`/`cat` on a task output, a gate
#     station log or a `run.log` with exit 2. The hook is part of the harness,
#     not of this repository: a public clone simply does not have it.
#   * The gate stations were copied into the reports by hand: 108 station
#     lines in one wave, and the same summary line in up to three files
#     (`befund/04-struktur.md` section 9.2 / 9.5). So the run is archived next
#     to the wave and `report` fills the header block from the archive.
#
# THE HEADER BLOCK is the point of all of it. Every report starts with a YAML
# block (strang, branch, issues, basis, gate, commits) and
# `scripts/wave_receipt.py` builds the receipt's strand table out of those
# blocks -- so the facts are written once and read by machine, instead of
# being retyped into the receipt (`befund/04-struktur.md` section 9.4).
#
# THE RULE FILE `plans/PREAMBLE.md`, which the dispatch prompt points at,
# lives in the private tree only -- `plans/` is not exported. In a public clone
# the prompt names a file that is not there, and that is the whole difference.
#
# LANGUAGE: this script, its comments and its help are English, like
# everything under `scripts/`. What it PRINTS into the wave -- the dispatch
# prompt and the report skeleton -- is German, because reports, plans and
# commit messages of a wave are German.
#
# THE PLANS TREE IS ALWAYS THE MAIN WORKTREE's. A strand builds in a linked
# worktree, but its report and its gate archive belong to the wave, and the
# wave lives once. Every path this script writes is resolved against the main
# worktree, whichever tree it was started from.
#
# Exit 0 = the step succeeded.

set -uo pipefail

usage() {
    # The header block above IS the help text: everything from the line after
    # the shebang up to the first line that is not a comment.
    awk 'NR > 1 && /^#/ { sub(/^# ?/, ""); print; next } NR > 1 { exit }' \
        "${BASH_SOURCE[0]}"
}

die() { echo "strand: $*" >&2; exit 2; }

REPORT=""; ARCHIVE=""

need_value() {
    [ "$2" -ge 2 ] || die "$1 needs a value"
}

# --- where things live ------------------------------------------------------

# The main worktree, from either side. `--git-common-dir` in absolute form is
# `<main>/.git` in the main tree and in a linked one alike -- that is the whole
# trick, and it is why no caller has to say which tree it is in.
main_root() {
    local common
    common=$(git rev-parse --path-format=absolute --git-common-dir 2>/dev/null) \
        || die "not a git repository"
    dirname -- "$common"
}

# The wave of the branch this tree is on: `<wave>/<name>` -> the one directory
# under `plans/` whose name starts with `<wave>-`. A strand carries its wave in
# its branch, so nothing has to be guessed for `gate`, `report` and `close`.
branch_wave_dir() {
    local plans="$1" wave cand hits=()
    wave=$(git rev-parse --abbrev-ref HEAD 2>/dev/null) || return 1
    case "$wave" in */*) wave=${wave%%/*} ;; *) return 1 ;; esac
    for cand in "$plans/$wave"-*/; do
        [ -d "$cand" ] && hits+=("$(basename -- "${cand%/}")")
    done
    [ ${#hits[@]} -eq 1 ] || return 1
    printf '%s\n' "${hits[0]}"
}

# The wave directory under `plans/`: `--wave` wins, then the branch, and only
# then the first row of `plans/README.md` section Live that names a directory
# which exists.
#
# That last step is a GUESS, and it used to be a silent one: section Live is an
# archive in order, not a pointer at what is running -- its top row is the wave
# built LAST, and the row for the running wave is written by the orchestrator
# when the wave ends, because it describes the result. So a row that says it is
# built is not a fallback, it is a stop: the report and the gate archive of a
# running strand would land in a wave that is over.
wave_dir() {
    local plans="$1" want="${2:-}" cand row rest
    if [ -n "$want" ]; then
        want=${want%/}
        [ -d "$plans/$want" ] || die "no such wave: plans/$want"
        echo "strand: wave $want" >&2
        printf '%s\n' "$want"
        return 0
    fi
    if cand=$(branch_wave_dir "$plans"); then
        echo "strand: wave $cand (from the branch)" >&2
        printf '%s\n' "$cand"
        return 0
    fi
    [ -f "$plans/README.md" ] || die "no plans/README.md -- pass --wave"
    # SC2016: the backticks in the sed pattern are markdown table syntax,
    # not a command substitution -- single quotes are exactly right here.
    # shellcheck disable=SC2016
    while IFS= read -r row; do
        cand=${row%%	*}; rest=${row#*	}
        [ -d "$plans/$cand" ] || continue
        case "$rest" in
            '**Built'*|'**Done'*|'**Gebaut'*)
                die "the newest wave in plans/README.md section Live is over"\
                    "($cand) and the running one is not in the table yet --"\
                    "pass --wave <verzeichnis>" ;;
        esac
        echo "strand: wave $cand (top row of plans/README.md section Live)" >&2
        printf '%s\n' "$cand"
        return 0
    done < <(awk '/^## Live/ { live = 1; next }
                  live && /^## / { exit }
                  live' "$plans/README.md" \
             | sed -n 's#^| *`\([^`]*\)/` *| *\(.*\)#\1\t\2#p')
    die "no wave directory found in plans/README.md section Live -- pass --wave"
}

# `welle-p-2026-09-19` -> `welle-p`. The branch of a strand is `<wave>/<name>`,
# and a branch does not carry the date twice.
wave_name() { printf '%s\n' "${1%-[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]}"; }

# --- new --------------------------------------------------------------------

# The section of the plan that describes this strand: from the first heading
# whose text mentions <key> down to the next heading of the same or a higher
# level. Nothing found means the prompt carries a placeholder instead -- an
# empty order is better than a wrong one.
plan_section() {
    local file="$1" key="$2"
    awk -v key="$key" '
        function level(s) { match(s, /^#+/); return RLENGTH }
        /^#+ / {
            if (inside && level($0) <= want) { exit }
            if (!inside && tolower($0) ~ tolower(key)) {
                inside = 1; want = level($0); print; next
            }
        }
        inside { print }
    ' "$file"
}

cmd_new() {
    local name="" issues=() plan="" section="" base="master" wave_in="" force=0
    [ $# -ge 1 ] || die "new needs a strand name"
    name="$1"; shift
    case "$name" in -*) die "new needs a strand name" ;; esac
    while [ $# -gt 0 ]; do
        case "$1" in
            --issue)   need_value "$1" "$#"; issues+=("${2#\#}"); shift 2 ;;
            --plan)    need_value "$1" "$#"; plan="$2"; shift 2 ;;
            --section) need_value "$1" "$#"; section="$2"; shift 2 ;;
            --base)    need_value "$1" "$#"; base="$2"; shift 2 ;;
            --wave)    need_value "$1" "$#"; wave_in="$2"; shift 2 ;;
            --force)   force=1; shift ;;
            *) die "new: unknown argument: $1" ;;
        esac
    done
    [ ${#issues[@]} -gt 0 ] || die "new needs at least one --issue"

    local root plans wdir wave branch tree report basis issue_list
    root=$(main_root) || exit 2
    plans="$root/plans"
    wdir=$(wave_dir "$plans" "$wave_in") || exit 2
    wave=$(wave_name "$wdir")
    branch="$wave/$name"
    tree="$(dirname -- "$root")/$(basename -- "$root")-wt-$name"
    report="$plans/$wdir/berichte/$name.md"

    if [ "$force" = 0 ]; then
        [ -e "$report" ] && die "$name: the report already exists ($report)"
        [ -e "$tree" ] && die "$name: the worktree already exists ($tree)"
        git -C "$root" rev-parse --verify --quiet "$branch" >/dev/null \
            && die "$name: the branch already exists ($branch)"
    fi

    basis=$(git -C "$root" rev-parse --short=8 "$base" 2>/dev/null) \
        || die "no such base: $base"

    git -C "$root" worktree add -b "$branch" "$tree" "$base" >/dev/null 2>&1 \
        || die "could not create the worktree $tree on $branch"

    issue_list=$(printf '#%s, ' "${issues[@]}"); issue_list="[${issue_list%, }]"

    mkdir -p "$(dirname -- "$report")"
    {
        # The header block. `gate` and `commits` stay empty until there is a
        # gate run and a commit to name; `report` fills them from the archive.
        printf -- '---\n'
        printf 'strang: %s\n' "$name"
        printf 'branch: %s\n' "$branch"
        printf 'issues: %s\n' "$issue_list"
        printf 'basis: %s\n' "$basis"
        printf 'gate: ""\n'
        printf 'commits: []\n'
        printf -- '---\n\n'
        printf '# Strang %s\n\n' "$name"
        printf '## Auftrag\n\n(aus dem Plan)\n\n'
        printf '## Rote Zeile\n\n(der Test, der zuerst rot war)\n\n'
        printf '## Änderungen\n\n(Datei:Zeile, WARUM)\n\n'
        printf '## Gate\n\n(die Summenzeile steht im Kopfblock)\n\n'
        printf '## Offene Punkte\n\n'
        printf '## Strang-Rulings\n\n'
    } >"$report"

    # The dispatch prompt. It carries the order and the paths, and for the
    # rules it carries a POINTER -- see WHY THIS EXISTS above.
    local order="(Auftrag hier einsetzen)"
    if [ -n "$plan" ]; then
        local found
        found=$(plan_section "$plan" "${section:-$name}")
        [ -n "$found" ] && order="$found"
    fi
    cat <<PROMPT
Du bist ein Opus-Bauer des Strangs "$name" der Welle $wave.

Regeln: plans/PREAMBLE.md (gilt wörtlich) und plans/$wdir/BUILD-PREAMBLE.md
(das Wellen-Spezifische). Beide zuerst lesen; sie werden hier nicht wiederholt.

Worktree: $tree
Branch:   $branch (Basis $basis)
Issues:   $issue_list
Bericht:  plans/$wdir/berichte/$name.md (Kopfblock steht schon drin)

Auftrag:
$order

Gate und Bericht laufen über das Kit:
  scripts/strand.sh gate            -- fährt das Gate einmal, blockt, druckt die Summenzeile
  scripts/strand.sh report          -- füllt den Kopfblock aus dem Archiv
PROMPT
}

# --- gate -------------------------------------------------------------------

# The strand this tree belongs to: `<wave>/<name>` -> `<name>`.
strand_of_branch() {
    local branch
    branch=$(git rev-parse --abbrev-ref HEAD 2>/dev/null) || return 1
    case "$branch" in
        */*) printf '%s\n' "${branch##*/}" ;;
        *)   return 1 ;;
    esac
}

cmd_gate() {
    local mode="strand" strand_in="" wave_in="" args=() have_log_dir=0
    if [ $# -ge 1 ]; then
        case "$1" in strand|integration|release|ci) mode="$1"; shift ;; esac
    fi
    while [ $# -gt 0 ]; do
        case "$1" in
            --strand)  need_value "$1" "$#"; strand_in="$2"; shift 2 ;;
            --wave)    need_value "$1" "$#"; wave_in="$2"; shift 2 ;;
            --log-dir) have_log_dir=1; args+=("$1"); shift ;;
            *)         args+=("$1"); shift ;;
        esac
    done

    local root plans wdir name archive_root run_id archive runlog gate rc summary
    root=$(main_root) || exit 2
    plans="$root/plans"
    wdir=$(wave_dir "$plans" "$wave_in") || exit 2
    name="$strand_in"
    [ -n "$name" ] || name=$(strand_of_branch) \
        || die "cannot tell the strand from the branch -- pass --strand"

    # ONE DIRECTORY PER RUN, and a `latest` pointer beside them.
    #
    # The archive was named by strand alone, so a second run wrote its
    # summary, its run log, its receipt and every station log over the first
    # one's -- and a strand gates twice as a matter of course: red, fix,
    # green. That is the material the report and the review are made of, and
    # it happened twice in one day on 2026-09-21 (GH #802). The run id starts
    # with a UTC timestamp, so the directories sort chronologically by name,
    # and it carries the commit that was gated, so a reader knows WHICH run
    # without opening it.
    archive_root="$plans/$wdir/receipts/$name"
    run_id="$(date -u +%Y%m%dT%H%M%SZ)-$(git rev-parse --short=8 HEAD 2>/dev/null || echo nohead)"
    # Two runs in the same second at the same commit are still two runs.
    local suffix=1 base="$run_id"
    while [ -e "$archive_root/$run_id" ]; do
        suffix=$((suffix + 1))
        run_id="$base-$suffix"
    done
    archive="$archive_root/$run_id"
    mkdir -p "$archive" || die "cannot create the archive $archive"
    # `rm` first: `ln -sfn` onto an existing SYMLINK TO A DIRECTORY would
    # otherwise put the new link inside the old target.
    rm -f "$archive_root/latest" 2>/dev/null
    ln -s "$run_id" "$archive_root/latest" 2>/dev/null \
        || printf '%s\n' "$run_id" >"$archive_root/latest.txt"
    runlog="$archive/run.log"

    # The gate of THIS tree, not of the main one and not of the tree the kit
    # was called from: a strand gates the sources it is standing in. The wave
    # that changes `gate.sh` is exactly the wave in which the difference shows.
    gate="$(git rev-parse --show-toplevel 2>/dev/null)/scripts/gate.sh"
    [ -x "$gate" ] \
        || gate="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)/gate.sh"
    [ -x "$gate" ] || die "no gate runner at $gate"

    # The archive is named twice on purpose: `--log-dir` is what the runner
    # understands today, `MECLAW_GATE_ARCHIVE` is the option the gate strand
    # of this wave adds. Both name the TARGET DIRECTORY ITSELF -- the runner
    # copies the receipt and `logs/` straight into it and appends nothing to
    # the path. Whichever lands, receipt and logs end up in the same place,
    # and naming both costs one extra copy of a few kB.
    [ "$have_log_dir" = 1 ] || args+=(--log-dir "$archive")

    # BLOCKING, and the whole run goes to the log instead of to the caller.
    # This is the point of the subcommand: the caller gets the verdict when
    # there is one and has nothing to poll in the meantime (see WHY THIS
    # EXISTS). `MECLAW_GATE_ARCHIVE` travels as an environment variable so an
    # older runner simply ignores it.
    MECLAW_GATE_ARCHIVE="$archive" "$gate" "$mode" "${args[@]}" >"$runlog" 2>&1
    rc=$?

    # Red stations first, then the summary -- the two things a report needs.
    grep -E '^GATE [^ ]+ \[.*\] [0-9]+s RED' "$runlog" || true
    summary=$(grep -E '^GATE-SUMMARY ' "$runlog" | tail -1)
    if [ -z "$summary" ]; then
        echo "strand: the gate wrote no summary line -- last lines of $runlog:" >&2
        tail -20 "$runlog" >&2
        return "$rc"
    fi
    printf '%s\n' "$summary" >"$archive/summary.txt"
    printf '%s\n' "$summary"
    return "$rc"
}

# --- report and close -------------------------------------------------------

# Both steps read the same three things -- the report, the gate archive and
# the git history of the tree the strand built in -- so they share the setup.
# `strand_paths <strand-in> <wave-in>` sets REPORT and ARCHIVE.
strand_paths() {
    local strand_in="$1" wave_in="$2" root plans wdir name
    root=$(main_root) || exit 2
    plans="$root/plans"
    wdir=$(wave_dir "$plans" "$wave_in") || exit 2
    name="$strand_in"
    [ -n "$name" ] || name=$(strand_of_branch) \
        || die "cannot tell the strand from the branch -- pass --strand"
    REPORT="$plans/$wdir/berichte/$name.md"
    ARCHIVE=$(latest_run "$plans/$wdir/receipts/$name")
    [ -f "$REPORT" ] || die "no report at $REPORT"
}

# The newest run of a strand, in three answers, most reliable first: the
# `latest` pointer that `gate` writes; the last run directory by name (the run
# id opens with a UTC timestamp, so lexical order IS chronological); and, for
# an archive written before the per-run layout, the directory itself.
latest_run() {
    local root="$1" newest
    if [ -d "$root/latest" ]; then
        printf '%s\n' "$root/latest"
        return 0
    fi
    if [ -f "$root/latest.txt" ] && [ -d "$root/$(cat "$root/latest.txt")" ]; then
        printf '%s\n' "$root/$(cat "$root/latest.txt")"
        return 0
    fi
    newest=$(find "$root" -mindepth 1 -maxdepth 1 -type d \
                  -name '????????T??????Z-*' 2>/dev/null | sort | tail -1)
    printf '%s\n' "${newest:-$root}"
}

cmd_report() {
    local strand_in="" wave_in=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --strand) need_value "$1" "$#"; strand_in="$2"; shift 2 ;;
            --wave)   need_value "$1" "$#"; wave_in="$2"; shift 2 ;;
            *) die "report: unknown argument: $1" ;;
        esac
    done
    strand_paths "$strand_in" "$wave_in"
    MECLAW_S_REPORT="$REPORT" MECLAW_S_ARCHIVE="$ARCHIVE" python3 - <<'PY'
import os, re, subprocess, sys

report = os.environ["MECLAW_S_REPORT"]
archive = os.environ["MECLAW_S_ARCHIVE"]
FIELDS = ["strang", "branch", "issues", "basis", "gate", "commits"]


def git(*args):
    out = subprocess.run(["git"] + list(args), capture_output=True, text=True)
    return out.stdout.strip() if out.returncode == 0 else ""


text = open(report).read()
m = re.match(r"^---\n(.*?)\n---\n", text, re.S)
if not m:
    sys.exit("strand: %s has no header block" % report)
head, body = {}, text[m.end():]
order = []
for line in m.group(1).splitlines():
    key, _, value = line.partition(":")
    key = key.strip()
    head[key] = value.strip()
    order.append(key)
for f in FIELDS:
    if f not in head:
        head[f] = ""
        order.append(f)

# The branch of the header block is what is read, never the `HEAD` of the tree
# the caller stands in: from the main worktree -- after the merge the normal
# place to run this from -- `HEAD` is master, and the answer would be the whole
# wave under the name of one strand. Only a branch that is gone falls back.
tip = head.get("branch", "").strip()
if not tip or not git("rev-parse", "--verify", "--quiet", "%s^{commit}" % tip):
    tip = "HEAD"

# `basis` is the commit the strand branched from; it never changes, so an
# entry that is already there wins over a fresh merge-base.
if not head["basis"]:
    head["basis"] = git("merge-base", "master", tip)[:8]

# The commits ARE the branch -- retyping them is how a receipt grows a wrong
# SHA. Oldest first, the order a reader walks them in, and FIRST PARENT only:
# a fix round merges master before it starts, and every commit that merge
# carries in belongs to another strand of the wave. Measured on the first fix
# round that did it: 25 commits in the header for a strand that wrote 12.
base = head["basis"].strip('"')
if base:
    shas = git("log", "--abbrev=8", "--format=%h", "--reverse",
               "--first-parent", "%s..%s" % (base, tip)).split()
    if shas:
        head["commits"] = "[%s]" % ", ".join(shas)

# The gate line comes from the archive, not from a human retyping it: the
# same summary line stood in up to three files of one wave.
summary = os.path.join(archive, "summary.txt")
if os.path.isfile(summary):
    lines = [ln.strip() for ln in open(summary) if ln.strip()]
    if lines:
        head["gate"] = '"%s"' % lines[-1]

seen, keys = set(), []
for k in FIELDS + order:
    if k not in seen:
        seen.add(k)
        keys.append(k)
open(report, "w").write(
    "---\n" + "\n".join("%s: %s" % (k, head[k]) for k in keys) + "\n---\n" + body)

missing = [f for f in FIELDS
           if not head[f].strip() or head[f].strip() in ('""', "[]")]
if missing:
    sys.exit("strand: the header block is incomplete: %s" % ", ".join(missing))
if head["gate"].rstrip('"').endswith("RED"):
    sys.exit("strand: the gate is RED -- fix it, run it again, then report")
print("strand: %s -- header block complete, gate green" % report)
PY
}

cmd_close() {
    local strand_in="" wave_in="" do_it=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --strand) need_value "$1" "$#"; strand_in="$2"; shift 2 ;;
            --wave)   need_value "$1" "$#"; wave_in="$2"; shift 2 ;;
            --do)     do_it=1; shift ;;
            *) die "close: unknown argument: $1" ;;
        esac
    done
    strand_paths "$strand_in" "$wave_in"
    MECLAW_S_REPORT="$REPORT" MECLAW_S_DO="$do_it" python3 - <<'PY'
import os, re, subprocess, sys

report = os.environ["MECLAW_S_REPORT"]
do_it = os.environ["MECLAW_S_DO"] == "1"


def git(*args):
    out = subprocess.run(["git"] + list(args), capture_output=True, text=True)
    return out.stdout.strip() if out.returncode == 0 else ""


text = open(report).read()
m = re.match(r"^---\n(.*?)\n---\n", text, re.S)
if not m:
    sys.exit("strand: %s has no header block" % report)
head = {}
for line in m.group(1).splitlines():
    key, _, value = line.partition(":")
    head[key.strip()] = value.strip()

for f in ("branch", "issues", "basis", "gate", "commits"):
    if not head.get(f, "").strip() or head[f].strip() in ('""', "[]"):
        sys.exit("strand: run `strand.sh report` first -- %s is empty" % f)

gate = head["gate"].strip('"')
if gate.endswith("RED"):
    sys.exit("strand: the gate is RED -- nothing to close")

branch = head["branch"]
base = head["basis"]
commits = [c.strip() for c in head["commits"].strip("[]").split(",") if c.strip()]
issues = [i.strip().lstrip("#") for i in head["issues"].strip("[]").split(",")
          if i.strip()]
merged = ""
for line in git("log", "--merges", "--format=%h\t%s", "master").splitlines():
    sha, _, subject = line.partition("\t")
    if branch in subject:
        merged = sha
        break

# The strand is its BRANCH, not the `HEAD` of whatever tree this runs in. The
# orchestrator closes from the main tree after the merge, where `HEAD` is
# master: `base..HEAD` would list every strand of the wave under this one
# issue. A branch already deleted after the merge still has its tip -- the
# second parent of the merge commit.
tip = branch if git("rev-parse", "--verify", "--quiet",
                    "%s^{commit}" % branch) else ""
if not tip and merged:
    tip = "%s^2" % merged
if not tip:
    sys.exit("strand: neither the branch %s nor a merge of it is in this "
             "repository -- close where one of them is" % branch)

changed = [p for p in git("diff", "--name-only", "%s..%s" % (base, tip)).splitlines()
           if p]

# EVERYTHING in this comment is machine-read: the branch, the base, the gate
# line, the SHAs and the file list out of git. The German prose of the report
# is deliberately NOT read -- a closing comment is public and English, and a
# sentence written for the wave carries the owner, a private host or a port
# sooner or later. What cannot be copied cannot leak.
lines = ["Built on `%s` from %s." % (branch, base), ""]
lines.append("Gate: `%s`" % gate)
lines.append("Commits: %s" % ", ".join(commits))
if changed:
    lines.append("Changed: %s" % ", ".join("`%s`" % c for c in changed))
if merged:
    lines.append("Merged as %s." % merged)
comment = "\n".join(lines)

for issue in issues:
    argv = ["gh", "issue", "close", issue, "--comment", comment]
    if do_it:
        rc = subprocess.run(argv).returncode
        if rc != 0:
            sys.exit("strand: gh failed for #%s" % issue)
    else:
        print("gh issue close %s --comment '%s'"
              % (issue, comment.replace("'", "'\\''")))
PY
}

# --- main -------------------------------------------------------------------

if [ $# -lt 1 ] || [ "$1" = "-h" ] || [ "$1" = "--help" ]; then
    usage
    [ $# -lt 1 ] && exit 2
    exit 0
fi

sub="$1"; shift
case "$sub" in
    new)    cmd_new "$@" ;;
    gate)   cmd_gate "$@" ;;
    report) cmd_report "$@" ;;
    close)  cmd_close "$@" ;;
    *) die "unknown subcommand: $sub (expected new|gate|report|close)" ;;
esac
