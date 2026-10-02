#!/usr/bin/env bash
# meclaw -- the strand kit. One command for the four steps every strand of a
# wave repeats by hand.
#
#     scripts/strand.sh new <name> --issue <nr> [options]
#     scripts/strand.sh gate [<mode>] [--host <lane>] [gate.sh options]
#     scripts/strand.sh test '<nextest filterset>' [--host <lane>] [nextest args]
#     scripts/strand.sh report [--strand <name>]
#     scripts/strand.sh close [--strand <name>] [--do]
#     scripts/strand.sh token take|release|who|check|init|off|lane [options]
#     scripts/strand.sh lanes status
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
# THE VERDICT has three words: GREEN, RED and ASK. ASK (exit 4 of the
# runner, passed through unchanged) is a question for the owner that the
# passes raise instead of a silent red; `gate` prints the asking stations,
# the summary and the runner's question block. `report` and `close` take
# GREEN only -- answer the question or measure, then report.
#
# THE CARGO TOKEN (`token`) limits how many strands of a wave are in their
# cargo phase at once. The cargo lock serialises builds, but it does not say
# how many builders wait behind it: in one wave eleven builders queued for
# 20-90 minutes per single test and woke 103 times to a cold prompt cache in
# three hours (the wave's lesson on pipeline pace, sections 1-2). So the
# orchestrator arms N tokens (`token init --max N`, default 3), a strand
# takes one after its rebase (`token take`; exit 3 = queued, first come
# first served -- end the turn and wait to be woken), and `report` gives it
# back at the end of the cargo phase. Once armed, `scripts/test-tier.sh`
# and `strand.sh gate` refuse without a token (exit 3). Never checked: a
# host without a token file (a public clone, CI, every test, any work
# outside a wave), a tree whose branch has no `/` (the main tree, where the
# orchestrator runs the pass), a tier run inside a gate, and
# `MECLAW_STRAND_TOKEN_SKIP=<reason>` -- which is logged, not silent.
#
#     token init [--max N] [--ttl MIN] [--force]   arm the host (refuses over a queue)
#     token init --lanes a,b,c [--ttl MIN]         arm one named token per build lane
#     token take [--strand S]                      hold a token or join the queue
#     token release [--strand S]                   give it back, name the next
#     token who                                    holders, queue, last events
#     token check [--pid P]                        what the gate and the tier call
#     token off [--force]                          disarm (refuses over a queue; the log stays)
#     token lane [--strand S]                      the lane this strand holds, or nothing
#
# The file lives next to the cargo lock: `${MECLAW_GATE_LOCK%.lock}.tokens`
# (default /tmp/meclaw-w26-cargo.tokens), or `MECLAW_STRAND_TOKENS`, with
# `.lock` (flock) and `.log` (append-only TSV) beside it. A holder is STALE
# when its worktree is gone, or when it was last seen longer ago than the
# TTL and no live process holds it -- a running gate keeps its token as long
# as it runs. A waiter in its own worktree keeps its place as long as the
# tree stands (`who` marks it once it waits beyond the TTL); one queued from
# outside leaves after the TTL. `init` and `off` refuse while anybody holds
# a token or waits for one, unless `--force`. A strand is
# `<wave>/<name>`, its branch: `--strand <name>` takes the wave from the
# branch or `--wave`, `--strand <wave>/<name>` is taken as it stands.
#
# THE LANES. A cargo token on the owner's machine is a place in ONE queue for
# ONE cargo lock, under the caps that keep a live colony's watchdog quiet.
# Build hosts run no colony: the test station of one gate took 459 s there at
# full width against 788 s capped, and three hosts running in parallel
# differed by 2 % (measured 2026-10-01). So a lane is a NAMED token, one per
# build host: `token init --lanes a,b,c` arms one token per name, `take`
# hands out the first free name in that order (the spare host stands last)
# and prints `lane <name>` on stdout, `who` shows who holds which. `--lanes`
# and `--max` exclude each other (exit 2); queue, TTL and exit 3 are the
# cargo token's, unchanged.
#
#     gate [<mode>] --host <lane>   the gate on that build host
#
# Without `--host`, a strand that holds a lane gates on it; without either,
# the gate is local as it always was. The token check wants exactly the lane
# that is held (exit 3 otherwise). The commit goes by `git push` into a bare
# repository on the host, the uncommitted rest -- staged or not, measured
# against HEAD -- by an rsync overlay, so the host gates exactly the tree a
# local gate would. A file whose name says secret (`.env`, `.env.*` but
# `*.example`, `vault.env`, `vault-pass*`, credentials; `scripts/lane_sync.sh`
# holds the list) never leaves: untracked it is left out, TRACKED the gate is
# refused with its path (exit 2, fail closed). Both sides hash the overlay
# and a difference is refused too. The host's runner runs with `--lane --base
# <merge base>`, the base computed HERE, where `master` is. Receipt and
# station logs come back into the wave's archive -- or the run is not green;
# the summary, the red stations and the exit code (1/2/4) are the runner's,
# after one first line `strand: lane <name> (<host>) rev <sha> overlay
# <hash>`. Without a held token, `--host` wants MECLAW_STRAND_TOKEN_SKIP.
#
# A LANE RUNS EVERY STATION (GH #942). While the lanes are armed this machine
# builds nothing -- it runs the owner's live and lab colonies, and a lab
# colony died at load 15 while strands built beside it (2026-10-02) -- so
# `scripts/test-tier.sh` and a local `gate.sh` with a cargo station refuse
# with exit 3 (`MECLAW_LANE_LOCAL_OK=<reason>` is the one way out). The host
# gets what the stations there need: a secret-free `.env` written in its tree
# (`gate_plan.py --print lane-env`), the `github-main` and `master` refs, and
# `node` is the host's own. Nothing is left to run here afterwards
# (`gate_plan.py --print lane-local` is empty); the code that ran such a
# station here, with a second summary line and the lane's receipt handed
# over, stays for a station that is ever listed there again.
#
#     test '<filterset>' [--host <lane>]   one nextest filterset on the lane
#
# The same transport as a gate (push, overlay, the same refusals), then
# `scripts/test-tier.sh filter '<filterset>'` in the host's tree. The whole
# log comes back to `receipts/<strand>/test-<run>/test.log` -- `latest`, the
# gate's pointer, is not moved -- and the last line is
# `TEST [<filterset>] <passed>/<total> <secs>s GREEN|RED`; the exit is the
# remote one (3 = no lane held, 2 = a wrong call or an unreachable host).
# It blocks like a gate: start it in the background and wait.
#
# A host that does not answer is exit 2 with `strand: lane <name> unreachable`,
# no `latest` and the token still held: whether to take another lane or gate
# locally is the orchestrator's call, never a silent fallback.
#
# The lanes are named in a PRIVATE host file, `${MECLAW_LANES_FILE:-
# ~/.config/meclaw/lanes}`: `<name> <ssh-target> [<bmc>]` per line, `#` a
# comment. It is never in the repository -- it names machines -- and it holds
# no password. Build hosts are normally switched off; switching one on or off
# is a step of the orchestrator's, outside this kit, and `lanes status` is
# the one lane verb that reads: per lane reachable or not, the file system
# type and free space of its `/srv/target`, strand refs in its bare
# repository. On a tmpfs the first gate after a boot builds cold and pushes
# the history; on a disk the target stays warm (GH #942).
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
#
# A prefix can come back: on 2026-09-26 `plans/` held `welle-fix-2026-09-12`
# and `welle-fix-2026-09-27`, the branch `welle-fix/V` matched both, and every
# `gate`/`report`/`close` of the running wave without `--wave` ended in exit 2
# on the live table (GH #861, the review of strand V). Of several hits, the
# one directory that carries this strand's report `berichte/<name>.md` wins --
# `new` wrote it there. Two or none with that report stay ambiguous.
branch_wave_dir() {
    local plans="$1" branch wave cand hits=() owners=()
    branch=$(git rev-parse --abbrev-ref HEAD 2>/dev/null) || return 1
    case "$branch" in */*) wave=${branch%%/*} ;; *) return 1 ;; esac
    for cand in "$plans/$wave"-*/; do
        [ -d "$cand" ] || continue
        hits+=("$(basename -- "${cand%/}")")
        [ -f "${cand}berichte/${branch##*/}.md" ] && owners+=("${hits[-1]}")
    done
    if [ ${#hits[@]} -eq 1 ]; then
        printf '%s\n' "${hits[0]}"
    elif [ ${#owners[@]} -eq 1 ]; then
        printf '%s\n' "${owners[0]}"
    else
        return 1
    fi
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
#
# A line inside a fenced code block is never a heading: plan parts carry
# shell blocks, and their `# comment` lines used to cut the order short or
# start it in the wrong place. The key is compared as plain text, not as an
# awk regex -- `Strang (R)` never matched itself, `[` broke the program. It
# travels through ENVIRON, because `awk -v` rewrites backslashes (GH #891,
# F17 of the wave Gate receipt).
plan_section() {
    local file="$1" key="$2"
    PLAN_SECTION_KEY="$key" awk '
        function level(s) { match(s, /^#+/); return RLENGTH }
        BEGIN { key = tolower(ENVIRON["PLAN_SECTION_KEY"]) }
        /^[ \t]*```/ { fence = !fence; if (inside) print; next }
        !fence && /^#+ / {
            if (inside && level($0) <= want) { exit }
            if (!inside && index(tolower($0), key)) {
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

# ONE DIRECTORY PER RUN, and a `latest` pointer beside them.
#
# The archive was named by strand alone, so a second run wrote its
# summary, its run log, its receipt and every station log over the first
# one's -- and a strand gates twice as a matter of course: red, fix,
# green. That is the material the report and the review are made of, and
# it happened twice in one day on 2026-09-21 (GH #802). The run id starts
# with a UTC timestamp, so the directories sort chronologically by name,
# and it carries the commit that was gated, so a reader knows WHICH run
# without opening it. `open_run_dir <archive-root>` sets RUN_DIR, and
# PREV_LATEST to what `latest` named before (for `restore_latest`).
RUN_DIR=""; PREV_LATEST=""
open_run_dir() {
    local archive_root="$1" run_id suffix=1 base
    run_id="$(date -u +%Y%m%dT%H%M%SZ)-$(git rev-parse --short=8 HEAD 2>/dev/null || echo nohead)"
    # Two runs in the same second at the same commit are still two runs.
    base="$run_id"
    while [ -e "$archive_root/$run_id" ]; do
        suffix=$((suffix + 1))
        run_id="$base-$suffix"
    done
    RUN_DIR="$archive_root/$run_id"
    mkdir -p "$RUN_DIR" || die "cannot create the archive $RUN_DIR"
    PREV_LATEST=$(readlink "$archive_root/latest" 2>/dev/null || true)
    # `rm` first: `ln -sfn` onto an existing SYMLINK TO A DIRECTORY would
    # otherwise put the new link inside the old target.
    rm -f "$archive_root/latest" 2>/dev/null
    ln -s "$run_id" "$archive_root/latest" 2>/dev/null \
        || printf '%s\n' "$run_id" >"$archive_root/latest.txt"
}

# A run that never reached a verdict (a lane lost mid-run) must not be what
# `latest` -- and so `report` -- points at.
restore_latest() {
    local archive_root="$1"
    rm -f "$archive_root/latest" 2>/dev/null
    [ -n "$PREV_LATEST" ] && ln -s "$PREV_LATEST" "$archive_root/latest" 2>/dev/null
    return 0
}

# Red and asking stations of a run log -- the lines a report needs.
red_lines() { grep -E '^GATE [^ ]+ \[.*\] [0-9]+s (RED|ASK)' "$1" || true; }

# --- the gate on a build host (THE LANES) -------------------------------------

# The ssh target of a lane in the host file, or nothing.
lane_target() {
    local file
    file=$(lanes_file)
    [ -f "$file" ] || return 0
    awk -v n="$1" '{ sub(/#.*/, "") } $1 == n && NF >= 2 { print $2; exit }' "$file"
}

# `gate_on_host <mode> <lane> <strand> <archive-root> <runner> [gate options]`
gate_on_host() {
    local mode="$1" lane="$2" name="$3" archive_root="$4" gate="$5"; shift 5
    local target sync base sha digest out errf prc runlog rc lrc=0 envs="" summary lsum="" pass=()
    target=$(lane_target "$lane")
    [ -n "$target" ] || die "no lane $lane in the host file $(lanes_file)"
    sync="$(dirname -- "$gate")/lane_sync.sh"
    [ -x "$sync" ] || die "no lane transport at $sync"
    [ "$mode" != ci ] || die "a ci run belongs to the workflow, not to a lane"

    # What travels to the runner there: everything but the base (computed
    # here) and an archive path (one of this machine).
    while [ $# -gt 0 ]; do
        case "$1" in
            --base|--log-dir|--archive) shift 2 ;;
            *) pass+=("$1"); shift ;;
        esac
    done

    # The base, as the runner of THIS tree computes it -- one rule, not two
    # -- resolved to a full sha here, where `master` is: the host's copy of
    # the repository has no reliable one.
    base=$("$gate" "$mode" --plan-only "$@" 2>/dev/null \
        | sed -n 's/^gate: base = \([^ ]*\) .*/\1/p' | head -1)
    base=$(git rev-parse --verify --quiet "${base:-none}^{commit}" 2>/dev/null) \
        || die "cannot tell the base of this diff -- see scripts/gate.sh $mode --plan-only"

    # Exit 5 of the transport is a refusal HERE -- a tracked secret, or a
    # host tree whose overlay hash is not this tree's (OR-S3-64): fail
    # closed, the paths or hashes on stderr, no archive, the token held.
    errf=$(mktemp) || die "no temp file"
    out=$("$sync" push "$target" "$name" 2>"$errf")
    prc=$?
    if [ "$prc" = 5 ]; then
        echo "strand: lane $lane refused -- the host would not gate this tree:" >&2
        sed 's/^/  /' "$errf" >&2
        rm -f "$errf"
        return 2
    elif [ "$prc" != 0 ]; then
        echo "strand: lane $lane unreachable" >&2
        sed 's/^/  /' "$errf" >&2
        rm -f "$errf"
        return 2
    fi
    rm -f "$errf"
    read -r sha digest <<<"$out"

    open_run_dir "$archive_root"
    runlog="$RUN_DIR/run.log"
    # The overlay hash names the tree that was gated -- the same on both
    # sides, or the push above had refused.
    printf 'strand: lane %s (%s) rev %s overlay %s\n' "$lane" "${target#*@}" "${sha:0:8}" "${digest:-?}"

    "$sync" run "$target" "$name" "$mode" "$base" ${pass[@]+"${pass[@]}"} >"$runlog" 2>&1
    rc=$?
    if [ "$rc" = 255 ] && ! grep -q '^GATE-SUMMARY ' "$runlog"; then
        restore_latest "$archive_root"
        echo "strand: lane $lane unreachable (lost during the run, log: $runlog)" >&2
        return 2
    fi
    # Receipt and station logs HERE are part of the verdict (review M5): a
    # run whose archive did not come back is not green, whatever it said.
    if ! "$sync" fetch "$target" "$name" "$RUN_DIR" >/dev/null; then
        echo "strand: the receipt of the run did not come back from lane $lane -- not green" \
            "(remote: $(grep -E '^GATE-SUMMARY ' "$runlog" | tail -1))" >&2
        restore_latest "$archive_root"
        return 2
    fi
    cmd_token check --pid 0 >/dev/null 2>&1 || true

    # The stations a host could not run (`SKIP <reason> (lane)`, `gate_plan.py
    # --print lane-local`) would run here, with the same base, and add their
    # own summary line (GH #934, OR-S3-96), with the lane's receipt handed
    # over. Since GH #942 that list is empty -- a lane gets a secret-free `.env`,
    # the export ref and has `node` -- so nothing runs here after a lane; the
    # mechanism stays for a station that is ever listed again.
    envs=$(sed -n 's/^GATE \([^ ]*\) \[.*\] [0-9]*s SKIP no-[a-z]* (lane)$/\1/p' "$runlog" \
        | paste -sd, -)
    if [ -n "$envs" ]; then
        local lane_receipt=""
        [ -f "$RUN_DIR/last-$mode.json" ] && lane_receipt="$RUN_DIR/last-$mode.json"
        MECLAW_GATE_LANE_RECEIPT="$lane_receipt" \
        MECLAW_GATE_ARCHIVE="$RUN_DIR/local" "$gate" "$mode" --only "$envs" \
            --base "$base" --log-dir "$RUN_DIR/local" >"$RUN_DIR/run-local.log" 2>&1
        lrc=$?
    fi

    red_lines "$runlog"
    [ -n "$envs" ] && red_lines "$RUN_DIR/run-local.log"
    summary=$(grep -E '^GATE-SUMMARY ' "$runlog" | tail -1)
    if [ -z "$summary" ]; then
        echo "strand: the gate on lane $lane wrote no summary line -- last lines of $runlog:" >&2
        tail -20 "$runlog" >&2
        return "$rc"
    fi
    printf '%s\n' "$summary"
    if [ -n "$envs" ]; then
        lsum=$(grep -E '^GATE-SUMMARY ' "$RUN_DIR/run-local.log" | tail -1)
        if [ -z "$lsum" ]; then
            echo "strand: the local run of $envs wrote no summary line -- see $RUN_DIR/run-local.log" >&2
            lsum="GATE-SUMMARY $mode local 0/0 0s RED"
            lrc=1
        fi
        printf '%s\n' "$lsum"
    fi
    sed -n '/^gate: ASK/,$p' "$runlog"

    # ONE verdict of the kit: RED when either half is red, ASK when either
    # asks and nothing is red. `summary.txt` -- what `report` reads -- holds
    # the line that decides it.
    local out="$rc"
    if [ "$rc" = 1 ] || [ "$lrc" = 1 ]; then out=1
    elif [ "$rc" = 0 ] && [ "$lrc" != 0 ]; then out="$lrc"
    fi
    local decides="$summary"
    if [ "$rc" != 1 ] && [ "$lrc" = 1 ]; then decides="$lsum"
    elif [ "$rc" = 0 ] && [ "$lrc" != 0 ]; then decides="$lsum"
    fi
    printf '%s\n' "$decides" >"$RUN_DIR/summary.txt"
    return "$out"
}

cmd_gate() {
    local mode="strand" strand_in="" wave_in="" args=() have_log_dir=0 plan_only=0 host=""
    if [ $# -ge 1 ]; then
        case "$1" in strand|integration|release|ci) mode="$1"; shift ;; esac
    fi
    while [ $# -gt 0 ]; do
        case "$1" in
            # A question, not a run. It used to reach the runner AFTER the
            # kit had made a run directory and pointed `latest` at it -- a
            # folder without a summary, and a `latest` through which `report`
            # no longer found the green gate (GH #861).
            -h|--help)
                usage
                echo "gate options pass through to scripts/gate.sh -- see scripts/gate.sh --help"
                return 0 ;;
            --strand)    need_value "$1" "$#"; strand_in="$2"; shift 2 ;;
            --wave)      need_value "$1" "$#"; wave_in="$2"; shift 2 ;;
            --host)      need_value "$1" "$#"; host="$2"; shift 2 ;;
            --log-dir)   have_log_dir=1; args+=("$1"); shift ;;
            --plan-only) plan_only=1; args+=("$1"); shift ;;
            *)           args+=("$1"); shift ;;
        esac
    done

    local root plans wdir name archive_root archive runlog gate rc summary

    # The gate of THIS tree, not of the main one and not of the tree the kit
    # was called from: a strand gates the sources it is standing in. The wave
    # that changes `gate.sh` is exactly the wave in which the difference shows.
    gate="$(git rev-parse --show-toplevel 2>/dev/null)/scripts/gate.sh"
    [ -x "$gate" ] \
        || gate="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)/gate.sh"
    [ -x "$gate" ] || die "no gate runner at $gate"

    # A plan runs nothing: it goes to the caller as the runner prints it,
    # with no archive, no `latest` and no token -- it builds nothing.
    if [ "$plan_only" = 1 ]; then
        "$gate" "$mode" "${args[@]}"
        return $?
    fi

    root=$(main_root) || exit 2
    plans="$root/plans"
    wdir=$(wave_dir "$plans" "$wave_in") || exit 2
    name="$strand_in"
    [ -n "$name" ] || name=$(strand_of_branch) \
        || die "cannot tell the strand from the branch -- pass --strand"

    archive_root="$plans/$wdir/receipts/$name"

    # THE LANES: `--host`, or the lane this strand holds. Without either the
    # gate is local, as it always was.
    [ -n "$host" ] || host=$(cmd_token lane 2>/dev/null) || host=""
    if [ -n "$host" ]; then
        # A gate on a host binds to a HELD token (review M3): unarmed, or in
        # a tree that is no strand, the check below says nothing -- and two
        # strands could share one host and one target unseen. Then the
        # caller names the reason, like for a local skip.
        if [ -z "${MECLAW_STRAND_TOKEN_SKIP:-}" ] \
            && { [ ! -f "$(token_file)" ] || [ "$(strand_of_branch 2>/dev/null)" = "" ]; }; then
            echo "strand: a gate on a build host binds to a held token -- arm the lanes and" \
                "take one, or name the reason in MECLAW_STRAND_TOKEN_SKIP" >&2
            return 3
        fi
        # The token check names the lane: a strand gates on the lane it holds.
        cmd_token check --pid "$$" --lane "$host" || return $?
        gate_on_host "$mode" "$host" "$name" "$archive_root" "$gate" ${args[@]+"${args[@]}"}
        return $?
    fi

    # The cargo token, BEFORE anything is written: a refused gate leaves no
    # run directory behind. The heartbeat names this shell, which blocks for
    # the whole run, so a gate longer than the TTL keeps its token. The exit
    # of the check goes on as it is, like in `test-tier.sh`: 3 is no token,
    # 2 a broken token file that somebody has to look at (review M4).
    cmd_token check --pid "$$" || return $?

    open_run_dir "$archive_root"
    archive="$RUN_DIR"
    runlog="$archive/run.log"

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
    # Seen after the run, and the pid of the finished run is gone from the
    # entry: from here on the TTL counts. A token released during the run is
    # no refusal (the check stays silent then).
    cmd_token check --pid 0 >/dev/null 2>&1 || true

    # Red and asking stations first, then the summary -- the two things a
    # report needs. ASK is the runner's third summary word (exit 4): a
    # question for the owner is open, and it is never green.
    red_lines "$runlog"
    summary=$(grep -E '^GATE-SUMMARY ' "$runlog" | tail -1)
    if [ -z "$summary" ]; then
        echo "strand: the gate wrote no summary line -- last lines of $runlog:" >&2
        tail -20 "$runlog" >&2
        return "$rc"
    fi
    printf '%s\n' "$summary" >"$archive/summary.txt"
    printf '%s\n' "$summary"
    # The question itself, as the runner printed it after the summary: the
    # three answers and the `--decide` line. Without it the caller holds an
    # ASK and has to open the run log to learn what was asked. Printed
    # whenever the run asked -- a RED run that ALSO asked ("gate: ASK as
    # well") carries the question too, so one run shows every finding.
    sed -n '/^gate: ASK/,$p' "$runlog"
    # The exit goes on unchanged: 4 is the open question, and it must stay
    # apart from 3 (no cargo token) and from 1 (a station is RED).
    return "$rc"
}

# --- a single test on a lane (GH #942) ------------------------------------------

cmd_test() {
    local expr="" host="" strand_in="" wave_in="" extra=()
    while [ $# -gt 0 ]; do
        case "$1" in
            -h|--help) usage; return 0 ;;
            --host)   need_value "$1" "$#"; host="$2"; shift 2 ;;
            --strand) need_value "$1" "$#"; strand_in="$2"; shift 2 ;;
            --wave)   need_value "$1" "$#"; wave_in="$2"; shift 2 ;;
            --)       shift; extra+=("$@"); break ;;
            *)        if [ -z "$expr" ]; then expr="$1"; else extra+=("$1"); fi; shift ;;
        esac
    done
    [ -n "$expr" ] || die "test: expected a nextest filterset, e.g. 'binary(~gh123)'"

    local root plans wdir name archive_root target sync gate errf out prc sha digest
    root=$(main_root) || exit 2
    plans="$root/plans"
    wdir=$(wave_dir "$plans" "$wave_in") || exit 2
    name="$strand_in"
    [ -n "$name" ] || name=$(strand_of_branch) \
        || die "cannot tell the strand from the branch -- pass --strand"
    archive_root="$plans/$wdir/receipts/$name"

    # The lane: `--host`, or the one this strand holds -- and a test binds to a
    # held token like a gate on a host (review M3 of GH #934).
    [ -n "$host" ] || host=$(cmd_token lane 2>/dev/null) || host=""
    if [ -z "$host" ]; then
        echo "strand: test runs on a lane -- take one ('scripts/strand.sh token take')" \
            "or pass --host <lane> with MECLAW_STRAND_TOKEN_SKIP=<reason>" >&2
        return 3
    fi
    if [ -z "${MECLAW_STRAND_TOKEN_SKIP:-}" ] \
        && { [ ! -f "$(token_file)" ] || [ "$(strand_of_branch 2>/dev/null)" = "" ]; }; then
        echo "strand: a test on a build host binds to a held token -- arm the lanes and" \
            "take one, or name the reason in MECLAW_STRAND_TOKEN_SKIP" >&2
        return 3
    fi
    cmd_token check --pid "$$" --lane "$host" || return $?

    target=$(lane_target "$host")
    [ -n "$target" ] || die "no lane $host in the host file $(lanes_file)"
    gate="$(git rev-parse --show-toplevel 2>/dev/null)/scripts/gate.sh"
    sync="$(dirname -- "$gate")/lane_sync.sh"
    [ -x "$sync" ] || sync="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)/lane_sync.sh"
    [ -x "$sync" ] || die "no lane transport at $sync"

    # The transport of a gate, with its refusals (a tracked secret, a host
    # tree that is not this one: exit 5 there, 2 here).
    errf=$(mktemp) || die "no temp file"
    out=$("$sync" push "$target" "$name" 2>"$errf")
    prc=$?
    if [ "$prc" = 5 ]; then
        echo "strand: lane $host refused -- the host would not test this tree:" >&2
        sed 's/^/  /' "$errf" >&2; rm -f "$errf"
        return 2
    elif [ "$prc" != 0 ]; then
        echo "strand: lane $host unreachable" >&2
        sed 's/^/  /' "$errf" >&2; rm -f "$errf"
        return 2
    fi
    rm -f "$errf"
    read -r sha digest <<<"$out"

    # Its own directory, NOT a gate run: `test-` keeps it out of `latest` and
    # out of the run-id glob `report` falls back on.
    local run_dir log t0 rc secs passed total verdict
    run_dir="$archive_root/test-$(date -u +%Y%m%dT%H%M%SZ)-${sha:0:8}"
    while [ -e "$run_dir" ]; do run_dir="$run_dir-2"; done
    mkdir -p "$run_dir" || die "cannot create $run_dir"
    log="$run_dir/test.log"
    printf 'strand: lane %s (%s) rev %s overlay %s\n' "$host" "${target#*@}" "${sha:0:8}" "${digest:-?}"

    t0=$(date +%s)
    "$sync" test "$target" "$name" "$expr" ${extra[@]+"${extra[@]}"} >"$log" 2>&1
    rc=$?
    secs=$(( $(date +%s) - t0 ))
    cmd_token check --pid 0 >/dev/null 2>&1 || true
    if [ "$rc" = 255 ] && ! grep -qE 'tests? run:' "$log"; then
        echo "strand: lane $host unreachable (lost during the test, log: $log)" >&2
        return 2
    fi
    # nextest's summary: `N tests run: P passed, ...` -- the last one counts.
    total=$(grep -oE '[0-9]+ tests? run: [0-9]+ passed' "$log" | tail -1 | sed -E 's/^([0-9]+).*/\1/')
    passed=$(grep -oE '[0-9]+ tests? run: [0-9]+ passed' "$log" | tail -1 | sed -E 's/.*run: ([0-9]+) passed/\1/')
    verdict=GREEN
    [ "$rc" = 0 ] || verdict=RED
    [ "$rc" = 0 ] || tail -20 "$log"
    printf 'TEST [%s] %s/%s %ss %s\n' "$expr" "${passed:-0}" "${total:-0}" "$secs" "$verdict" \
        | tee -a "$log"
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

