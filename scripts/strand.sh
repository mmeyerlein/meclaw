#!/bin/bash -p
# meclaw -- the strand kit. One command for the four steps every strand of a
# wave repeats by hand.
#
#     scripts/strand.sh -C <worktree> <subcommand> ...
#         (as `git -C`: a `cd <worktree>` first, for every subcommand)
#     scripts/strand.sh new [<wave>/]<name> --issue <nr> [options]
#         (the prefix names the wave when section Live runs several; GH #1226)
#     scripts/strand.sh pre [--strand <S>] [--no-lane] [--host <lane>] [--holder <n>] [--allow-bump <reason>]
#         (the pre-run before a strand gate: form, locks, cargo check; writes the stamp)
#     scripts/strand.sh gate [<mode>] [--host <lane>] [--holder <n>] [--no-pre --reason <text>] [gate.sh options]
#         (--wait-admission: block while queued instead of exit 3, woken by a lease end)
#     scripts/strand.sh test '<nextest filterset>' [--host <lane>] [--holder <n>] [nextest args]
#         (a second lease is --holder <n>, never --strand <s>:<n> -- refused, GH #1146)
#         (GH277_WRITE_GOLDEN=1 / MECLAW_GH935_BLESS=1 travel; the goldens come back)
#     scripts/strand.sh test --unittest <module|test id|file.py> ...
#         (the kit's Python unittests from the root of this tree, on this host:
#         no lane, no token; one log, one line TEST [unittest ...])
#     scripts/strand.sh report [--strand <name>]
#     scripts/strand.sh close [--strand <name>] [--wave <dir>] [--do]
#     scripts/strand.sh merge [--strand <name> | --batch <s1,s2>] [--wave <dir>] [--host <lane>]
#         [--no-pre --reason <text>] [--no-close] [--dequeue] [--wait]
#         (in a strand worktree the strand comes from the branch, as for gate/report)
#         (the one way into master: lease tree:master, tested candidate, versions
#          at the merge point; exit 3 = queued behind another merge;
#          --wait blocks in the queue until the lease is released instead)
#     scripts/strand.sh revert <merge-sha|name> --reason <text> [--wave <dir>] [--host <lane>]
#     scripts/strand.sh token take|release|who|check|init|off|lane [options]
#     scripts/strand.sh lanes status [<lane>]
#     scripts/strand.sh lanes idle-off [--after <h>] [--lane <name>]
#     scripts/strand.sh lanes release <lane> --run <run> [--by <who>]
#     scripts/strand.sh lanes browsers <lane> check|install [<engine>...]
#     scripts/strand.sh bench new <name> --ttl <h> [--lane <name>]
#     scripts/strand.sh bench collect <name> [--lane <name>]
#     scripts/strand.sh bench list [--lane <name>]
#     scripts/strand.sh wave open <wave> --title "<English sentence>"
#         (the Running row on top of plans/README.md section Live, committed)
#     scripts/strand.sh wave close <wave> [--dry-run] [--issue <n>]... [--bump]
#         (issues lock, retro, bump line, Orga inputs, export -- `pipeline wave close`)
#
# SINCE W3 (GH #1117) the kit is the entry and `scripts/pipeline.py` the core:
# `gate` runs this tree's `scripts/gate.sh <mode>` -- the shell in front of
# `pipeline.py gate` -- with `--strand <name> --wave <wave> --archive
# plans/<wave dir>/receipts/<strand>` and prints what it printed before (the
# red stations, the summary, the question block); `test` runs `pipeline.py
# test`; `token ...` and `lanes status` run `pipeline.py lanes ...` on the
# store (`MECLAW_PIPELINE_DB`), words and exit codes 1:1, `token` stays the
# alias and `MECLAW_STRAND_TOKEN_SKIP` still counts -- with two exceptions
# (Z11): `token take` on an unarmed store is exit 2 with the way to arm it,
# never a silent pass (`check` stays exit 0), and an old token file beside an unarmed store is
# carried into it once (lanes, max, TTL, holders, queue) or the verb fails
# with the reason. `gate ci` stays on the old runner
# (`scripts/gate_legacy.sh`, local only: `--host` is refused) for good;
# there is no other way back to it (GH #1156).
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
# host with an unarmed store (a public clone, CI, every test, any work
# outside a wave), a tree whose branch has no `/` (the main tree, where the
# orchestrator runs the pass), a tier run inside a gate, and
# `MECLAW_STRAND_TOKEN_SKIP=<reason>` -- which is logged, not silent.
#
#     token init [--max N] [--ttl MIN] [--force]   arm the host (refuses over a queue or a live run)
#     token init --lanes a,b,c [--ttl MIN]         arm one named token per build lane
#     token take [--strand S] [--lane NAME]        hold a token (that lane) or join the queue
#     token take ... --wait                         block until taken (start it in the background;
#                                                   the queue line says it blocks, it never returns queued)
#     token release [--strand S] [--force]         give it back, name the next (a lease of a live
#                                                   run, a process in the lane tree or a lane that
#                                                   does not answer stays, exit 3: its end frees it;
#                                                   --force ends the lane tree)
#     token who                                    holders, queue, last events
#     token check [--pid P]                        what the gate and the tier call
#     token off [--force]                          disarm (refuses over a queue; the log stays)
#     token lane [--strand S]                      the lane this strand holds, or nothing
#     token take|release|check|lane ... --holder N  a second lease of the strand (`<strand>:N`,
#                                                   GH #1146); `gate`/`test --holder N` run under it
#
# The file lives next to the cargo lock: `${MECLAW_GATE_LOCK%.lock}.tokens`
# (default /tmp/meclaw-w26-cargo.tokens), or `MECLAW_STRAND_TOKENS`, with
# `.lock` (flock) and `.log` (append-only TSV) beside it. A holder is STALE
# when its worktree is gone, or when it was last seen longer ago than the
# TTL and no live process holds it -- a running gate keeps its token as long
# as it runs. A waiter in its own worktree keeps its place as long as the
# tree stands (`who` marks it once it waits beyond the TTL); one queued from
# outside leaves after the TTL. A token or lane free for a waiter is OFFERED
# to it (`who`: `offered <lane> at <time>`); a waiter beyond the TTL that
# leaves an offer untaken for one poll period (10 min) drops out at the next
# `take` of another strand, and the offer moves on (GH #1053). `init` and
# `off` refuse while anybody holds
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
# and prints `lane <name>` on stdout, `who` shows who holds which. `take
# --lane <name>` takes THAT lane or waits in the queue for it -- a strand whose
# station needs one host (a browser lab set up on one lane only) must not be
# handed the first free one; a named waiter keeps its lane against a later
# take, and a lane nobody ahead wants still goes to the next. `--lanes`
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
# and a difference is refused too. The run on the host is the pipeline's
# (`pipeline.py exec`), against the merge base computed HERE, where `master` is. Receipt and
# station logs come back into the wave's archive -- or the run is not green;
# the summary, the red stations and the exit code (1/2/4) are the runner's,
# after one first line `strand: lane <name> (<host>) rev <sha> overlay
# <hash>`. Without a held token, `--host` wants MECLAW_STRAND_TOKEN_SKIP.
# The skip passes the token CHECK only: the scheduler's admission still
# queues the pass behind the lease queue (`lane <x> is leased to another
# strand or a waiter ahead`, exit 3) -- on purpose, a skip never takes a
# leased lane (kit find (d), 09.10.).
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
#     test '<filterset>' [--host <lane>] [--holder <n>]   one nextest filterset on the lane
#
# The same transport as a gate (push, overlay, the same refusals), then
# `scripts/test-tier.sh filter '<filterset>'` in the host's tree. The whole
# log comes back to `receipts/<strand>/test-<run>/test.log` -- `latest`, the
# gate's pointer, is not moved -- and the last line is
# `TEST [<filterset>] <passed>/<total> <secs>s GREEN|RED`; the exit is the
# remote one (3 = no lane held, 2 = a wrong call or an unreachable host,
# 6 = no test ran: `RED (no tests matched)` -- a filter that matched nothing,
# a summary of zero, no summary, or a filterset nextest refused; GH #997).
# It blocks like a gate: start it in the background and wait.
#
# A host that does not answer is exit 2 with `strand: lane <name> unreachable`,
# no `latest` and the token still held: whether to take another lane or gate
# locally is the orchestrator's call, never a silent fallback.
#
# The lanes are named in a PRIVATE host file, `${MECLAW_LANES_FILE:-
# ~/.config/meclaw/lanes}`: `<name> <ssh-target> [<bmc>] [threads=<n>]` per
# line, `#` a comment. `threads=<n>` caps the test threads and the build jobs
# of that lane (default: every core); old readers skip the word. Further
# words (W1, read by `scripts/lane_sync.sh`): `class=egon` (the protection
# class of the owner's machine, with `jobs=<n>` and `watch=<unit>`),
# `slots=<n>` (build paths with their own target, default 1), `sccache=1`,
# `linker=mold|wild`. A lane on THIS machine reads `<name> local <root> ...`:
# no ssh, its root in place of the bmc. It is never
# in the repository -- it names machines -- and it holds no password. Build hosts are normally switched off; switching one on or off
# is a step of the orchestrator's, outside this kit, and `lanes status` is
# the one lane verb that reads: per lane reachable or not, the file system
# type and free space of its `/srv/target`, strand refs in its bare
# repository. On a tmpfs the first gate after a boot builds cold and pushes
# the history; on a disk the target stays warm (GH #942).
#
# THE LANE HYGIENE (`scripts/pipeline/hygiene.py`, W2 of the dev-tempo
# wave). On 08.10.2026 four disks ran full in one day -- a linker `Bus error`
# at 100 %, 124 G of bench leftovers, five generations of test binaries in one
# `target/debug/deps` -- and each was found by hand, after the run it cost.
# Since then a gate on a lane asks the host first: the pipeline pipes
# `hygiene.py check` over ssh before the runner starts and `collect` after it;
# no room even after collecting is exit 2 with `refused the run -- its
# hygiene check found no room`, before a single crate is built
# (`MECLAW_HYGIENE=0` switches the check off). `lanes status` appends the
# hygiene's own word on the host to its line: warm trees, artifact
# generations, zombie scopes, stray targets.
#
#     bench new <name> --ttl <h> [--lane <name>]   a managed bench directory, path on stdout
#     bench collect <name> [--lane <name>]        remove it now
#     bench list [--lane <name>]                  the bench directories and their rest TTL
#
# A bench or comparison build gets its own directory under the lane root
# (`bench/<name>/`, its `CARGO_TARGET_DIR`) with a time to live: a one-shot
# timer on the host removes it when the TTL is over, and every `check`
# sweeps the ones whose timer was lost. A bare `target/` beside the layout is
# a finding of the next check (`stray-target`), not a thing that waits for
# somebody to notice the disk. Without `--lane` the hygiene runs here.
#
#     lanes idle-off [--after <h>] [--lane <name>]
#
# A build host that nobody uses switches itself off: one re-armable one-shot
# timer per lane on THIS machine (`systemd-run --user`, unit
# `pipe-idle-<lane>`), default 2 h (`MECLAW_IDLE_OFF_H`), that powers the host
# off when it fires and no strand holds the lane (`token who`). Every gate on
# a lane arms it again, so the clock starts at the end of the last run;
# `MECLAW_IDLE_OFF_H=0` arms nothing. A lane with `class=egon` in the host
# file, or the target `local`, is never armed. Switching a host ON stays the
# orchestrator's step; `systemctl --user stop pipe-idle-<lane>.timer` disarms.
#
#     lanes release <lane> --run <run> [--by <who>]
#
# A run whose driver ended without closing it (killed, a chain that stopped)
# keeps its task runs `dispatched` and the lane busy in the admission until
# deadline + grace. `lanes release` closes it the store's way (`close()`:
# workpackage `stopped/aborted`, open task runs `failed/orphan`, journal
# `run_closed reason=orphan by=<who>`) -- only when no local driver pid of the
# run lives (workpackage doc, lease entries, journal), the lane reports the
# run's `pipe-<run>.scope` gone and no process lives in its workspace or its
# tree. Any other answer, or none, refuses with the reason and exit 2. A run
# closed `stopped/lost` (its lease, run and runner stay until the deadline) is
# open too (#1228 round 3), and `token take` closes such a run -- or an
# `active` one whose driver died -- by itself on the same checks.
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
# Exit 0 = the step succeeded. `gate` passes the engine's codes through:
# GREEN 0, RED 1, ABORTED or a refused argument 2, queued 3, ASK 4, a lane
# that refuses this tree 5, a store error at the admission 6.

# One group, parsed whole before its first command runs: an in-place edit of this file under a running shell changes nothing that runs (GH #1223).
{
set -uo pipefail

# The kit's directory, absolute before the entry below changes the working
# directory -- `lane_sync.sh`, `gate.sh` and `pipeline.py` beside it belong
# to the same tree.
# This file's directory with builtins only: an external `dirname` missing from
# PATH made it the caller's cwd and the kit library came from there (M7w).
case "${BASH_SOURCE[0]}" in */*) KIT_DIR=${BASH_SOURCE[0]%/*}/ ;; *) KIT_DIR=./ ;; esac
KIT_DIR=$(cd -- "$KIT_DIR" && pwd) || exit 2
# The kit's one Python (`KIT_PY`, lane.PYTHON; re-check tv-bn M7t B-M4).
# shellcheck source=scripts/pipeline/kit_python.sh
. "$KIT_DIR/pipeline/kit_python.sh" || exit 2

usage() {
    # The header block above IS the help text: everything from the line after
    # the shebang up to the first line that is not a comment.
    awk 'NR > 1 && /^#/ { sub(/^# ?/, ""); print; next } NR > 1 { exit }' \
        "$KIT_DIR/$(basename -- "${BASH_SOURCE[0]}")"
}

die() { echo "strand: $*" >&2; exit 2; }

REPORT=""; ARCHIVE=""; S_REL=""

need_value() {
    [ "$2" -ge 2 ] || die "$1 needs a value"
}

# The pipeline core next to this kit, and the store it keeps the leases in.
pipeline_py() {
    printf '%s/pipeline.py\n' "$KIT_DIR"
}
pipeline_db() {
    printf '%s\n' "${MECLAW_PIPELINE_DB:-$HOME/.local/state/meclaw/pipeline/pipeline.db}"
}

# Is the store armed -- the cargo pool of `pipeline.py lanes init`? The query
# of `meclaw_lanes_active` in `cargo-target.sh`, without asking for lanes.
store_armed() {
    local db
    db=$(pipeline_db)
    [ -f "$db" ] || return 1
    "${KIT_PY[@]}" -c 'import sqlite3, sys, urllib.parse
try:
    db = sqlite3.connect("file:%s?mode=ro" % urllib.parse.quote(sys.argv[1]), uri=True, timeout=5)
    row = db.execute("SELECT 1 FROM lease_pools WHERE pool = ? AND armed IS NOT NULL",
                     ("cargo",)).fetchone()
except sqlite3.Error:
    sys.exit(1)
sys.exit(0 if row else 1)' "$db" 2>/dev/null
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
    local plans="$1" branch wave cand top hits=() owners=()
    branch=$(git rev-parse --abbrev-ref HEAD 2>/dev/null) || return 1
    top=$(git rev-parse --show-toplevel 2>/dev/null) || top=""
    case "$branch" in */*) wave=${branch%%/*} ;; *) return 1 ;; esac
    for cand in "$plans/$wave"-*/; do
        [ -d "$cand" ] || continue
        hits+=("$(basename -- "${cand%/}")")
        # The report lives in the strand's worktree since #1148; the main
        # tree's copy (older strands) counts as well.
        if [ -f "${cand}berichte/${branch##*/}.md" ] || { [ -n "$top" ] \
                && [ -f "$top/plans/${hits[-1]}/berichte/${branch##*/}.md" ]; }; then
            owners+=("${hits[-1]}")
        fi
    done
    if [ ${#hits[@]} -eq 1 ]; then
        printf '%s\n' "${hits[0]}"
    elif [ ${#owners[@]} -eq 1 ]; then
        printf '%s\n' "${owners[0]}"
    else
        return 1
    fi
}

# The wave directory under `plans/`: `--wave` wins, then the branch, then the
# one row of `plans/README.md` section Live that says `**Running since <date>:**`
# (`strand.sh wave open`, welle-validierung-2 3b, GH #1199), and only then the
# first row that names a directory which exists.
#
# Two Running rows (parallel waves) are no guess either: a stop naming both.
# The last step is a GUESS, and it used to be a silent one: without a Running
# row section Live is an archive in order -- its top row is the wave built
# LAST. So a row that says it is built is not a fallback, it is a stop: the
# report and the gate archive of a running strand would land in a wave that is
# over.
wave_dir() {
    local plans="$1" want="${2:-}" cand row rest names
    local -a running=()
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
    # The Running rows are read by the one reader `wave open`/`wave close` write
    # them for (`pipeline wave running`, wave.RUNNING): a second spelling of the
    # marker here would take rows the core does not (re-check v2-kit c23 M-5).
    names=$(running_waves "$plans") \
        || die "plans/README.md section Live is unreadable -- pass --wave <directory>"
    while IFS= read -r cand; do
        [ -n "$cand" ] && running+=("$cand")
    done <<<"$names"
    if [ ${#running[@]} -eq 1 ]; then
        echo "strand: wave ${running[0]} (Running row of plans/README.md section Live)" >&2
        printf '%s\n' "${running[0]}"
        return 0
    fi
    if [ ${#running[@]} -gt 1 ]; then
        die "plans/README.md section Live names ${#running[@]} running waves"\
            "(${running[*]}) -- pass --wave <directory>"
    fi
    while IFS= read -r row; do
        cand=${row%%	*}; rest=${row#*	}
        [ -d "$plans/$cand" ] || continue
        case "$rest" in
            '**Built'*|'**Done'*|'**Gebaut'*)
                die "the newest wave in plans/README.md section Live is over"\
                    "($cand) and the running one is not in the table yet --"\
                    "pass --wave <directory>" ;;
        esac
        echo "strand: wave $cand (top row of plans/README.md section Live)" >&2
        printf '%s\n' "$cand"
        return 0
    done < <(live_rows "$plans/README.md")
    die "no wave directory found in plans/README.md section Live -- pass --wave"
}

# The Running rows of section Live whose directory exists, one per line -- the
# one reader `wave open`/`wave close` write them for (`pipeline wave running`).
# Exit 1 when the reader fails.
running_waves() {
    local plans="$1" names cand
    names=$("${KIT_PY[@]}" "$(pipeline_py)" wave running "$plans/README.md") || return 1
    while IFS= read -r cand; do
        [ -n "$cand" ] && [ -d "$plans/$cand" ] && printf '%s\n' "$cand"
    done <<<"$names"
    return 0
}

# GH #1226: the wave `new` sets a strand up in when `--wave` does not say it. Two
# waves ran on the same lanes (welle-validierung-2 and welle-kfix, 16.10.), section
# Live named both, and every `new` stopped for a `--wave` by hand -- although the
# strand already said which wave is its own. In this order:
#   1. the prefix of `new <wave>/<name>` -- the branch the strand will carry: the
#      one directory `plans/<wave>-<date>` (of several, the one Running row);
#      a prefix that names none, or several Running ones, is a stop;
#   2. of two or more Running rows, the one wave whose `straenge/<name>.json` the
#      orchestrator wrote before `new` (the kinds, PREAMBLE section 4);
#   3. nothing -- `wave_dir` decides, as for every other command; with two or
#      more Running rows and no answer above, the stop names both ways out.
new_wave_hint() {
    local plans="$1" prefix="$2" name="$3" cand list
    local -a hits=() running=() owners=() live=()
    list=$(running_waves "$plans" 2>/dev/null) || list=""
    while IFS= read -r cand; do
        [ -n "$cand" ] && running+=("$cand")
    done <<<"$list"
    if [ -n "$prefix" ]; then
        for cand in "$plans/$prefix"-*/; do
            [ -d "$cand" ] || continue
            cand=$(basename -- "${cand%/}")
            # `welle-x-2-<date>` matches the glob of `welle-x` too: the name decides
            [ "$(wave_name "$cand")" = "$prefix" ] && hits+=("$cand")
        done
        [ ${#hits[@]} -gt 0 ] \
            || die "new: no wave directory plans/$prefix-<date> for the prefix $prefix/ -- pass --wave <directory>"
        if [ ${#hits[@]} -gt 1 ]; then
            for cand in "${hits[@]}"; do
                case " ${running[*]} " in *" $cand "*) live+=("$cand") ;; esac
            done
            [ ${#live[@]} -eq 1 ] && hits=("${live[@]}")
        fi
        [ ${#hits[@]} -eq 1 ] \
            || die "new: the prefix $prefix/ names ${#hits[@]} waves (${hits[*]}) -- pass --wave <directory>"
        echo "strand: wave ${hits[0]} (from the prefix $prefix/)" >&2
        printf '%s\n' "${hits[0]}"
        return 0
    fi
    [ ${#running[@]} -gt 1 ] || return 0
    for cand in "${running[@]}"; do
        [ -f "$plans/$cand/straenge/$name.json" ] && owners+=("$cand")
    done
    if [ ${#owners[@]} -eq 1 ]; then
        echo "strand: wave ${owners[0]} (its strand file plans/${owners[0]}/straenge/$name.json)" >&2
        printf '%s\n' "${owners[0]}"
        return 0
    fi
    die "plans/README.md section Live names ${#running[@]} running waves (${running[*]})"\
        "-- pass --wave <directory> or name the strand <wave>/$name"
}

# `<dir>\t<text>` for every row of the section Live table of a plans README.
live_rows() {
    # SC2016: the backticks in the sed pattern are markdown table syntax,
    # not a command substitution -- single quotes are exactly right here.
    # shellcheck disable=SC2016
    awk '/^## Live/ { live = 1; next }
         live && /^## / { exit }
         live' "$1" \
        | sed -n 's#^| *`\([^`]*\)/` *| *\(.*\)#\1\t\2#p'
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
    local name="" issues=() plan="" section="" base="master" wave_in="" force=0 graphify=0
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
            --graphify) graphify=1; shift ;;
            *) die "new: unknown argument: $1" ;;
        esac
    done
    [ ${#issues[@]} -gt 0 ] || die "new needs at least one --issue"
    # GH #1226: `new <wave>/<name>` -- the prefix is the wave of the branch.
    local prefix=""
    case "$name" in
        */*/*) die "new: a strand name carries at most one prefix (<wave>/<name>): $name" ;;
        */*) prefix=${name%%/*}; name=${name#*/}
             { [ -n "$prefix" ] && [ -n "$name" ]; } || die "new: <wave>/<name> needs both parts" ;;
    esac

    local root plans wdir wave branch tree report basis issue_list
    root=$(main_root) || exit 2
    plans="$root/plans"
    if [ -n "$wave_in" ]; then
        [ -z "$prefix" ] || [ "$(wave_name "${wave_in%/}")" = "$prefix" ] \
            || die "new: the prefix $prefix/ is not the wave $wave_in -- drop one of them"
        wdir=$(wave_dir "$plans" "$wave_in") || exit 2
    else
        wdir=$(new_wave_hint "$plans" "$prefix" "$name") || exit 2
        [ -n "$wdir" ] || wdir=$(wave_dir "$plans" "") || exit 2
    fi
    wave=$(wave_name "$wdir")
    branch="$wave/$name"
    tree="$(dirname -- "$root")/$(basename -- "$root")-wt-$name"
    # #1148: report and strand file go into the strand's WORKTREE -- the
    # strand commits them on its branch; the main tree is never written.
    report="$tree/plans/$wdir/berichte/$name.md"

    if [ "$force" = 0 ]; then
        [ -e "$plans/$wdir/berichte/$name.md" ] \
            && die "$name: the report already exists ($plans/$wdir/berichte/$name.md)"
        [ -e "$tree" ] && die "$name: the worktree already exists ($tree)"
        git -C "$root" rev-parse --verify --quiet "$branch" >/dev/null \
            && die "$name: the branch already exists ($branch)"
    fi

    basis=$(git -C "$root" rev-parse --short=8 "$base" 2>/dev/null) \
        || die "no such base: $base"
    # GH #1167 (g): the base is the master tip. Another base carries no commit
    # that is not the owner's (author AND committer) -- the merge would refuse
    # it after the gate (`foreign_commits`); here it costs nothing.
    if [ "$base" != "master" ]; then
        local foreign
        foreign=$("${KIT_PY[@]}" "$(kit_dir)/pipeline/turnaround.py" foreign "$root" "master..$base" 2>&1) \
            || die "base $base carries commits not by the owner -- start from master: $(printf '%s' "$foreign" | tr '\n' ';')"
    fi

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
        printf '## Versionsart\n\n(kind je Template in plans/%s/straenge/%s.json: fix|feature|break; fehlt es → fix; der Merge vergibt die Nummern)\n\n' "$wdir" "$name"
        printf '## Gate\n\n(die Summenzeile steht im Kopfblock)\n\n'
        printf '## Offene Punkte\n\n'
        # The field merge asks for (GH #1196, merge.OUTSOURCED), in the one
        # form it accepts (K13): `none`, or `#<n> (owner-ruling <reference>)`
        # items the strand writes in its place. The old placeholder
        # `<none | ...>` was copied as `<none>` and refused at the merge point.
        printf 'Outsourced findings: none\n\n'
        printf '## Strang-Rulings\n\n'
        # The section `merge` asks for (Z-I 2c (2)); fix rounds may also
        # write `## Fix-Runde Review <sha>`. Empty it is refused: merge wants
        # a verdict line under it (review Z-I gap I1, merge.REVIEW_VERDICT).
        printf '## Review\n\n(the reviewer adds the verdict line with the review file; merge asks for a yes)\n'
    } >"$report"

    # The strand file: the issue, the title of the merge commit and the KIND
    # of change per template (fix|feature|break). A strand never bumps a
    # version; `merge` reads the kinds and assigns the numbers from the master
    # it merges into (dev rules section 4). An existing file is kept -- the
    # orchestrator may have written the kinds before the strand was set up;
    # its copy in the main tree is carried into the worktree (#1148).
    local sfile="$tree/plans/$wdir/straenge/$name.json"
    local sfile_main="$plans/$wdir/straenge/$name.json"
    if [ ! -e "$sfile" ] && [ -f "$sfile_main" ]; then
        mkdir -p "$(dirname -- "$sfile")"
        cp -- "$sfile_main" "$sfile" || die "could not copy the strand file $sfile_main"
    fi
    if [ ! -e "$sfile" ]; then
        mkdir -p "$(dirname -- "$sfile")"
        MECLAW_S_FILE="$sfile" MECLAW_S_NAME="$name" "${KIT_PY[@]}" - "${issues[@]}" <<'PY' \
            || die "could not write the strand file $sfile"
import json, os, sys

nums = [int(i) if i.isdigit() else i for i in sys.argv[1:]]
doc = {"strand": os.environ["MECLAW_S_NAME"],
       "issue": nums[0] if len(nums) == 1 else nums,
       "title": "", "kind": {}}
with open(os.environ["MECLAW_S_FILE"], "w", encoding="utf-8") as fh:
    json.dump(doc, fh, indent=2, ensure_ascii=False)
    fh.write("\n")
PY
    fi

    # The dispatch prompt. It carries the order and the paths, and for the
    # rules it carries a POINTER -- see WHY THIS EXISTS above.
    local order="(Auftrag hier einsetzen)"
    # The contracts file only in a wave that has one (C-M4).
    local orient="dein Plan-Teil"
    [ -f "$plans/$wdir/plan-parts/V0-vertraege.md" ] \
        && orient="\`plan-parts/V0-vertraege.md\`, dein Plan-Teil"
    if [ -n "$plan" ]; then
        local found
        found=$(plan_section "$plan" "${section:-$name}")
        [ -n "$found" ] && order="$found"
    fi
    cat <<PROMPT
Du bist ein Opus-Bauer des Strangs "$name" der Welle $wave.

Regeln: plans/PREAMBLE.md (gilt wörtlich) und plans/$wdir/BUILD-PREAMBLE.md
(das Wellen-Spezifische). Beide zuerst lesen; sie werden hier nicht wiederholt.

Orientierung: $orient, die dort genannten
Dateien (Koepfe zuerst); nie \`grep -r\` ueber \`crates/\` ohne Pfadfilter.

Worktree: $tree
Branch:   $branch (Basis $basis)
Issues:   $issue_list
Bericht:  plans/$wdir/berichte/$name.md (Kopfblock steht schon drin)
Strang:   plans/$wdir/straenge/$name.json (kind je Template: fix|feature|break -- kein Bump im Strang)

Auftrag:
$order

Gate und Bericht laufen über das Kit:
  scripts/strand.sh gate            -- fährt das Gate einmal, blockt, druckt die Summenzeile
  scripts/strand.sh report          -- füllt den Kopfblock aus dem Archiv
PROMPT
    # `--graphify` (W6, OR-DT-18): the trial's one line -- questions across the
    # tree go to graphify's read-only commands against the main tree. No read
    # hooks, no MCP; the command is whatever the host names (never a path here).
    if [ "$graphify" = 1 ]; then
        printf '\nQuestions across the tree: %s query|path|explain '"'"'<question>'"'"' (read-only, main tree state; no read hooks, no MCP).\n' \
            "${MECLAW_GRAPHIFY_CMD:-graphify}"
    fi
}

# --- gate -------------------------------------------------------------------

# THE strand of a call (GH #1199 f): `strand_of_branch [<--strand value>]`
# -- `--strand` > the branch `<wave>/<name>` -> `<name>` > a refusal on stderr
# that names the way, return 1. A detached tree has no branch: S2 of wave
# validation 2 made one by hand to bind its token (OR-V2.S2.2). The Python
# twin is `pipeline/gitenv.py strand_of`, with the same two texts and the same
# name: `--strand <wave>/<S>` is `<S>` (re-check f+g: the value passed on
# whole archived under `receipts/<wave>/<S>`, `pipeline.py test` under `<S>`).
strand_of_branch() {
    local branch
    if [ -n "${1:-}" ]; then
        printf '%s\n' "${1##*/}"
        return 0
    fi
    branch=$(git rev-parse --abbrev-ref HEAD 2>/dev/null) || branch=""
    case "$branch" in
        */*)  printf '%s\n' "${branch##*/}" ;;
        HEAD) echo "strand: detached HEAD: pass --strand <S>" >&2; return 1 ;;
        *)    echo "strand: cannot tell the strand from the branch -- pass --strand" >&2; return 1 ;;
    esac
}

# A detached HEAD: no branch names the strand or its wave.
detached_head() {
    [ "$(git rev-parse --abbrev-ref HEAD 2>/dev/null)" = HEAD ]
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
# without opening it. `open_run_dir <archive-root>` sets RUN_DIR.
RUN_DIR=""
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
    # `rm` first: `ln -sfn` onto an existing SYMLINK TO A DIRECTORY would
    # otherwise put the new link inside the old target.
    rm -f "$archive_root/latest" 2>/dev/null
    ln -s "$run_id" "$archive_root/latest" 2>/dev/null \
        || printf '%s\n' "$run_id" >"$archive_root/latest.txt"
}

# Red, asking and aborted stations of a run log -- the lines a report needs
# (ABORTED: a watchdog stopped the station, C-M2).
red_lines() { grep -E '^GATE [^ ]+ \[.*\] [0-9]+s (RED|ASK|ABORTED)' "$1" || true; }

# GH #1199 (kit find V2-A8): a wrapper keeps one stream -- `> log` keeps
# stdout, a bare background start may keep stderr. A line for the caller goes
# onto both, once when fd 1 and fd 2 are one file (`> log 2>&1`).
one_stream() { [ /dev/stdout -ef /dev/stderr ]; }
say_both() {
    local text
    text=$(cat)
    printf '%s\n' "$text"
    one_stream || printf '%s\n' "$text" >&2
}

# The runner's output into the temp log `$1` as it comes (a tee), and each
# `GATE <station>` line onto the caller's streams at once: before, the caller
# saw nothing until the pass ended (re-check v2-fix-e I1, a stdout-only log
# held no station line while a 20-30 min pass ran).
gate_tee() {
    local ln both=1
    one_stream && both=0
    while IFS= read -r ln || [ -n "$ln" ]; do
        printf '%s\n' "$ln" >&3
        case "$ln" in
            "GATE "*)
                printf '%s\n' "$ln"
                if [ "$both" = 1 ]; then printf '%s\n' "$ln" >&2; fi ;;
        esac
    done 3>"$1"
}

# --- the lanes of the host file (THE LANES) ------------------------------------

# The ssh target of a lane in the host file, or nothing.
lane_target() {
    local file
    file=$(lanes_file)
    [ -f "$file" ] || return 0
    awk -v n="$1" '{ sub(/#.*/, "") } $1 == n && NF >= 2 { print $2; exit }' "$file"
}

# The width cap of a lane in the host file, or nothing: a `threads=<n>` word
# after the ssh target (`<name> <ssh-target> [<bmc>] [threads=<n>]`). A lane
# whose scenario tests time out at full width (a 24-thread build host, measured
# 2026-10-06) runs with n test threads and n build jobs instead of `nproc`.
# Anything but digits is ignored, so a typo costs the cap and not the gate.
lane_threads() {
    local file
    file=$(lanes_file)
    [ -f "$file" ] || return 0
    awk -v n="$1" '{ sub(/#.*/, "") } $1 == n && NF >= 2 {
        for (i = 3; i <= NF; i++) if ($i ~ /^threads=[1-9][0-9]*$/) { sub(/^threads=/, "", $i); print $i; exit }
        exit }' "$file"
}

# A `key=value` word of a lane in the host file (`class=`, `slots=`, `jobs=`,
# `watch=`, `sccache=`, `linker=`, `root=`), or nothing.
lane_kv() {   # <lane> <key>
    local file
    file=$(lanes_file)
    [ -f "$file" ] || return 0
    awk -v n="$1" -v k="$2" '{ sub(/#.*/, "") } $1 == n && NF >= 2 {
        for (i = 3; i <= NF; i++) if (index($i, k "=") == 1) { print substr($i, length(k) + 2); exit }
        exit }' "$file"
}

# The root of a `local` lane: `root=<path>`, or the third word when it is a
# path (`<name> local <root> class=egon ...`).
lane_local_root() {   # <lane>
    local file r
    r=$(lane_kv "$1" root)
    if [ -z "$r" ]; then
        file=$(lanes_file)
        [ -f "$file" ] || return 0
        r=$(awk -v n="$1" '{ sub(/#.*/, "") } $1 == n && NF >= 3 && $3 ~ /^\// { print $3; exit }' "$file")
    fi
    printf '%s' "$r"
}

# The settings of a lane for `scripts/lane_sync.sh`, exported into this
# process: every later call of the transport for this lane sees them.
lane_settings() {   # <lane>
    local lane="$1" target root
    target=$(lane_target "$lane")
    export MECLAW_LANE_NAME="$lane"
    export MECLAW_LANE_THREADS; MECLAW_LANE_THREADS=$(lane_threads "$lane")
    export MECLAW_LANE_CLASS; MECLAW_LANE_CLASS=$(lane_kv "$lane" class)
    export MECLAW_LANE_SLOTS; MECLAW_LANE_SLOTS=$(lane_kv "$lane" slots)
    export MECLAW_LANE_JOBS; MECLAW_LANE_JOBS=$(lane_kv "$lane" jobs)
    export MECLAW_LANE_WATCH; MECLAW_LANE_WATCH=$(lane_kv "$lane" watch)
    export MECLAW_LANE_SCCACHE; MECLAW_LANE_SCCACHE=$(lane_kv "$lane" sccache)
    export MECLAW_LANE_LINKER; MECLAW_LANE_LINKER=$(lane_kv "$lane" linker)
    if [ "$target" = local ]; then
        root=$(lane_local_root "$lane")
        [ -n "$root" ] || die "the local lane $lane names no root (<name> local <root> ... in $(lanes_file))"
        export MECLAW_LANE_ROOT="$root"
    fi
}

cmd_gate_legacy() {
    local mode="strand" strand_in="" wave_in="" args=() have_log_dir=0 plan_only=0 host=""
    local no_pre=0 pre_reason=""
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
            --no-pre)    no_pre=1; shift ;;
            --reason)    need_value "$1" "$#"; pre_reason="$2"; shift 2 ;;
            *)           args+=("$1"); shift ;;
        esac
    done
    [ "$no_pre" = 0 ] || [ -n "$pre_reason" ] || die "gate: --no-pre needs --reason \"<text>\""

    local root plans wdir name archive_root archive runlog gate rc summary

    # The gate of THIS tree, not of the main one and not of the tree the kit
    # was called from: a strand gates the sources it is standing in. The wave
    # that changes `gate.sh` is exactly the wave in which the difference shows.
    gate="$(git rev-parse --show-toplevel 2>/dev/null)/scripts/gate.sh"
    [ -x "$gate" ] \
        || gate="$KIT_DIR/gate.sh"
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
    name=$(strand_of_branch "$strand_in") || exit 2

    archive_root="$plans/$wdir/receipts/$name"

    # THE PRE STAMP (OR-DT-23, W4): a strand gate from a strand worktree (a
    # branch with a `/`) runs only on a tree `strand.sh pre` graded GREEN in
    # the last 24 h. Passes in the main tree and `merge` check no stamp.
    # `--no-pre --reason` gets past it, and the receipt says so.
    if [ "$mode" = strand ] && [ "$no_pre" = 0 ] && pre_applies; then
        local pre_py
        pre_py="$(git rev-parse --show-toplevel)/scripts/pipeline/pre.py"
        if [ -f "$pre_py" ]; then
            "${KIT_PY[@]}" "$pre_py" --stamp-check || return 2
        fi
    fi

    # A gate on a lane is the pipeline's (`strand|integration|release
    # --host`); this runner gates here only (GH #1156).
    [ -z "$host" ] || die "gate $mode --host: a lane runs the pipeline's modes, not this runner"

    # The cargo token, BEFORE anything is written: a refused gate leaves no
    # run directory behind. The heartbeat names this shell, which blocks for
    # the whole run, so a gate longer than the TTL keeps its token. The exit
    # of the check goes on as it is, like in `test-tier.sh`: 3 is no lease,
    # any other a store that somebody has to look at (review M4).
    ! store_armed || cmd_token_store check --pid "$$" || return $?

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
    ! store_armed || cmd_token_store check --pid 0 >/dev/null 2>&1 || true

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
    [ "$no_pre" = 0 ] || record_pre_skipped "$archive" "$pre_reason"
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

# --- the pre-run (W4, GH #1118) ---------------------------------------------------

# Does the stamp apply here? A strand worktree: a branch with a `/`, in a
# linked worktree (the main tree's passes never need a stamp).
pre_applies() {
    local branch gd common
    branch=$(git symbolic-ref --short -q HEAD 2>/dev/null) || return 1
    case "$branch" in */*) ;; *) return 1 ;; esac
    gd=$(git rev-parse --git-dir 2>/dev/null) || return 1
    common=$(git rev-parse --git-common-dir 2>/dev/null) || return 1
    [ "$(cd -- "$gd" && pwd -P)" != "$(cd -- "$common" && pwd -P)" ]
}

# `--no-pre --reason`: the decision goes into the receipt beside the owner's
# other decisions, so the retro sees every gate that ran without a pre-run.
record_pre_skipped() {   # dir reason
    local dir="$1" reason="$2"
    [ -n "$dir" ] && [ -d "$dir" ] || return 0
    echo "gate: pre skipped -- $reason"
    "${KIT_PY[@]}" - "$dir" "$reason" <<'PY'
import json, os, sys, datetime
d, reason = sys.argv[1], sys.argv[2]
entry = {"station": "pre", "decision": "pre_skipped", "reason": reason,
         "at": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")}
path = os.path.join(d, "receipt.json")
try:
    with open(path, encoding="utf-8") as fh:
        doc = json.load(fh)
except (OSError, ValueError):
    doc = None
if isinstance(doc, dict):
    doc.setdefault("owner_decisions", []).append(entry)
    doc["pre_skipped"] = reason
    with open(path, "w", encoding="utf-8") as fh:
        json.dump(doc, fh, indent=2)
        fh.write("\n")
else:
    with open(os.path.join(d, "pre_skipped.json"), "w", encoding="utf-8") as fh:
        json.dump(entry, fh, indent=2)
        fh.write("\n")
PY
}

cmd_pre() {
    local host="" held="" no_lane=0 allow="" target="" holder="" strand_in="" wave_in="" args=()
    while [ $# -gt 0 ]; do
        case "$1" in
            -h|--help) usage; return 0 ;;
            --host)       need_value "$1" "$#"; host="$2"; shift 2 ;;
            --holder)     need_value "$1" "$#"; holder="$2"; shift 2
                          # fail closed (re-check Z-A M-b): "" took the main
                          # lease, a bad suffix lost the rc 2 of `token lane`.
                          [[ "$holder" =~ ^[A-Za-z0-9_-]{1,32}$ ]] \
                              || die "pre: --holder takes a short suffix ([A-Za-z0-9_-], 1-32), got '$holder'" ;;
            --strand)     need_value "$1" "$#"; strand_in="$2"; shift 2 ;;
            --wave)       need_value "$1" "$#"; wave_in="$2"; shift 2 ;;
            --no-lane)    no_lane=1; shift ;;
            --fast)       no_lane=1; args+=(--fast); shift ;;
            --allow-bump) need_value "$1" "$#"; allow="$2"; shift 2 ;;
            *) die "pre: unknown option $1" ;;
        esac
    done
    local top name
    top=$(git rev-parse --show-toplevel 2>/dev/null) || die "pre: not in a git tree"
    name=$(strand_of_branch "$strand_in" 2>/dev/null) || name=""
    args+=(--repo "$top")
    # The lease key stays the caller's: `--strand <wave>/<S>` binds `<wave>/<S>`.
    [ -n "$name" ] && args+=(--strand "${strand_in:-$name}")
    [ -n "$allow" ] && args+=(--allow-bump "$allow")
    if [ "$no_lane" = 1 ]; then
        args+=(--no-lane)
    else
        # A held lane and an explicit --host must agree (kit find (a)): the
        # gate's admission refuses the mismatch, so pre refuses it first.
        # `--holder <n>`: the lane of the strand's second lease (GH #1146, Review Z-A2 R3).
        held=$(cmd_token lane ${strand_in:+--strand "$strand_in"} ${wave_in:+--wave "$wave_in"} \
            ${holder:+--holder "$holder"} 2>/dev/null) || held=""
        [ -z "$host" ] || [ -z "$held" ] || [ "$host" = "$held" ] \
            || die "pre: ${name:-this strand} holds lane $held, not $host -- the gate would refuse" \
                "--host $host; pre on $held, or release and take $host"
        [ -n "$host" ] || host="$held"
        if [ -n "$host" ]; then
            target=$(lane_target "$host")
            [ -n "$target" ] || die "pre: no lane $host in the host file $(lanes_file)"
            args+=(--lane "$host" --lane-target "$target")
            # pre calls the transport itself (`lane_sync.sh push|check|test`):
            # the lane's settings and the cargo build path, as for a gate.
            lane_settings "$host"
            export MECLAW_LANE_CARGO=1
        fi
    fi
    "${KIT_PY[@]}" "$top/scripts/pipeline/pre.py" "${args[@]}"
}

# --- report and close -------------------------------------------------------

# Both steps read the same three things -- the report, the gate archive and
# the git history of the tree the strand built in -- so they share the setup.
# `strand_paths <strand-in> <wave-in>` sets REPORT and ARCHIVE.
strand_paths() {
    local strand_in="$1" wave_in="$2" root plans wdir name
    # The strand first (GH #1199 f): a detached tree hears the way out.
    name=$(strand_of_branch "$strand_in") || exit 2
    root=$(main_root) || exit 2
    plans="$root/plans"
    wdir=$(wave_dir "$plans" "$wave_in") || exit 2
    REPORT="$plans/$wdir/berichte/$name.md"
    S_REL="plans/$wdir/berichte/$name.md"
    ARCHIVE=$(latest_run "$plans/$wdir/receipts/$name")
    # Called from another tree (`--strand`): the report `new` wrote into the
    # strand's worktree (#1148) is the one -- also over a stale copy in the
    # main tree, which is never written while the strand has a worktree
    # (GH #1167 e: an untracked copy there stops the merge's fast-forward).
    local wt
    if wt=$(strand_worktree "$root" "$(wave_name "$wdir")/$name") \
            && [ -f "$wt/$S_REL" ]; then
        REPORT="$wt/$S_REL"
    fi
    [ -f "$REPORT" ] || [ -f "$(git rev-parse --show-toplevel 2>/dev/null)/$S_REL" ] \
        || die "no report at $REPORT"
}

# The worktree that has <branch> checked out, or exit 1.
strand_worktree() {
    git -C "$1" worktree list --porcelain 2>/dev/null | awk -v want="branch refs/heads/$2" '
        /^worktree / { path = substr($0, 10) }
        $0 == want { print path; found = 1; exit }
        END { exit !found }'
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
    # E-DT-W0.a: the report lives in the strand's worktree -- from there that
    # copy is the one filled, never the main tree's (strands copied files into
    # the main tree to feed `report`, and stalled on the permission dialog).
    local top
    top=$(git rev-parse --show-toplevel 2>/dev/null) || top=""
    # The main tree is no strand tree: from there `strand_paths` chose (GH #1167 e).
    [ -n "$top" ] && [ "$top" = "$(main_root)" ] && top=""
    # Only the strand's OWN tree (re-check H2 m8): a copy another strand's
    # tree carries (a rebase brought it along) is not its report.
    if [ -n "$top" ] && [ -f "$top/$S_REL" ] \
            && [ "$(strand_of_branch 2>/dev/null)" = "$(basename -- "$S_REL" .md)" ]; then
        REPORT="$top/$S_REL"
    fi
    # Convergence A (owner, 13.10.): a strand reports done only on a head
    # `strand.sh pre` graded GREEN -- the done-definition's (a), checked here
    # instead of trusted. The report's own tree decides: a strand worktree (the
    # branch has a `/`, a linked worktree) needs a fresh stamp that still
    # covers its code (`pre.py --stamp-check`, as the strand gate asks); a
    # report in the main tree (after the merge) has no stamp to ask for.
    local rtree
    rtree=$(git -C "$(dirname -- "$REPORT")" rev-parse --show-toplevel 2>/dev/null) || rtree=""
    if [ -n "$rtree" ] && [ -f "$rtree/scripts/pipeline/pre.py" ] \
            && (cd -- "$rtree" && pre_applies); then
        # GH #1199 R1.2: the merge's `lifted_bans` (merge.lifted_problem, the
        # same code as pre's check `lifted-bans`) -- a `lifted` declaration in
        # the strand file is a doc change the pre stamp survives.
        "${KIT_PY[@]}" "$rtree/scripts/pipeline/pre.py" --repo "$rtree" --lifted-check \
            --strand "$(basename -- "$S_REL" .md)" --wave "$(basename -- "$(dirname -- "$(dirname -- "$S_REL")")")" \
            || die "report: the merge would refuse lifted_bans (the line above) -- fix the hits, then pre and report"
        "${KIT_PY[@]}" "$rtree/scripts/pipeline/pre.py" --repo "$rtree" --stamp-check >/dev/null \
            || die "report: $rtree has no fresh pre-GREEN stamp on its head -- run strand.sh pre first"
    fi
    MECLAW_S_REPORT="$REPORT" MECLAW_S_ARCHIVE="$ARCHIVE" MECLAW_S_ROOT="$(main_root)" \
        MECLAW_S_KIT="$KIT_DIR" MECLAW_S_WAVE="$(basename -- "$(dirname -- "$(dirname -- "$S_REL")")")" \
        "${KIT_PY[@]}" - <<'PY'
import json, os, re, subprocess, sys

report = os.environ["MECLAW_S_REPORT"]
archive = os.environ["MECLAW_S_ARCHIVE"]
sys.path.insert(0, os.environ["MECLAW_S_KIT"])
from pipeline import merge  # noqa: E402 -- the merge point's own reading of the line
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
# The verdict is the LAST `GATE-SUMMARY` line, never the last line: the lane
# gate writes `GATE-LANE ...` after it (W3), and the station lines before it.
# A file without one fails closed: the header gets a non-GREEN gate first,
# so an earlier GREEN cannot stay behind for `close`.
# The run is the one the merge point reads (review v2-r3-kit M1): the newest
# STRAND receipt (`merge.strand_receipts`), not the newest run of any mode --
# a later release or integration run in the archive is not the strand's gate.
# No strand run in the archive is no gate (review v2-r3-kit M5): a GREEN `gate
# ci` or a receipt that names no mode stood in the head block while the merge
# point refuses it (`no_strand_receipt`).
# An archive from before the per-run layout stays as it is.
no_strand_run = None
if os.path.basename(archive) == "latest" or re.match(r"\d{8}T\d{6}Z-", os.path.basename(archive)):
    sys.path.insert(0, os.path.dirname(os.environ["MECLAW_S_KIT"]))
    from scripts.pipeline import merge
    runs = merge.strand_receipts(os.path.dirname(archive))
    if runs:
        archive = runs[0][0]
    else:
        no_strand_run = os.path.dirname(archive)
summary = os.path.join(archive, "summary.txt")
no_summary = False
not_head = None


def receipt_rev(line):
    """`(rev, dirty)` of the gate run the line comes from: the run's receipt
    (`receipt.json`, or the `last-strand.json` copy), else -- an archive from
    before the per-run layout -- the rev of the summary line itself."""
    for fn in ("last-strand.json", "receipt.json"):
        try:
            with open(os.path.join(archive, fn), encoding="utf-8") as fh:
                doc = json.load(fh)
        except (OSError, ValueError):
            continue
        if not isinstance(doc, dict) or doc.get("mode", doc.get("stage")) != "strand":
            continue   # the merge point's filter (`merge.strand_receipts`)
        if doc.get("rev"):
            return doc["rev"], doc.get("dirty") is True
    words = line.split()
    return (words[2] if len(words) > 2 else ""), False


def covers_head(line):
    """GH #1227: why the gate line does NOT stand for the head, or None. The
    gate ran with the change still uncommitted: its receipt was the base, and
    `report` copied its line as the head's. The merge point's rule
    (`merge.preconditions`, `receipt_not_branch_head`): the receipt's rev is
    the head, or code-equal to it (`turnaround.code_equal_revs`: only the
    strand's own report and strand files moved since); and the run saw no
    uncommitted change (`dirty: true` in the receipt)."""
    rev, dirty = receipt_rev(line)
    head_sha = git("rev-parse", "%s^{commit}" % tip)
    gated = git("rev-parse", "--verify", "--quiet", "%s^{commit}" % rev) if rev else ""
    why = []
    if gated != head_sha:
        sys.path.insert(0, os.path.join(os.environ["MECLAW_S_KIT"], "pipeline"))
        import turnaround
        same, bad = (turnaround.code_equal_revs(os.environ["MECLAW_S_ROOT"], gated, head_sha,
                                                os.environ["MECLAW_S_WAVE"])
                     if gated and head_sha else (False, {"?": "no such rev %s" % (rev or "-")}))
        if not same:
            why.append("gate receipt on %s, the head is %s; code changed since: %s"
                       % ((gated or rev or "?")[:9], head_sha[:9], turnaround.describe(bad)))
    if dirty:
        why.append("the gate ran on a dirty tree (uncommitted changes, receipt dirty: true)")
    return "; ".join(why) or None


if no_strand_run:
    head["gate"] = '"no strand gate"'
elif os.path.isfile(summary):
    lines = [ln.strip() for ln in open(summary) if ln.strip()]
    sums = [ln for ln in lines if ln.startswith("GATE-SUMMARY ")]
    if sums:
        # only a GREEN line is a candidate: RED/ASK keep their own refusal below
        not_head = covers_head(sums[-1]) if sums[-1].endswith(" GREEN") else None
        head["gate"] = ('"%s"' % sums[-1]) if not_head is None else '"gate not on the head"'
    elif lines:
        head["gate"] = '"no GATE-SUMMARY"'
        no_summary = True

# K13 (16.10.): the `Outsourced findings:` line in the form the merge point
# accepts (`merge.outsourced_findings_ok`): the placeholder `<none>` is
# written as `none`; any other value the merge cannot read stays and the
# report refuses below, with merge's words. Re-check v2-r3-lease m4: a
# MISSING line is a forgotten answer (#1196) -- refused below, never added.
found = list(merge.OUTSOURCED.finditer(body))
if found:
    last = found[-1]
    if re.fullmatch(r"<\s*none\s*>", last.group(1), re.I):
        line = last.group(0)
        body = (body[:last.start()]
                + line[:line.index(last.group(1), line.index(":"))] + "none"
                + body[last.end(1):])

seen, keys = set(), []
for k in FIELDS + order:
    if k not in seen:
        seen.add(k)
        keys.append(k)
open(report, "w").write(
    "---\n" + "\n".join("%s: %s" % (k, head[k]) for k in keys) + "\n---\n" + body)

if no_strand_run:
    sys.exit("strand: no strand gate in %s -- its runs are of other modes (ci, integration, "
             "release) or name none; run `strand.sh gate` on the head, then report" % no_strand_run)
if no_summary:
    sys.exit("strand: %s has no GATE-SUMMARY line -- run the gate again"
             % summary)
if not_head:
    sys.exit("strand: the gate line does not stand for the head: %s -- run the gate on "
             "the committed head, then report" % not_head)
if not found:
    sys.exit("strand: %s has no line `Outsourced findings:` -- write `Outsourced findings: none` "
             "under `## Offene Punkte`, or `#<n> (owner-ruling <reference>)` per issue a finding "
             "moved to; report never adds it (GH #1196: a missing line is a forgotten answer)"
             % report)
if not merge.outsourced_findings_ok(body):
    sys.exit("strand: %s: the line `Outsourced findings:` is neither `Outsourced findings: none` "
             "nor `Outsourced findings: #<n> (owner-ruling <reference>)` -- the merge point "
             "refuses it (no_outsourced_findings)" % report)

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
    # block, whichever tree this runs in. Only an armed source holds one: a
    # store whose pool is off has nothing to give back (C-M3).
    if store_armed; then
        branch=$(awk 'NR == 1 && /^---$/ { head = 1; next }
                      head && /^---$/ { exit }
                      head && sub(/^branch: */, "") { print; exit }' "$REPORT")
        # A release that leaves the lease standing (exit 3: a live run, or a
        # stale-but-busy note) is passed on, not thrown away -- the report is
        # written, but the lane is still held (re-check tv-kit fix11 M1).
        if [ -n "$branch" ]; then
            cmd_token release --strand "$branch"
            rc=$?
            if [ "$rc" != 0 ]; then
                echo "strand: the report is written, but the lease of $branch stays" \
                     "(token release exit $rc) -- see the line above" >&2
                return "$rc"
            fi
        fi
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
    # The comment is built by `pipeline.py close` (GH #1107), the same code the
    # merge closes with. It reads machine facts only: the commit list comes from
    # git -- `<merge>^1..<tip>` once the strand is merged, `merge-base..<branch>`
    # before -- not from the head block, so the commits of other strands that a
    # rebase or a merge of master brought in are never listed under this issue;
    # the gate line is the one of the NEWEST STRAND receipt of the archive; and
    # a head block that names commits reachable from master before the branch
    # point is refused (run `report` again) instead of closing with them.
    # The copy `report` fills (review Z-A R3 M2): the strand tree's, when it
    # stands there -- else close read the main tree's and refused it.
    local top
    top=$(git rev-parse --show-toplevel 2>/dev/null) || top=""
    if [ -n "$top" ] && [ -f "$top/$S_REL" ]; then
        REPORT="$top/$S_REL"
    fi
    local args=(close --report "$REPORT")
    [ "$do_it" = 1 ] && args+=(--do)
    "${KIT_PY[@]}" "$(kit_dir)/pipeline.py" "${args[@]}"
}

# --- merge and revert -------------------------------------------------------

# Both live in `scripts/pipeline/` (merge.py, revert.py); the kit only hands
# the arguments through and passes the exit on unchanged: 0 merged, 1 refused,
# 2 usage, 3 queued behind the `tree:master` lease of another merge.
cmd_merge() {
    # GH #1199 (a): `gate` and `report` read the strand from the branch
    # `<wave>/<S>`, and `merge` in the very same tree ended with "merge: name
    # the strand" (wave validation 2, strand v2-p1078). The same function
    # names it here; the main tree is no strand tree (as for `report`), so
    # there the merge still asks for `--strand`/`--batch`.
    local arg named=0 name top
    for arg in "$@"; do
        case "$arg" in --strand|--strand=*|--batch|--batch=*|-h|--help) named=1 ;; esac
    done
    if [ "$named" = 0 ] && top=$(git rev-parse --show-toplevel 2>/dev/null) \
            && [ "$top" != "$(main_root 2>/dev/null)" ] \
            && name=$(strand_of_branch 2>/dev/null); then
        set -- --strand "$name" "$@"
    fi
    "${KIT_PY[@]}" "$(kit_dir)/pipeline.py" merge "$@"
}

cmd_revert() {
    "${KIT_PY[@]}" "$(kit_dir)/pipeline.py" revert "$@"
}

# --- token ------------------------------------------------------------------

# The token file of this host: next to the cargo lock, so every test that
# already points `MECLAW_GATE_LOCK` at a throw-away path runs UNARMED without
# a word about tokens.
token_file() {
    local lock="${MECLAW_GATE_LOCK:-/tmp/meclaw-w26-cargo.lock}"
    printf '%s\n' "${MECLAW_STRAND_TOKENS:-${lock%.lock}.tokens}"
}

# The private host file of the build lanes: `<name> <ssh-target> [<bmc>] [threads=<n>]` per
# line, `#` a comment. Never in the repository -- it names machines.
lanes_file() {
    printf '%s\n' "${MECLAW_LANES_FILE:-$HOME/.config/meclaw/lanes}"
}

# The directory of this script -- `lane_sync.sh` and `pipeline/hygiene.py`
# beside it belong to the same tree.
kit_dir() {
    printf '%s\n' "$KIT_DIR"
}

# Why a lane of the host file is never switched off by idle-off, or nothing:
# the owner's machine (`class=egon`) and a lane that is this machine (target
# `local`) stay on whatever the timer thinks.
lane_idle_skip() {
    local file
    file=$(lanes_file)
    [ -f "$file" ] || return 0
    awk -v n="$1" '{ sub(/#.*/, "") } $1 == n && NF >= 2 {
        if ($2 == "local") { print "transport local"; exit }
        for (i = 3; i <= NF; i++) {
            if ($i == "class=egon") { print "class egon"; exit }
            if ($i == "transport=local") { print "transport local"; exit } }
        exit }' "$file"
}

# `idle_arm <lane> <hours>`: (re-)arm the idle-off timer of one lane here.
#
# The timer asks a holder command before it powers a host off, and powers
# it off when the command's output does not name the lane. `token who` as it
# stands names EVERY armed lane (its `lanes:` line) and nothing at all when it
# fails -- the first would keep every host on forever, the second would switch
# off a host that a strand holds. So the holder command is `token who`
# narrowed to its holder lines (`held <lane>`), and a `token who` that fails
# prints `held <lane>`: fail closed, the host stays on. It runs the MAIN
# tree's kit (a strand's worktree may be gone when the timer fires) with the
# token and host file of this call, so the timer reads the queue this caller
# sees. One string, split by the script with `shlex`.
# shellcheck disable=SC2016  # expanded by the timer's bash, not here
HOLDER_SH='out=$("$0" token who 2>&1) || { echo "held $1 (token who failed: the lane counts as held)"; exit 0; }
printf "%s\n" "$out" | sed -n "s/^  [^ ].*  lane \([^ ][^ ]*\)  held .*/held \1/p"'

idle_arm() {
    local lane="$1" hours="$2" hyg main common holder
    main=$(kit_dir)
    common=$(git rev-parse --path-format=absolute --git-common-dir 2>/dev/null) \
        && [ -x "$(dirname -- "$common")/scripts/strand.sh" ] \
        && main="$(dirname -- "$common")/scripts"
    # The timer calls the script it was armed with, hours later: the main
    # tree's when it has one (a strand's worktree is gone after `strand
    # close`), else the kit's own (before the hygiene is merged).
    hyg="$main/pipeline/hygiene.py"
    [ -f "$hyg" ] || hyg="$(kit_dir)/pipeline/hygiene.py"
    [ -f "$hyg" ] || { echo "strand: no $hyg -- idle-off not armed" >&2; return 3; }
    holder=$("${KIT_PY[@]}" -c 'import shlex, sys; print(shlex.join(sys.argv[1:]))' \
        env "MECLAW_STRAND_TOKENS=$(token_file)" "MECLAW_LANES_FILE=$(lanes_file)" \
        bash -c "$HOLDER_SH" "$main/strand.sh" "$lane") || return 2
    "${KIT_PY[@]}" "$hyg" idle-off arm --lane "$lane" --after "$hours" --lanes-file "$(lanes_file)" \
        "--holder-cmd=$holder"
}

# --- lanes ------------------------------------------------------------------

# --- the delegation to the pipeline core (W3, GH #1117) ----------------------

# `gate [<mode>] [options]`: this tree's `scripts/gate.sh` (the shell in front
# of `pipeline.py gate`), with the strand, the wave and the archive root named.
# It still BLOCKS and prints the red and asking stations, the summary (and
# `GATE-LANE`) and the question block -- the rest of the run goes to `run.log`
# in the run directory the pipeline opened under the archive (`latest`).
cmd_gate() {
    # A tree older than W3 (no `gate_legacy.sh` beside its `gate.sh`) carries
    # the old runner, which knows no `--strand` -- it gates the old way (C-M1).
    local own
    own="$(git rev-parse --show-toplevel 2>/dev/null)/scripts"
    if [ "${1:-}" = ci ] \
        || { [ -x "$own/gate.sh" ] && [ ! -e "$own/gate_legacy.sh" ]; }; then
        cmd_gate_legacy "$@"
        return $?
    fi
    local mode="strand" strand_in="" wave_in="" args=() have_archive=0 plan_only=0 modes=() m
    if [ $# -ge 1 ]; then
        case "$1" in strand|integration|release) modes+=("$1"); shift ;; esac
    fi
    while [ $# -gt 0 ]; do
        case "$1" in
            -h|--help)
                usage
                echo "gate options pass through to scripts/pipeline.py gate -- see scripts/gate.sh --help"
                return 0 ;;
            # `--mode <m>` is the positional form (#1199 K13); a value flag's
            # value is never read as one (`--reason --mode`)
            --mode)      need_value "$1" "$#"; modes+=("$2"); shift 2 ;;
            --mode=*)    modes+=("${1#--mode=}"); shift ;;
            --base|--only|--decide|--host|--reason|--repo|--slots|--holder|--cache)
                         need_value "$1" "$#"; args+=("$1" "$2"); shift 2 ;;
            --strand)    need_value "$1" "$#"; strand_in="$2"; shift 2 ;;
            --wave)      need_value "$1" "$#"; wave_in="$2"; shift 2 ;;
            --archive|--log-dir) have_archive=1; args+=("$1"); shift ;;
            --plan-only) plan_only=1; args+=("$1"); shift ;;
            *)           args+=("$1"); shift ;;
        esac
    done
    for m in ${modes[@]+"${modes[@]}"}; do
        case "$m" in strand|integration|release) ;;
            *) die "gate: unknown mode: $m (expected strand|integration|release)" ;; esac
        [ "$m" = "${modes[0]}" ] \
            || die "gate: two modes: ${modes[0]} and $m (--mode <m> and the positional <m> name the same one)"
        mode="$m"
    done

    local gate root plans wdir name archive_root log rc summary run
    gate="$(git rev-parse --show-toplevel 2>/dev/null)/scripts/gate.sh"
    [ -x "$gate" ] \
        || gate="$KIT_DIR/gate.sh"
    [ -x "$gate" ] || die "no gate runner at $gate"

    # A plan runs nothing: it goes to the caller as the pipeline prints it --
    # with the wave the run would name (re-check m-N3), else the plan of an
    # integration differs from its run. A named wave that does not exist stops
    # here as in the run; no wave found plans without one (full suite).
    if [ "$plan_only" = 1 ]; then
        local pwdir=""
        if [ -n "$wave_in" ]; then
            root=$(main_root) || exit 2
            pwdir=$(wave_dir "$root/plans" "$wave_in") || exit 2
        elif root=$(main_root 2>/dev/null); then
            pwdir=$(wave_dir "$root/plans" "" 2>/dev/null) || pwdir=""
        fi
        if [ -n "$pwdir" ]; then
            MECLAW_GATE_WAVE="${MECLAW_GATE_WAVE:-$pwdir}" \
                "$gate" "$mode" --wave "$(wave_name "$pwdir")" ${args[@]+"${args[@]}"}
        else
            "$gate" "$mode" ${args[@]+"${args[@]}"}
        fi
        return $?
    fi

    # The strand first (GH #1199 f): a detached tree hears the way out.
    name=$(strand_of_branch "$strand_in") || exit 2
    root=$(main_root) || exit 2
    plans="$root/plans"
    wdir=$(wave_dir "$plans" "$wave_in") || exit 2
    archive_root="$plans/$wdir/receipts/$name"
    [ "$have_archive" = 1 ] || args+=(--archive "$archive_root")

    log=$(mktemp) || die "no temp file"
    # The integration narrows from the start of this wave: the directory with
    # its date (re-check m-N2), as MECLAW_GATE_WAVE -- `--wave` keeps the name
    # without it (lease holder, schedule). A wave set by hand still wins.
    MECLAW_GATE_WAVE="${MECLAW_GATE_WAVE:-$wdir}" \
        "$gate" "$mode" --strand "$name" --wave "$(wave_name "$wdir")" \
        ${args[@]+"${args[@]}"} 2>&1 | gate_tee "$log"
    rc=${PIPESTATUS[0]}
    red_lines "$log"
    summary=$(grep -E '^GATE-SUMMARY ' "$log" | tail -1)
    if [ -z "$summary" ]; then
        # GH #1199: a refusal of the runner (pre stamp, admission, lane)
        # reaches a stdout log too, not only stderr.
        { echo "strand: the gate wrote no summary line -- last lines of its output:"
          tail -20 "$log"; } | say_both
        rm -f "$log"
        return "$rc"
    fi
    if [ "$have_archive" = 0 ]; then
        run=$(latest_run "$archive_root")
        if [ -d "$run" ] && [ "$run" != "$archive_root" ]; then
            [ -e "$run/run.log" ] || cp "$log" "$run/run.log"
            [ -e "$run/summary.txt" ] || printf '%s\n' "$summary" >"$run/summary.txt"
        fi
    fi
    printf '%s\n' "$summary"
    grep -E '^GATE-LANE ' "$log" | tail -1
    sed -n '/^gate: ASK/,$p' "$log"
    rm -f "$log"
    return "$rc"
}

# `test '<filterset>' [options]`: `pipeline.py test`, every argument as given.
cmd_test() {
    case "${1:-}" in -h|--help) usage; return 0 ;; --unittest) shift; cmd_test_unittest "$@"; return $? ;; esac
    # A detached tree (GH #1199 f): the wave of `--strand <name>` comes from
    # the context as for `gate`/`token` (`wave_dir`); every word goes on as given.
    local prev="" arg strand_in="" wave_in="" root wave extra=()
    for arg in "$@"; do
        [ "$arg" = -- ] && break    # nextest's own words follow
        case "$prev" in --strand) strand_in="$arg" ;; --wave) wave_in="$arg" ;; esac
        prev="$arg"
    done
    if [ -n "$strand_in" ] && [ -z "$wave_in" ] && [ "${strand_in#*/}" = "$strand_in" ] \
            && detached_head; then
        root=$(main_root) || exit 2
        wave=$(wave_dir "$root/plans" "") || exit 2
        extra=(--wave "$wave")
    fi
    "${KIT_PY[@]}" "$(pipeline_py)" test ${extra[@]+"${extra[@]}"} "$@"
}

# `test --unittest <module|test id|file.py> ...`: the kit's Python unittests
# (kit gap KX, wave validation 2). `python3 -I -m unittest` finds `scripts`
# only with the tree's root as cwd, and an agent shell cannot keep a `cd` -- a
# wrapper was a manual step. The run starts at the root of this tree (`-C`)
# through `selftest_shard.py unittest`, the gate-selftest station's runner,
# on this host: no lane, no token, no store. A file is taken from the cwd and
# becomes its dotted module. Every name must hold a test: one that loads none
# (a script-style `test_*.py` with `main()`, a namespace package's module)
# would vanish in a shard of the others and leave the line GREEN (re-check
# RC-K3 I2), so it is named (`no test in <name>`) and the run is RED. One
# log; the line `TEST [unittest <args>] <passed>/<ran> (skipped <k>) <secs>s
# GREEN|RED` -- a skipped test is neither passed nor failed --, then
# `log: <path>`; RED adds the log's tail. Exit 0 GREEN, 1 RED (also a name
# without a test), 2 a usage error, 128 + signal when the runner was stopped.
cmd_test_unittest() {
    local top arg rel mods=() log rc start secs ran failed skipped tail empty sumline counted shardlogs
    [ $# -ge 1 ] || die "test --unittest needs a module, a test id or a file"
    top=$(git rev-parse --show-toplevel 2>/dev/null) || die "test --unittest: not in a git tree"
    for arg in "$@"; do
        case "$arg" in
            -*) die "test --unittest: unknown option $arg (modules, test ids or files)" ;;
            *.py)
                [ -f "$arg" ] || die "test --unittest: no such file: $arg"
                rel=$(realpath --relative-to="$top" -- "$arg") || die "test --unittest: cannot resolve $arg"
                case "$rel" in ../*) die "test --unittest: $arg is outside the tree $top" ;; esac
                rel=${rel%.py}
                mods+=("${rel//\//.}") ;;
            *) mods+=("$arg") ;;
        esac
    done
    log="${TMPDIR:-/tmp}/strand-unittest/${top##*/}-$(date +%Y%m%dT%H%M%S)-$$.log"
    mkdir -p -- "${log%/*}" || die "test --unittest: cannot create ${log%/*}"
    start=$SECONDS
    (cd -- "$top" && "${KIT_PY[@]}" "$KIT_DIR/pipeline/selftest_shard.py" unittest "${mods[@]}") \
        >"$log" 2>&1
    rc=$?
    secs=$((SECONDS - start))
    # The names that load no test, from the same root as the run (a name
    # that does not load at all is left to the run, which reports it red).
    empty=$("${KIT_PY[@]}" - "$top" "${mods[@]}" <<'PY'
import os, sys, unittest
os.chdir(sys.argv[1])
sys.path.insert(0, sys.argv[1])
loader = unittest.TestLoader()
for name in sys.argv[2:]:
    try:
        if loader.loadTestsFromName(name).countTestCases() == 0:
            print(name)
    except Exception:
        pass
PY
    )
    ran=$(sed -n 's/^Ran \([0-9]*\) tests\{0,1\} in .*/\1/p' "$log" | tail -n 1)
    # failed = failures + errors + unexpected successes of the last summary
    sumline=$(sed -n 's/^\(OK\|FAILED\)\( (.*)\)\{0,1\}$/&/p' "$log" | tail -n 1)
    failed=$(printf '%s\n' "$sumline" | grep -oE '(failures|errors|unexpected successes)=[0-9]+' \
        | awk -F= '{s += $2} END {print s + 0}')
    # skipped: from the shards' own logs when the runner split the run (its
    # combined summary carries no skip count), else from this log
    shardlogs=("${log%.log}".shard-*.log)
    [ -e "${shardlogs[0]}" ] || shardlogs=("$log")
    skipped=$(sed -n 's/^\(OK\|FAILED\) (.*skipped=\([0-9]*\).*)$/\2/p' "${shardlogs[@]}" \
        | awk '{s += $1} END {print s + 0}')
    if [ -n "$empty" ]; then
        while IFS= read -r counted; do
            printf 'no test in %s (a script-style file is not a unittest module)\n' "$counted"
        done <<<"$empty" | tee -a "$log"
        [ "$rc" = 0 ] && rc=1
    fi
    [ "$rc" = 5 ] && rc=1    # unittest's "no tests ran" is a red run here
    tail=$( [ "$rc" = 0 ] || tail -n 20 "$log")
    [ -n "$tail" ] && printf '%s\n' "$tail"
    printf 'TEST [unittest %s] %s/%s (skipped %s) %ss %s\n' "${mods[*]}" \
        "$(( ${ran:-0} - failed - skipped ))" "${ran:-0}" "$skipped" \
        "$secs" "$( [ "$rc" = 0 ] && echo GREEN || echo RED)" | tee -a "$log"
    printf 'log: %s\n' "$log"
    return "$rc"
}