# `basis` is where the strand stands on master NOW: the merge-base at the time
# of the report. A strand rebases onto master before its cargo phase, and the
# base of its skeleton is then a commit below the new master -- `basis..tip`
# walked master's first-parent line down from there and listed every commit
# and every merge of the other strands. Eight head blocks of one wave were
# corrected by hand (wave Substrat, receipt section 9). Once the branch is in
# master its merge-base IS its tip, and the head block's base is the one that
# still says where the strand started.
fresh = git("merge-base", "master", tip)
if fresh and fresh != git("rev-parse", tip):
    head["basis"] = fresh[:8]
elif not head["basis"]:
    head["basis"] = fresh[:8]

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
# Only GREEN is green. RED is a finding, ASK an open question for the owner,
# and any other word is one this reader does not know -- none of them is a
# gate a report may stand on.
word = head["gate"].strip('"').split()[-1] if head["gate"].strip('"').split() else ""
if word != "GREEN":
    if word == "ASK":
        sys.exit("strand: the gate is ASK, not GREEN -- answer the question "
                 "or measure, then report")
    sys.exit("strand: the gate is %s, not GREEN -- fix it, run it again, "
             "then report" % (word or "empty"))
print("strand: %s -- header block complete, gate green" % report)
PY
    local rc=$? branch
    [ "$rc" = 0 ] || return "$rc"
    # The report ends the cargo phase (plans/PREAMBLE.md section 5), so it
    # gives the token back: a forgotten `release` would block a place in the
    # pipeline until the TTL runs out. The strand is the BRANCH of the head
    # block, whichever tree this runs in.
    if [ -f "$(token_file)" ]; then
        branch=$(awk 'NR == 1 && /^---$/ { head = 1; next }
                      head && /^---$/ { exit }
                      head && sub(/^branch: */, "") { print; exit }' "$REPORT")
        [ -n "$branch" ] && cmd_token release --strand "$branch"
    fi
    return 0
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
# Only GREEN closes an issue (see `report`): ASK is an open question.
word = gate.split()[-1] if gate.split() else ""
if word != "GREEN":
    if word == "ASK":
        sys.exit("strand: the gate is ASK, not GREEN -- answer the question "
                 "or measure, then report")
    sys.exit("strand: the gate is %s, not GREEN -- nothing to close"
             % (word or "empty"))

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

# --- token ------------------------------------------------------------------

# The token file of this host: next to the cargo lock, so every test that
# already points `MECLAW_GATE_LOCK` at a throw-away path runs UNARMED without
# a word about tokens.
token_file() {
    local lock="${MECLAW_GATE_LOCK:-/tmp/meclaw-w26-cargo.lock}"
    printf '%s\n' "${MECLAW_STRAND_TOKENS:-${lock%.lock}.tokens}"
}

# The private host file of the build lanes: `<name> <ssh-target> [<bmc>]` per
# line, `#` a comment. Never in the repository -- it names machines.
lanes_file() {
    printf '%s\n' "${MECLAW_LANES_FILE:-$HOME/.config/meclaw/lanes}"
}

cmd_token() {
    local verb="${1:-}"
    [ $# -ge 1 ] && shift
    case "$verb" in
        take|release|who|check|init|off|lane) ;;
        *) die "token: expected take|release|who|check|init|off|lane" ;;
    esac
    local strand_in="" wave_in="" max="3" ttl="90" pid="" force=0
    local lanes="" lane_want="" max_set=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --strand) need_value "$1" "$#"; strand_in="$2"; shift 2 ;;
            --wave)   need_value "$1" "$#"; wave_in="$2"; shift 2 ;;
            --max)    need_value "$1" "$#"; max="$2"; max_set=1; shift 2 ;;
            --lanes)  need_value "$1" "$#"; lanes="$2"; shift 2 ;;
            --lane)   need_value "$1" "$#"; lane_want="$2"; shift 2 ;;
            --ttl)    need_value "$1" "$#"; ttl="$2"; shift 2 ;;
            --pid)    need_value "$1" "$#"; pid="$2"; shift 2 ;;
            --force)  force=1; shift ;;
            *) die "token $verb: unknown argument: $1" ;;
        esac
    done

    local file branch tree key="" wave root
    file=$(token_file)
    branch=$(git rev-parse --abbrev-ref HEAD 2>/dev/null || true)
    tree=$(git rev-parse --show-toplevel 2>/dev/null || true)
    case "$strand_in" in
        */*) key="$strand_in" ;;
        "")  case "$branch" in */*) key="$branch" ;; esac ;;
        *)
            if [ -n "$wave_in" ]; then
                root=$(main_root) || exit 2
                wave=$(wave_dir "$root/plans" "$wave_in") || exit 2
                wave=$(wave_name "$wave")
            else
                case "$branch" in
                    */*) wave=${branch%%/*} ;;
                    *) die "token: --strand $strand_in needs --wave, a strand branch, or the form <wave>/<name>" ;;
                esac
            fi
            key="$wave/$strand_in" ;;
    esac

    if [ "$verb" = check ] || [ "$verb" = lane ]; then
        # Unarmed host, or a tree that is no strand (the main tree, a
        # detached HEAD): nothing to check, and nothing to say.
        [ -f "$file" ] || return 0
        [ -n "$key" ] || return 0
    fi
    if [ -n "$lanes" ]; then
        [ "$verb" = init ] || die "token $verb: --lanes belongs to init"
        [ "$max_set" = 0 ] || die "token init: --lanes and --max exclude each other" \
            "(a lane host file entry is one token; the count is the list)"
    fi
    case "$verb" in
        take|release)
            [ -n "$key" ] || die "token $verb: cannot tell the strand from the branch -- pass --strand" ;;
    esac

    MECLAW_T_VERB="$verb" MECLAW_T_KEY="$key" MECLAW_T_BRANCH="$branch" \
    MECLAW_T_TREE="$tree" MECLAW_T_FILE="$file" MECLAW_T_MAX="$max" \
    MECLAW_T_TTL="$ttl" MECLAW_T_PID="$pid" MECLAW_T_FORCE="$force" \
    MECLAW_T_LANES="$lanes" MECLAW_T_LANE="$lane_want" \
    MECLAW_T_LANES_FILE="$(lanes_file)" \
    MECLAW_T_SELF="$$" python3 - <<'PY'
import fcntl, json, os, sys, time

verb = os.environ["MECLAW_T_VERB"]
key = os.environ["MECLAW_T_KEY"]
branch = os.environ["MECLAW_T_BRANCH"]
tree = os.environ["MECLAW_T_TREE"]
path = os.environ["MECLAW_T_FILE"]
force = os.environ["MECLAW_T_FORCE"] == "1"
pid_arg = os.environ["MECLAW_T_PID"]
caller = int(os.environ["MECLAW_T_SELF"])   # the kit process that asks
LOG = path + ".log"
now = int(os.environ.get("MECLAW_STRAND_NOW") or time.time())   # TEST HOOK

REFUSED = ("no cargo token for %s -- run 'scripts/strand.sh token take' first. "
           "Without a token: write code, tests and docs without cargo, then end "
           "your turn and wait to be woken (plans/PREAMBLE.md section 5).")


def say(msg):
    print("strand: " + msg, file=sys.stderr)


def fail(msg):
    say(msg)
    sys.exit(2)


def whole(name, value):
    try:
        n = int(value)
    except ValueError:
        n = 0
    if n < 1:
        fail("token %s: %s must be a whole number of at least 1, not %r"
             % (verb, name, value))
    return n


def iso(t):
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(t))


def mins(t):
    return max(0, (now - t) // 60)


def log(event, strand, detail=""):
    with open(LOG, "a") as fh:
        fh.write("\t".join([iso(now), event, strand, branch or "-", detail]) + "\n")


def load():
    """The state, None for an unarmed host -- and never a silent new file
    over a broken one: that would hand out tokens somebody already holds."""
    try:
        text = open(path).read()
    except FileNotFoundError:
        return None
    try:
        st = json.loads(text)
        if not (isinstance(st["holders"], list) and isinstance(st["waiting"], list)):
            raise ValueError
        st["max"], st["ttl_min"] = int(st["max"]), int(st["ttl_min"])
        for h in st["holders"]:
            # The first form of the file kept one `pid` per holder.
            old = h.pop("pid", 0)
            h["pids"] = [int(p) for p in h.get("pids", [old] if old else [])]
    except (ValueError, KeyError, TypeError, AttributeError):
        fail("the token file %s is broken -- look at it, then "
             "'scripts/strand.sh token init --force'" % path)
    return st


def write(st):
    tmp = "%s.tmp.%d" % (path, os.getpid())
    with open(tmp, "w") as fh:
        json.dump(st, fh, indent=2)
        fh.write("\n")
    os.replace(tmp, path)


def alive(pid):
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    return True


def stale(h, st):
    if h.get("tree") and not os.path.isdir(h["tree"]):
        return "its worktree is gone"
    if now - h["seen"] <= st["ttl_min"] * 60 or any(alive(p) for p in h["pids"]):
        return ""
    return "last seen %d min ago, no live process" % mins(h["seen"])


def stale_wait(w, st):
    # A waiter in its own worktree is alive as long as the tree is: it waits
    # on purpose, and with three tokens for eight strands a wait beyond the
    # TTL is the normal case -- the drop landed at somebody else's `take`, so
    # the strand that lost its place never heard of it (review I1). The TTL
    # is for waiters queued from outside (`--strand`), which have no tree.
    if w.get("tree"):
        return "" if os.path.isdir(w["tree"]) else "its worktree is gone"
    if now - w["since"] > st["ttl_min"] * 60:
        return "waiting %d min, nobody took it up" % mins(w["since"])
    return ""


def holder(st, strand):
    return next((h for h in st["holders"] if h["strand"] == strand), None)


LANES_FILE = os.environ["MECLAW_T_LANES_FILE"]


def host_lanes():
    """The lane names of the host file: `<name> <ssh-target> [<bmc>]` per
    line, `#` starts a comment. The file is private to the host -- it names
    machines -- and never part of the repository (GH #934)."""
    try:
        text = open(LANES_FILE).read()
    except OSError:
        fail("token init: --lanes needs the host file %s (lines "
             "'<name> <ssh-target> [<bmc>]')" % LANES_FILE)
    names = []
    for line in text.splitlines():
        words = line.split("#", 1)[0].split()
        if len(words) >= 2:
            names.append(words[0])
    return names


def free_lane(st):
    """The first lane in the order of `init --lanes` nobody holds."""
    held = {h.get("lane") for h in st["holders"]}
    return next((n for n in st["lanes"] if n not in held), None)


def refuse_over_a_queue(st, way_out):
    """Exit 2 while anybody holds a token or waits for one -- the verbs that
    throw the state away (`init`, `off`) never do it over a queue."""
    if st and (st["holders"] or st["waiting"]):
        who = ["%s holds a cargo token" % h["strand"] for h in st["holders"]]
        who += ["%s waits in the queue" % w["strand"] for w in st["waiting"]]
        fail("token %s: %s -- %s" % (verb, ", ".join(who), way_out))


mine_tree = tree if branch == key else ""

# Unarmed means without a trace: only `init` creates a file, the `.lock`
# included -- a `take` on an unarmed host left an empty one behind (review
# M1). Arming happens only through `init`, so the look outside the lock
# cannot miss a file that another call is writing.
if verb != "init" and not os.path.exists(path):
    if verb == "who":
        print("cargo tokens are not armed on this host (%s)" % path)
    elif verb == "off":
        say("cargo tokens are not armed on this host -- nothing to switch off")
    elif verb in ("take", "release"):
        say("cargo tokens are not armed on this host -- nothing to %s" % verb)
    sys.exit(0)

# One writer at a time, reader included: `who` must not see half a queue.
with open(path + ".lock", "a") as guard:
    fcntl.flock(guard, fcntl.LOCK_EX)

    if verb == "init":
        cap, ttl = whole("--max", os.environ["MECLAW_T_MAX"]), whole("--ttl", os.environ["MECLAW_T_TTL"])
        lanes = [n for n in os.environ["MECLAW_T_LANES"].split(",") if n]
        if os.environ["MECLAW_T_LANES"] and lanes:
            known = host_lanes()
            unknown = [n for n in lanes if n not in known]
            if unknown:
                fail("token init: no lane %s in the host file %s"
                     % (", ".join(unknown), LANES_FILE))
            if len(set(lanes)) != len(lanes):
                fail("token init: a lane is named twice in --lanes")
            cap = len(lanes)
        elif os.environ["MECLAW_T_LANES"]:
            fail("token init: --lanes needs at least one name")
        if not force:
            # Holders AND waiters: a re-init in the middle of a wave emptied
            # the queue without a word (review M6).
            refuse_over_a_queue(load(), "pass --force to start over")
        fresh = {"max": cap, "ttl_min": ttl, "armed": iso(now),
                 "holders": [], "waiting": []}
        if lanes:
            fresh["lanes"] = lanes
        write(fresh)
        log("init --force" if force else "init", "-", "max %d ttl %d%s"
            % (cap, ttl, (" lanes " + ",".join(lanes)) if lanes else ""))
        if lanes:
            say("build lanes armed: %s, ttl %d min (%s)" % (" ".join(lanes), ttl, path))
        else:
            say("cargo tokens armed: %d, ttl %d min (%s)" % (cap, ttl, path))
        sys.exit(0)

    if verb == "off":
        if not force:
            # The same check as `init`: `off` dropped the waiters without a
            # word and only looked at the holders (review fix round 1, m3).
            refuse_over_a_queue(load(), "release them or pass --force")
        if not os.path.exists(path):
            say("cargo tokens are not armed on this host -- nothing to switch off")
            sys.exit(0)
        os.remove(path)
        log("off --force" if force else "off", "-")
        say("cargo tokens switched off; the log stays (%s)" % LOG)
        sys.exit(0)

    st = load()

    if verb == "who":
        if st is None:
            print("cargo tokens are not armed on this host (%s)" % path)
            sys.exit(0)
        print("cargo tokens: %d/%d held, ttl %d min, armed %s"
              % (len(st["holders"]), st["max"], st["ttl_min"], st.get("armed", "?")))
        if st.get("lanes"):
            print("lanes: %s" % " ".join(st["lanes"]))
        for h in st["holders"]:
            why = stale(h, st)
            pids = ", ".join("%d (%s)" % (p, "alive" if alive(p) else "gone")
                             for p in h["pids"])
            print("  %s%s  held %d min, seen %d min ago, pid %s%s"
                  % (h["strand"], ("  lane " + h["lane"]) if h.get("lane") else "",
                     mins(h["since"]), mins(h["seen"]), pids or "-",
                     ("  STALE: " + why) if why else ""))
        if st["waiting"]:
            print("queue:")
            for n, w in enumerate(st["waiting"], 1):
                why = stale_wait(w, st)
                if why:
                    note = "  STALE: " + why
                elif now - w["since"] > st["ttl_min"] * 60:
                    # A waiter in a standing tree never expires (review I1),
                    # so a builder that died while it waited blocks the
                    # queue until `release --strand`. Say so -- a hint, the
                    # entry stays (review fix round 1, m1).
                    note = "  waiting beyond the ttl"
                else:
                    note = ""
                print("  #%d %s  waiting %d min%s" % (n, w["strand"], mins(w["since"]), note))
        else:
            print("queue: empty")
        try:
            tail = open(LOG).read().splitlines()[-5:]
        except FileNotFoundError:
            tail = []
        if tail:
            print("last events:")
            for line in tail:
                print("  " + "  ".join(line.split("\t")))
        sys.exit(0)

    if st is None:
        if verb in ("take", "release"):
            say("cargo tokens are not armed on this host -- nothing to %s" % verb)
        sys.exit(0)

    if verb == "lane":
        # The lane this strand holds, or nothing -- what `gate` asks when it
        # was given no `--host`.
        mine = holder(st, key)
        if mine and mine.get("lane"):
            print(mine["lane"])
        sys.exit(0)

    if verb == "check":
        skip = os.environ.get("MECLAW_STRAND_TOKEN_SKIP", "")
        if skip:
            log("skip", key, skip)
            write(st)
            say("token check skipped for %s: %s" % (key, skip))
            sys.exit(0)
        mine = holder(st, key)
        want = os.environ["MECLAW_T_LANE"]
        if mine and want and st.get("lanes") and mine.get("lane") != want:
            # A gate on a host is a run on THAT lane: two strands on one host
            # share one target directory and one cargo lock (GH #934).
            log("refused", key, "lane %s" % want)
            say("%s holds lane %s, not %s -- gate on your own lane, or release "
                "and take again" % (key, mine.get("lane") or "-", want))
            sys.exit(3)
        if mine:
            mine["seen"] = now
            if pid_arg:
                # Every live process of the strand holds the token, not the
                # last one to say so: a single test beside a background gate
                # wrote its pid over the gate's, and after the test the entry
                # named a dead process -- a gate running past the TTL lost its
                # token (review M2). `--pid 0` takes back the caller's own pid
                # (the gate after its run); dead pids fall out on the way.
                pids = [p for p in mine["pids"] if p != caller and alive(p)]
                new_pid = int(pid_arg) if pid_arg.isdigit() else 0
                if new_pid and new_pid not in pids:
                    pids.append(new_pid)
                mine["pids"] = pids
            write(st)
            sys.exit(0)
        if pid_arg == "0":
            # The heartbeat after a run: the token was released while it ran,
            # by the orchestrator or by the station itself. Nothing is being
            # refused -- the run is over (review M3).
            sys.exit(3)
        log("refused", key)
        say(REFUSED % key)
        sys.exit(3)

    if verb == "release":
        mine = holder(st, key)
        queued = any(w["strand"] == key for w in st["waiting"])
        if not mine and not queued:
            say("%s holds no cargo token and is not in the queue -- nothing to release" % key)
            sys.exit(0)
        st["holders"] = [h for h in st["holders"] if h["strand"] != key]
        st["waiting"] = [w for w in st["waiting"] if w["strand"] != key]
        write(st)
        by = "" if branch == key else " (released by %s)" % (branch or "a detached tree")
        if not mine:
            log("leave" if not by else "leave-by", key)
            say("%s left the queue%s" % (key, by))
            sys.exit(0)
        log("release" if not by else "release-by", key, "held %d min" % mins(mine["since"]))
        if st["waiting"]:
            nxt = st["waiting"][0]
            tail = "next in the queue: %s (waiting %d min)" % (nxt["strand"], mins(nxt["since"]))
        else:
            tail = "the queue is empty"
        say("cargo token released by %s after %d min -- %d/%d held; %s%s"
            % (key, mins(mine["since"]), len(st["holders"]), st["max"], tail, by))
        sys.exit(0)

    # take
    for h in list(st["holders"]):
        why = stale(h, st)
        if why:
            st["holders"].remove(h)
            log("stale", h["strand"], why)
            say("reclaimed the cargo token of %s -- %s (ttl %d min)"
                % (h["strand"], why, st["ttl_min"]))
    # A waiter nobody wakes any more would hold its place for everybody behind
    # it. The caller's own entry is exempt: its `take` IS the sign of life.
    for w in list(st["waiting"]):
        why = "" if w["strand"] == key else stale_wait(w, st)
        if why:
            st["waiting"].remove(w)
            log("stale-wait", w["strand"], why)
            say("dropped %s from the queue -- %s (ttl %d min)"
                % (w["strand"], why, st["ttl_min"]))
    mine = holder(st, key)
    if mine:
        mine["seen"] = now
        write(st)
        say("cargo token already held by %s (%d/%d)" % (key, len(st["holders"]), st["max"]))
        if mine.get("lane"):
            print("lane %s" % mine["lane"])
        sys.exit(0)
    names = [w["strand"] for w in st["waiting"]]
    pos = names.index(key) if key in names else len(names)
    free = st["max"] - len(st["holders"])
    # FIFO: a token goes to the head of the queue -- a strand that just
    # finished writing does not overtake one that has been waiting.
    if pos < free:
        st["waiting"] = [w for w in st["waiting"] if w["strand"] != key]
        entry = {"strand": key, "since": now, "seen": now,
                 "pids": [], "tree": mine_tree}
        lane = free_lane(st) if st.get("lanes") else None
        if lane:
            entry["lane"] = lane
        st["holders"].append(entry)
        log("take", key, ("lane " + lane) if lane else "")
        write(st)
        say("cargo token %d/%d taken by %s" % (len(st["holders"]), st["max"], key))
        if lane:
            # stdout, one line: the one thing a builder needs from `take`.
            print("lane %s" % lane)
        sys.exit(0)
    if key not in names:
        st["waiting"].append({"strand": key, "since": now, "tree": mine_tree})
        log("wait", key)
    write(st)
    held = ", ".join("%s %d min" % (h["strand"], mins(h["since"])) for h in st["holders"])
    if free <= 0:
        head = "all %d cargo tokens are held (%s)" % (st["max"], held)
    else:
        head = ("%d of %d cargo tokens are held (%s) and the queue goes first"
                % (len(st["holders"]), st["max"], held or "none"))
    say("%s -- %s is #%d in the queue. End your turn; you are woken when a "
        "token is free." % (head, key, pos + 1))
    sys.exit(3)
PY
}

# --- lanes ------------------------------------------------------------------

# `lanes status`: one line per lane of the host file -- reachable or not, the
# file system type (`tmpfs` or a disk) and free space of the host's target
# root, and how many strand refs its bare repository holds. The only lane verb
# that reads; switching a host on or off stays a step of the orchestrator's
# (THE LANES).
cmd_lanes() {
    [ "${1:-}" = status ] || die "lanes: expected status"
    local file sync name target rest
    file=$(lanes_file)
    [ -f "$file" ] || die "no lane host file at $file"
    sync="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)/lane_sync.sh"
    [ -x "$sync" ] || die "no lane transport at $sync"
    while read -r name target rest; do
        case "$name" in ''|'#'*) continue ;; esac
        [ -n "$target" ] || continue
        case "$target" in '#'*) continue ;; esac
        printf '%s  %s\n' "$name" "$("$sync" status "$target" </dev/null)"
    done <"$file"
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
    test)   cmd_test "$@" ;;
    report) cmd_report "$@" ;;
    close)  cmd_close "$@" ;;
    token)  cmd_token "$@" ;;
    lanes)  cmd_lanes "$@" ;;
    *) die "unknown subcommand: $sub (expected new|gate|test|report|close|token|lanes)" ;;
esac