# `token <verb> [options]`: `pipeline.py lanes <verb> [options]`. The one thing
# resolved here is `--strand <name> --wave <wave dir>`: the wave directory is
# looked up under plans/ as before and goes on as `--strand <wave>/<name>`.
# The caller (`MECLAW_LANES_CALLER`) is this kit's process, like the old
# heartbeat's.
cmd_token() {
    cmd_token_store "$@"
}

cmd_token_store() {
    local args=() strand_in="" wave_in="" root wave
    while [ $# -gt 0 ]; do
        case "$1" in
            --strand) need_value "$1" "$#"; strand_in="$2"; shift 2 ;;
            --wave)   need_value "$1" "$#"; wave_in="$2"; shift 2 ;;
            *)        args+=("$1"); shift ;;
        esac
    done
    # A detached tree (GH #1199 f) has no branch to name the wave of
    # `--strand <name>`: it comes from the context as for `gate` (`wave_dir`).
    if [ -n "$strand_in" ] && [ "${strand_in#*/}" = "$strand_in" ] \
            && { [ -n "$wave_in" ] || detached_head; }; then
        root=$(main_root) || exit 2
        wave=$(wave_dir "$root/plans" "$wave_in") || exit 2
        strand_in="$(wave_name "$wave")/$strand_in"
        wave_in=""
    fi
    [ -n "$strand_in" ] && args+=(--strand "$strand_in")
    [ -n "$wave_in" ] && args+=(--wave "$wave_in")
    MECLAW_LANES_CALLER="$$" "${KIT_PY[@]}" "$(pipeline_py)" lanes ${args[@]+"${args[@]}"}
}

# `lanes status [<lane>]`: `pipeline.py lanes status` (one line per lane of
# the host file, or of the one named; the store learns which lanes answer).
# The free space of a lane comes from here, over the host file's target --
# never from `ssh <alias> df`, whose name known_hosts may not know (GH #1144).
cmd_lanes() {
    # `idle-off` is the host's own timer (W2), no lane state of the store.
    case "${1:-}" in
        status)   shift; [ $# -le 1 ] || die "lanes status: at most one lane name" ;;
        idle-off) shift; cmd_lanes_idle_off "$@"; return $? ;;
        # an orphaned run (its driver ended without close): fail-closed, Z-I #1126
        release)  shift; "${KIT_PY[@]}" "$(pipeline_py)" lanes release-run "$@"; return $? ;;
        # the browser packages of a lane (GH #1161): the transport with the
        # lane's ssh target from the host file
        browsers) shift; cmd_lanes_browsers "$@"; return $? ;;
        # the lane's fingerprint now, into the store (Re-Check S3 Teil 2 m-2)
        fingerprint) shift; MECLAW_LANES_CALLER="$$" "${KIT_PY[@]}" "$(pipeline_py)" lanes fingerprint "$@"
                     return $? ;;
        *) die "lanes: expected status|idle-off|release|browsers|fingerprint" ;;
    esac
    "${KIT_PY[@]}" "$(pipeline_py)" lanes status "$@"
}

# `lanes browsers <lane> check|install [<engine>...]`: `lane_sync.sh browsers`
# against the lane's ssh target; an unknown lane is refused before any call.
# `install` runs apt as root there: never on the owner's machine (`class=egon`)
# or on this one (`local`) -- the lanes idle-off skips too (lane_idle_skip);
# `check` only reads. The class travels on, lane_sync.sh refuses a second time.
cmd_lanes_browsers() {
    local lane="${1:-}" target why
    [ -n "$lane" ] || die "lanes browsers <lane> check|install [<engine>...]"
    shift
    target=$(lane_target "$lane")
    [ -n "$target" ] || die "lanes browsers: no lane $lane in the host file $(lanes_file)"
    if [ "${1:-}" = install ]; then
        why=$(lane_idle_skip "$lane")
        [ -z "$why" ] || die "lanes browsers: never installs on $lane ($why) -- packages there are the owner's call"
    fi
    local rc=0
    MECLAW_LANE_CLASS=$(lane_kv "$lane" class) "$(kit_dir)/lane_sync.sh" browsers "$target" "$@" || rc=$?
    # The store learns the lane's fingerprint again, real start included: a lane
    # refused for an engine stayed refused after `install` until some other run
    # wrote a new one (Re-Check S3 Teil 2 m-2). Its words go to stderr; the exit
    # code stays the recipe's.
    MECLAW_LANES_CALLER="$$" "${KIT_PY[@]}" "$(pipeline_py)" lanes fingerprint "$lane" >&2 || true
    return "$rc"
}

# `lanes idle-off [--after <h>] [--lane <name>]`: arm the idle-off timer of
# every lane of the host file (or the one named), skipping `class=egon` and
# `local`. One line per lane; exit 1 when one could not be armed. Arming
# again replaces the lane's timer (`hygiene.py idle-off arm` stops the old
# one first).
cmd_lanes_idle_off() {
    local after=2 only="" file name target rest why rc=0 seen=0 arc
    # The default is the gates' own (`MECLAW_IDLE_OFF_H`) when that arms at
    # all -- `0` switches the re-arm after a gate off, not this command.
    if [[ "${MECLAW_IDLE_OFF_H:-}" =~ ^[0-9]+([.][0-9]+)?$ ]] \
            && ! [[ "$MECLAW_IDLE_OFF_H" =~ ^0+([.]0+)?$ ]]; then
        after="$MECLAW_IDLE_OFF_H"
    fi
    while [ $# -gt 0 ]; do
        case "$1" in
            --after) need_value "$1" "$#"; after="$2"; shift 2 ;;
            --lane)  need_value "$1" "$#"; only="$2"; shift 2 ;;
            *) die "lanes idle-off: unknown argument: $1" ;;
        esac
    done
    [[ "$after" =~ ^[0-9]+([.][0-9]+)?$ ]] || die "lanes idle-off: --after wants hours, got '$after'"
    [[ "$after" =~ ^0+([.]0+)?$ ]] \
        && die "lanes idle-off: --after 0 arms nothing -- disarm with systemctl --user stop pipe-idle-<lane>.timer"
    file=$(lanes_file)
    [ -f "$file" ] || die "no lane host file at $file"
    [ -f "$(kit_dir)/pipeline/hygiene.py" ] || die "no lane hygiene at $(kit_dir)/pipeline/hygiene.py"
    while read -r name target rest <&3; do
        case "$name" in ''|'#'*) continue ;; esac
        [ -n "$target" ] || continue
        case "$target" in '#'*) continue ;; esac
        [ -z "$only" ] || [ "$name" = "$only" ] || continue
        seen=1
        why=$(lane_idle_skip "$name")
        if [ -n "$why" ]; then
            printf '%s  skipped (%s)\n' "$name" "$why"
            continue
        fi
        idle_arm "$name" "$after" >/dev/null
        arc=$?
        if [ "$arc" = 0 ]; then
            printf '%s  idle-off in %s h unless a strand holds it\n' "$name" "$after"
        else
            printf '%s  NOT armed (exit %s)\n' "$name" "$arc"
            rc=1
        fi
    done 3<"$file"
    [ "$seen" = 1 ] || die "lanes idle-off: no lane ${only:-at all} in the host file $file"
    return "$rc"
}

# --- bench --------------------------------------------------------------------

# `bench new|collect|list` (THE LANE HYGIENE): `hygiene.py bench` on the lane
# (`--lane`, the script over ssh like every hygiene call) or here. `new`
# prints the directory as its last stdout line -- the `CARGO_TARGET_DIR` of
# the bench build.
cmd_bench() {
    local verb="${1:-}" name="" ttl="" lane="" hyg target sync rc args=()
    [ $# -ge 1 ] && shift
    case "$verb" in
        new|collect|list) ;;
        *) die "bench: expected new|collect|list" ;;
    esac
    if [ "$verb" != list ]; then
        [ $# -ge 1 ] || die "bench $verb: expected a name"
        name="$1"; shift
        [[ "$name" =~ ^[A-Za-z0-9][A-Za-z0-9._-]*$ ]] \
            || die "bench $verb: a name is letters, digits, '.', '_' and '-', got '$name'"
    fi
    while [ $# -gt 0 ]; do
        case "$1" in
            --ttl)  [ "$verb" = new ] || die "bench $verb: --ttl belongs to 'bench new'"
                    need_value "$1" "$#"; ttl="$2"; shift 2 ;;
            --lane) need_value "$1" "$#"; lane="$2"; shift 2 ;;
            *) die "bench $verb: unknown argument: $1" ;;
        esac
    done
    if [ "$verb" = new ]; then
        [ -n "$ttl" ] || die "bench new: --ttl <hours> is required -- a bench without an end is the leftover of tomorrow"
        [[ "$ttl" =~ ^[0-9]+([.][0-9]+)?$ ]] || die "bench new: --ttl wants hours, got '$ttl'"
    fi
    args=(bench "$verb")
    [ -z "$name" ] || args+=("$name")
    [ -z "$ttl" ] || args+=(--ttl "$ttl")
    if [ -z "$lane" ]; then
        hyg="$(kit_dir)/pipeline/hygiene.py"
        [ -f "$hyg" ] || die "no lane hygiene at $hyg"
        "${KIT_PY[@]}" "$hyg" "${args[@]}" --root "${MECLAW_LANE_ROOT:-/srv/target}"
        return $?
    fi
    target=$(lane_target "$lane")
    [ -n "$target" ] || die "no lane $lane in the host file $(lanes_file)"
    sync="$(kit_dir)/lane_sync.sh"
    [ -x "$sync" ] || die "no lane transport at $sync"
    # `new` leaves a timer on the host, and a timer needs a script path: the
    # transport installs the script under the lane root first (`--kit`).
    if [ "$verb" = new ]; then
        "$sync" hygiene --kit "$target" "${args[@]}"
    else
        "$sync" hygiene "$target" "${args[@]}"
    fi
    rc=$?
    case "$rc" in
        255) echo "strand: lane $lane unreachable" >&2; return 2 ;;
        3)   return 2 ;;
    esac
    return "$rc"
}

# --- wave close (W11) --------------------------------------------------------

# `wave close <wave>` and `wave open <wave>` are the pipeline core's commands;
# the kit only hands them over. `open` writes and commits in THIS tree (the
# branch it stands on), not in the tree of the kit.
cmd_wave() {
    [ $# -ge 1 ] || die "wave: expected 'wave open|close <wave> [options]'"
    case "$1" in
        close) "${KIT_PY[@]}" "$(kit_dir)/pipeline.py" wave "$@" ;;
        open)
            local top
            top=$(git rev-parse --show-toplevel 2>/dev/null) || die "wave open: not in a git tree"
            "${KIT_PY[@]}" "$(kit_dir)/pipeline.py" wave "$@" --root "$top" ;;
        *) die "wave: unknown step $1 (expected open|close)" ;;
    esac
}

# --- main -------------------------------------------------------------------

# `-C <dir>` in front of the subcommand: a `cd <dir>` first, as `git -C` --
# for every subcommand, a relative path from the caller's directory, several
# in a row each from the one before (GH #1199 (b), class V2-A4). An agent
# shell that resets its cwd on every call reached a worktree only through a
# wrapper script (wave validation 2, strand v2-p1078: one line per use in its
# log of manual steps). The kit stays the one this file belongs to (KIT_DIR
# above), as git stays the git that was called.
while [ "${1:-}" = "-C" ]; do
    need_value "$1" "$#"
    [ -d "$2" ] || die "-C: no such directory: $2"
    cd -- "$2" || die "-C: cannot enter $2"
    shift 2
done

if [ $# -lt 1 ] || [ "$1" = "-h" ] || [ "$1" = "--help" ]; then
    usage
    [ $# -lt 1 ] && exit 2
    exit 0
fi

# An inherited repository (`GIT_DIR`/`GIT_WORK_TREE`, the recipe without
# `cd`) is neutralized once, here: the tree it names becomes the working
# directory and the variables go, so no `git -C <tree>` below acts on the
# caller's tree (re-check pd-legacy M-1; `pipeline.py` does the same through
# scripts/pipeline/gitenv.py).
for _var in $(git rev-parse --local-env-vars 2>/dev/null); do
    if [ -n "${!_var+x}" ]; then
        # The repository `GIT_DIR` names, before it goes: when the tree that
        # wins is another one, say so (re-check pd-legacy m-N2).
        _named=""
        if [ -n "${GIT_DIR+x}" ]; then
            _named=$(git rev-parse --absolute-git-dir 2>/dev/null) \
                && _named=$(cd -- "$_named" 2>/dev/null && pwd -P) \
                || _named="(no repository)"
        fi
        if _top=$(git rev-parse --show-toplevel 2>/dev/null); then
            cd -- "$_top" || die "cannot enter $_top"
        fi
        # shellcheck disable=SC2046 # one name per word
        unset $(git rev-parse --local-env-vars)
        if [ -n "$_named" ]; then
            _won=$(git rev-parse --absolute-git-dir 2>/dev/null) \
                && _won=$(cd -- "$_won" 2>/dev/null && pwd -P) \
                || _won="(no repository)"
            [ "$_named" = "$_won" ] \
                || echo "strand: NOTE inherited GIT_DIR names $_named, working in $_won ($PWD)" >&2
        fi
        break
    fi
done

sub="$1"; shift
case "$sub" in
    new)    cmd_new "$@" ;;
    pre)    cmd_pre "$@" ;;
    gate)   cmd_gate "$@" ;;
    test)   cmd_test "$@" ;;
    report) cmd_report "$@" ;;
    close)  cmd_close "$@" ;;
    merge)  cmd_merge "$@" ;;
    revert) cmd_revert "$@" ;;
    token)  cmd_token "$@" ;;
    lanes)  cmd_lanes "$@" ;;
    bench)  cmd_bench "$@" ;;
    wave)   cmd_wave "$@" ;;
    *) die "unknown subcommand: $sub (expected new|pre|gate|test|report|close|merge|revert|token|lanes|bench|wave)" ;;
esac
}; exit $?
