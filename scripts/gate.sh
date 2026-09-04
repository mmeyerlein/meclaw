#!/usr/bin/env bash
# meclaw -- THE gate. One entry point for every gate chain in this repo.
#
# WHY THIS EXISTS
# ===============
# There used to be three chains: a strand gate, a pre-push gate and the CI
# workflow, each with its own hard-coded list of steps. They drifted, they
# overlapped, and every one of them paid for work the diff did not ask for --
# a Python-only change still compiled the workspace. This runner replaces all
# three. It asks ONE resolver (`scripts/gate_plan.py`) which stations a diff
# needs, runs them, and reports each one on a single line.
#
#     scripts/gate.sh <strand|integration|release|ci> [options]
#
#     --base REF     base of the diff (see MODES below)
#     --only s1,s2   run exactly these stations
#     --fail-fast    stop after the first RED (default: run everything)
#     --plan-only    print the plan and a final `tests=true|false`, run nothing
#     --log-dir DIR  additionally copy receipt and logs into DIR
#     --no-nice      do not wrap cargo stations in nice/ionice, and do not
#                    cap the build width
#     --resync       force a full touch of the integration test sources
#
# MODES
# =====
#   strand       during a strand, before its commit. Base: --base, else
#                `git merge-base master HEAD`; if that is HEAD itself, the
#                diff is the dirty tree, or HEAD~1 when the tree is clean.
#   integration  the once-per-wave pass over everything the wave changed.
#   release      integration plus advisories, export selftest, export audit.
#   ci           what the workflow runs. No disk/nice/flock hygiene (the
#                runner is alone on that machine), no tree-sync and no stamp
#                (the checkout is fresh, target/ starts empty, so there are no
#                ghost binaries to chase), and no `tests` station: CI runs the
#                suite in shards. Base: --base (the push event's `before`);
#                an unknown or all-zero base means full scope.
#
# For integration and release the base is the master commit the public tree
# was last rebuilt from -- read from the `github-main` ref's commit subject
# ("export: public tree rebuilt from <sha>").
#
# OUTPUT
# ======
#   GATE <station> [<scope>] <secs>s <GREEN|RED|SKIP|NOTE> [reason]
#   GATE-SUMMARY <mode> <rev> <green>/<total> <secs>s <GREEN|RED>
#
# Verdicts: GREEN the station passed. RED it failed. SKIP it could not run
# (missing tool, or planned elsewhere) -- never a failure. NOTE a finding to
# read, not a judgement on the commit (advisories, tree-sync).
#
# `<green>/<total>` counts the stations that were JUDGED: GREEN and RED only.
# SKIP and NOTE appear in neither half -- they are on their own GATE line and
# in the receipt, but a run with three notes is not a run with three failures.
#
# It runs each station ONCE and it does NOT restart the chain because one
# finding was fixed -- fix everything it lists, then run it again as a whole
# (docs/development-rules.md § 7).
#
# ARTEFACTS
# =========
#   <gate>/<mode>-<rev>.json         the receipt, rewritten after each station
#   <gate>/last-<mode>.json          a copy of the newest receipt
#   <gate>/logs/<mode>-<station>.log
#   <target>/.gate-tree              which tree filled <target> last, written
#                                    only after a cargo station came back green
#
# `<gate>` is `<target>/gate/<tree>`, NOT `<target>/gate`: <tree> is the
# basename of the worktree the runner was started from. Several worktrees share
# one target directory (see below) and every strand of a wave starts at the SAME
# master commit, so `<mode>-<rev>.json` and `logs/<mode>-<station>.log` named the
# very same files in all of them -- two runs overwrote each other's station logs
# mid-run, and the main tree's `strand-tests.log` came back holding another
# worktree's compile output and a nextest error that was not its own (measured
# 2026-09-04). Everything a run WRITES lives under the per-tree directory: the
# receipt, its `last-<mode>.json` copy, the logs, and what `--log-dir` copies
# out. The tree STAMP stays shared on purpose -- it answers "which tree filled
# <target> last", a question about the shared directory, not about one run.
#
# WHICH <target>, and why it is not simply `./target`: every worktree of this
# repository shares ONE target directory. A private one per worktree costs a
# full cold build each and fills the disk -- four of them once took the volume
# to 94 %, and a linked worktree that built its own also had no `target/debug/
# meclaw` for the scenario suites to run against (measured 2026-09-04). So an
# explicit CARGO_TARGET_DIR always wins; otherwise a LINKED worktree uses the
# main worktree's `target/` and exports CARGO_TARGET_DIR so every cargo station
# builds there too. The main worktree keeps `./target`. The runner prints the
# directory it settled on as its first line.
#
# THE CARGO LOCK IS HELD FOR THE WHOLE RUN, NOT PER STATION
# =========================================================
# One shared target/ means one build at a time. While a run took the lock only
# around each cargo command, another worktree built the same workspace members
# from ITS sources in the gaps between the stations; cargo then handed this run
# its own older sources back as fresh and linked against the foreign rlib.
# Measured 2026-09-04, release run 4 with five strand gates of the wave running
# in parallel: `error[E0063]: missing base_path` in `meclaw_core::
# TransferBounds` -- a field that exists in one worktree of the wave only.
#
# So the lock is taken ONCE, immediately before the first `tree_sync` (that is,
# before the first cargo station), and released when the run ends. It spans the
# non-cargo stations in between as well, because they use the artefacts the
# cargo stations produced: `scenarios:*` run `target/debug/meclaw`, so does
# `recall-harness`, and `export-audit` builds in a target of its own but reads
# this run's receipt. Stations BEFORE the first cargo one (the anchor gates,
# corpus, shellcheck, gate-selftest) never touch target/ and stay unlocked, as
# does every `ci` run and every `--plan-only` run.
#
# THE PRICE IS A QUEUE: parallel strands wait for each other instead of handing
# each other foreign rlibs. `deny-advisories` once sat 2895 s in that queue.
# That is the cost, and it is cheaper than a gate grading a build it did not
# produce.
#
# `scripts/test-tier.sh` takes the same lock when it runs on its own, so the
# runner tells it not to: `MECLAW_CARGO_LOCK_HELD=1` is exported with the lock.
#
# TEST HOOKS (part of the interface, used by scripts/tests/test_gate_sh.py)
# ========================================================================
#   MECLAW_GATE_PLAN=<file.tsv>  use this plan instead of calling the resolver
#   MECLAW_GATE_DRY=1            log every command, run none of them
#   MECLAW_GATE_LOCK=<path>      the cargo lock file, read by this runner AND
#                                by scripts/test-tier.sh. The tests point it at
#                                a throw-away path so a real build on this host
#                                cannot block the test suite.
#   MECLAW_GATE_MIN_FREE_G=<n>   the disk floor in GB (default 60). The tests
#                                set it to 0: a full disk must not turn the
#                                self-test station red with a message about
#                                a build it never runs.
#
# Exit 0 = no station is RED.

set -uo pipefail

usage() {
    # The header block above IS the help text: everything from the line after
    # the shebang up to the first line that is not a comment.
    awk 'NR > 1 && /^#/ { sub(/^# ?/, ""); print; next } NR > 1 { exit }' \
        "${BASH_SOURCE[0]}"
}

# --- arguments --------------------------------------------------------------
mode=""; base=""; only=""; fail_fast=0; plan_only=0; log_dir=""
no_nice=0; resync=0

if [ $# -ge 1 ] && { [ "$1" = "-h" ] || [ "$1" = "--help" ]; }; then
    usage
    exit 0
fi

if [ $# -lt 1 ]; then
    usage >&2
    exit 2
fi

mode="$1"; shift
case "$mode" in
    strand|integration|release|ci) ;;
    *) echo "gate: unknown mode: $mode (expected strand|integration|release|ci)" >&2
       exit 2 ;;
esac

# `shift 2` on a one-element argv leaves $# at 0 in bash but errors under
# `set -e` shells and, worse, `--base` with nothing after it used to hand the
# resolver an EMPTY base -- which reads as "diff against nothing" and made the
# runner sit in `git diff` forever. An option that wants a value says so.
need_value() {
    [ "$2" -ge 2 ] || { echo "gate: $1 needs a value" >&2; exit 2; }
}

while [ $# -gt 0 ]; do
    case "$1" in
        --base)      need_value "$1" "$#"; base="$2"; shift 2 ;;
        --only)      need_value "$1" "$#"; only="$2"; shift 2 ;;
        --log-dir)   need_value "$1" "$#"; log_dir="$2"; shift 2 ;;
        --fail-fast) fail_fast=1; shift ;;
        --plan-only) plan_only=1; shift ;;
        --no-nice)   no_nice=1; shift ;;
        --resync)    resync=1; shift ;;
        -h|--help)   usage; exit 0 ;;
        *) echo "gate: unknown argument: $1" >&2; exit 2 ;;
    esac
done

root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root" || exit 1

# --- the target directory (see ARTEFACTS above) -----------------------------
# `--git-common-dir` is `.git` in the main worktree and an absolute path to the
# main worktree's `.git` in a linked one -- that difference IS the test.
if [ -n "${CARGO_TARGET_DIR:-}" ]; then
    target_dir="$CARGO_TARGET_DIR"
elif [ "$(git rev-parse --git-common-dir 2>/dev/null || echo .git)" != ".git" ]; then
    common=$(git rev-parse --path-format=absolute --git-common-dir 2>/dev/null || echo "")
    if [ -n "$common" ]; then
        target_dir="$(dirname -- "$common")/target"
        export CARGO_TARGET_DIR="$target_dir"
    else
        target_dir="$root/target"
    fi
else
    target_dir="$root/target"
fi
echo "gate: target = $target_dir"

# Everything this run writes is namespaced by the tree it runs in -- see
# ARTEFACTS. Worktrees have distinct directory names (the main tree is
# `meclaw-core`), which is all the identity a receipt needs.
gate_dir="$target_dir/gate/$(basename -- "$root")"
logs_dir="$gate_dir/logs"
echo "gate: receipts = $gate_dir"

# The linter is installed per-user on the maintainer machine; the CI image
# has it on PATH. Extending PATH unconditionally would shadow a system install.
if ! command -v shellcheck >/dev/null 2>&1; then
    PATH="$PATH:$HOME/.local/bin"
    export PATH
fi

# --- the diff ---------------------------------------------------------------
EMPTY_TREE=4b825dc642cb6eb9a060e54bf8d69288fbee4904

rev=$(git rev-parse HEAD 2>/dev/null || echo "")
# The full list -- tracked AND untracked. It feeds the diff and the tree stamp,
# where the question is "which files must look newer than the artefacts".
dirty_files=$(git status --porcelain --untracked-files=all 2>/dev/null | sed 's/^...//')
dirty_any=0
[ -n "$dirty_files" ] && dirty_any=1

# `dirty` in the RECEIPT is a narrower question -- "would an export of this tree
# carry something that is not committed" -- and it has two subtractions.
#
#   * Untracked files do not count. The export takes tracked blobs only, so a
#     stray scratch file proves nothing about the tree that would travel.
#   * The resolver's IGNORED list does not count. The scenario stations REWRITE
#     `workshop/evals/*/last_run.json` while the gate is running, so without
#     this every release run ended `dirty: true` -- by construction, on its own
#     artefacts -- and `make_export.py` refused (measured 2026-09-04).
#
# The tree stamp keeps the full list on purpose: a rewritten run artefact still
# has to be touched, whatever the export thinks of it.
dirty_tracked=$(git status --porcelain 2>/dev/null \
    | grep -v '^??' | sed 's/^...//' | sed '/^$/d')
gate_ignored=$(python3 "$root/scripts/gate_plan.py" --print ignored 2>/dev/null || true)
if [ -n "$dirty_tracked" ] && [ -n "$gate_ignored" ]; then
    dirty_tracked=$(printf '%s\n' "$dirty_tracked" | grep -vxF "$gate_ignored" || true)
fi
dirty=0
[ -n "$dirty_tracked" ] && dirty=1

github_main_base() {
    local subject sha
    subject=$(git log -1 --format=%s github-main 2>/dev/null || true)
    sha=$(printf '%s' "$subject" | sed -n 's/^export: public tree rebuilt from \([0-9a-f]\{7,40\}\).*$/\1/p')
    if [ -z "$sha" ]; then
        echo "gate: cannot read the export base from the github-main ref." >&2
        echo "      Its subject must look like:" >&2
        echo "        export: public tree rebuilt from 885d75a" >&2
        echo "      Pass it explicitly instead: scripts/gate.sh $mode --base <sha>" >&2
        return 1
    fi
    git rev-parse --verify --quiet "$sha^{commit}" || {
        echo "gate: github-main names $sha, which is not a commit here." >&2
        return 1
    }
}

full_scope=0
if [ -z "$base" ]; then
    case "$mode" in
        strand)
            base=$(git merge-base master HEAD 2>/dev/null || echo "")
            if [ -z "$base" ] || [ "$base" = "$rev" ]; then
                if [ "$dirty_any" = 1 ]; then
                    base="$rev"          # the diff IS the dirty tree
                else
                    base=$(git rev-parse --verify --quiet HEAD~1 || echo "$EMPTY_TREE")
                fi
            fi
            ;;
        integration|release)
            base=$(github_main_base) || exit 2
            ;;
        ci)
            full_scope=1
            ;;
    esac
fi

if [ "$mode" = ci ] && [ -n "$base" ]; then
    if [ "$base" = "${base//[^0]/}" ] || ! git rev-parse --verify --quiet "$base^{commit}" >/dev/null; then
        full_scope=1                     # first push on a branch, or a base we do not have
        base=""
    fi
fi

if [ "$full_scope" = 1 ]; then
    changed=$(git ls-files)
else
    changed=$( { git diff --name-only "$base" HEAD 2>/dev/null
                 printf '%s\n' "$dirty_files"; } | sed '/^$/d' | sort -u)
fi

# --- the plan ---------------------------------------------------------------
if [ -n "${MECLAW_GATE_PLAN:-}" ]; then
    plan_tsv=$(cat "$MECLAW_GATE_PLAN")
else
    plan_tsv=$(printf '%s\n' "$changed" \
        | python3 "$root/scripts/gate_plan.py" --mode "$mode" --files-from - --format tsv) \
        || { echo "gate: the resolver refused this diff -- see above." >&2; exit 2; }
fi

# Fold the per-command rows into stations. A station with several commands
# (corpus: regenerate, then check) is ONE gate line. Rows are matched by NAME
# across the whole plan, not only against the row before: a resolver that ever
# emits a station's commands non-adjacently must not silently get two gate
# lines under one name, with the second overwriting the first one's log.
st_names=(); st_scopes=(); st_cargo=(); st_cmds=()
while IFS=$'\t' read -r n s c cmd cwd; do
    [ -z "$n" ] && continue
    at=-1
    for j in ${st_names[@]+"${!st_names[@]}"}; do
        [ "${st_names[$j]}" = "$n" ] && at=$j && break
    done
    if [ "$at" -ge 0 ]; then
        st_cmds[at]="${st_cmds[at]}"$'\n'"$cmd"$'\t'"$cwd"
        [ "$c" = 1 ] && st_cargo[at]=1
    else
        st_names+=("$n"); st_scopes+=("$s"); st_cargo+=("$c")
        st_cmds+=("$cmd"$'\t'"$cwd")
    fi
done <<<"$plan_tsv"

# --only: an exact, order-preserving filter over the plan.
if [ -n "$only" ]; then
    IFS=',' read -r -a wanted <<<"$only"
    for w in "${wanted[@]}"; do
        found=0
        for n in ${st_names[@]+"${st_names[@]}"}; do
            [ "$n" = "$w" ] && found=1 && break
        done
        [ "$found" = 1 ] || { echo "gate: --only names an unplanned station: $w" >&2; exit 2; }
    done
    f_names=(); f_scopes=(); f_cargo=(); f_cmds=()
    for i in "${!st_names[@]}"; do
        for w in "${wanted[@]}"; do
            if [ "${st_names[$i]}" = "$w" ]; then
                f_names+=("${st_names[$i]}"); f_scopes+=("${st_scopes[$i]}")
                f_cargo+=("${st_cargo[$i]}"); f_cmds+=("${st_cmds[$i]}")
                break
            fi
        done
    done
    st_names=(${f_names[@]+"${f_names[@]}"}); st_scopes=(${f_scopes[@]+"${f_scopes[@]}"})
    st_cargo=(${f_cargo[@]+"${f_cargo[@]}"}); st_cmds=(${f_cmds[@]+"${f_cmds[@]}"})
fi

has_tests=false
has_cargo=0
for i in ${st_names[@]+"${!st_names[@]}"}; do
    [ "${st_names[$i]}" = "tests" ] && has_tests=true
    [ "${st_cargo[$i]}" = 1 ] && has_cargo=1
done

if [ "$plan_only" = 1 ]; then
    for i in ${st_names[@]+"${!st_names[@]}"}; do
        while IFS=$'\t' read -r cmd cwd; do
            printf '%s\t%s\t%s\t%s\t%s\n' \
                "${st_names[$i]}" "${st_scopes[$i]}" "${st_cargo[$i]}" "$cmd" "$cwd"
        done <<<"${st_cmds[$i]}"
    done
    echo "tests=$has_tests"
    exit 0
fi

# --- hygiene (not in ci: there the runner owns the machine) ------------------
wrap=()
lock_path=""
if [ "$mode" != ci ]; then
    if [ "$has_cargo" = 1 ]; then
        # A live colony shares this disk. A cargo target directory that runs it
        # dry takes the colony down with the build, so the gate refuses first.
        min_free=${MECLAW_GATE_MIN_FREE_G:-60}
        free_g=$(df -BG --output=avail / 2>/dev/null | tail -1 | tr -dc '0-9')
        if [ -n "$free_g" ] && [ "$free_g" -lt "$min_free" ]; then
            echo "gate: only ${free_g}G free on / -- need ${min_free}G for a cargo station." >&2
            echo "      Free some disk (delete stale target directories), then start the gate again." >&2
            exit 2
        fi
    fi
    # A live colony's watchdog reads a long CPU gap as a wedge; half the cores
    # is the ceiling while it runs, and a tmpfs TMPDIR keeps the test fsyncs
    # off the spinning parts of the machine.
    export NEXTEST_TEST_THREADS=${NEXTEST_TEST_THREADS:-4}
    export TMPDIR=/dev/shm/meclaw-tests
    mkdir -p "$TMPDIR" 2>/dev/null || true
    if [ "$no_nice" = 0 ]; then
        # A shared machine builds at half width, like it tests at half width:
        # a full-width cold build starves a colony sharing the host (measured
        # 2026-09-04). nice/ionice alone did not stop it -- the watchdog reads
        # the CPU gap, not the priority.
        half=$(( $(nproc 2>/dev/null || echo 2) / 2 ))
        [ "$half" -lt 1 ] && half=1
        export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-$half}
    fi
    [ "$no_nice" = 0 ] && wrap+=(nice -n 19 ionice -c3)
    # The cargo lock. WHEN it is taken and for how long: see the header section
    # "THE CARGO LOCK IS HELD FOR THE WHOLE RUN". WHERE it lives: on a
    # descriptor of this shell (fd 9), never in a station's argv. `flock <file>
    # <cmd>` hands the open descriptor to the command, and from there to every
    # child it leaves behind: a test that spawns `sleep 300` and outlives its
    # runner inherited the lock fd and made the next cargo station wait for it
    # (measured 2026-09-04: 209 s and 263 s for a 0.5 s cargo-deny; `lsof` on
    # the lock file named an orphaned `sleep` with PPID 1). So the subshell
    # closes fd 9 before every exec. `flock --close` does NOT do this -- it
    # closes the fd BEFORE the exec and thereby drops the lock immediately,
    # which is no lock at all.
    lock_path="${MECLAW_GATE_LOCK:-/tmp/meclaw-w26-cargo.lock}"
fi

# The lock is taken at most once per run and released when the run ends.
run_lock=0
take_run_lock() {
    [ "$run_lock" = 1 ] && return 0
    [ -z "$lock_path" ] && return 0        # ci: the runner owns the machine
    exec 9>"$lock_path" || {
        echo "gate: cannot open the cargo lock $lock_path" >&2
        exit 2
    }
    flock 9
    run_lock=1
    # scripts/test-tier.sh takes this same lock when it runs on its own. Inside
    # the gate the run already holds it, and a second flock would deadlock the
    # runner against its own child.
    export MECLAW_CARGO_LOCK_HELD=1
}
release_run_lock() {
    [ "$run_lock" = 1 ] || return 0
    run_lock=0
    unset MECLAW_CARGO_LOCK_HELD
    exec 9>&-
}

# --- artefacts (gate_dir/logs_dir were settled with the target above) --------
mkdir -p "$logs_dir"
receipt="$gate_dir/$mode-${rev:-unknown}.json"
rows_file=$(mktemp)
trap 'rm -f "$rows_file"' EXIT
: >"$rows_file"

started=$(date -Is)
t_start=$(date +%s)

# write_receipt <verdict> [final]
# Without `final` the run is still going and `finished` stays null -- a
# timestamp on a receipt that is still being written would claim the run ended.
write_receipt() {
    local finished=""
    [ "${2:-}" = final ] && finished=$(date -Is)
    MECLAW_R_MODE="$mode" MECLAW_R_REV="$rev" MECLAW_R_BASE="$base" \
    MECLAW_R_DIRTY="$dirty" MECLAW_R_STARTED="$started" \
    MECLAW_R_FINISHED="$finished" MECLAW_R_VERDICT="$1" \
    MECLAW_R_ROWS="$rows_file" MECLAW_R_OUT="$receipt" \
    python3 - <<'PY'
import json, os
rows = []
with open(os.environ["MECLAW_R_ROWS"]) as fh:
    for line in fh:
        line = line.rstrip("\n")
        if not line:
            continue
        name, scope, secs, verdict, log = line.split("\t")
        rows.append({"name": name, "scope": scope, "secs": int(secs),
                     "verdict": verdict, "log": log})
doc = {
    "mode": os.environ["MECLAW_R_MODE"],
    "rev": os.environ["MECLAW_R_REV"],
    "base": os.environ["MECLAW_R_BASE"],
    "dirty": os.environ["MECLAW_R_DIRTY"] == "1",
    "started": os.environ["MECLAW_R_STARTED"],
    "finished": os.environ["MECLAW_R_FINISHED"] or None,
    "stations": rows,
    "verdict": os.environ["MECLAW_R_VERDICT"],
}
with open(os.environ["MECLAW_R_OUT"], "w") as fh:
    json.dump(doc, fh, indent=2)
    fh.write("\n")
PY
    cp -f "$receipt" "$gate_dir/last-$mode.json"
}

# `green`/`total` count JUDGEMENTS, so only GREEN and RED are in either half.
# A NOTE is a finding to read and a SKIP is a station that ran elsewhere or had
# no tool; counting them in the denominator made a clean run report 9/12 and
# read like three failures. Every station is still on its own GATE line and in
# the receipt -- the summary just does not pretend to grade them.
green=0; total=0; red=0
report() {   # name scope secs verdict log [reason]
    local name="$1" scope="$2" secs="$3" verdict="$4" log="$5" reason="${6:-}"
    printf 'GATE %s [%s] %ss %s%s\n' \
        "$name" "$scope" "$secs" "$verdict" "${reason:+ $reason}"
    printf '%s\t%s\t%s\t%s\t%s\n' "$name" "$scope" "$secs" "$verdict" "$log" >>"$rows_file"
    case "$verdict" in
        GREEN) green=$((green + 1)); total=$((total + 1)) ;;
        RED)   red=$((red + 1));     total=$((total + 1)) ;;
    esac
    write_receipt "$([ "$red" -gt 0 ] && echo RED || echo GREEN)"
}

# --- ghost binaries ---------------------------------------------------------
# Several worktrees share one target/. cargo decides freshness by mtime and
# gives workspace members the same metadata hash in every tree, so a test
# binary built in tree A is handed back as fresh in tree B. The stamp records
# which tree filled target/ last; a mismatch means the sources that differ
# between the two trees must look newer than the artefacts.
stamp="$target_dir/.gate-tree"
tree_synced=0
tree_sync() {
    [ "$tree_synced" = 1 ] && return 0
    tree_synced=1
    # ci gets a fresh checkout with an empty target/: nothing to sync against,
    # and the NOTE line would be pure noise in every workflow log.
    [ "$mode" = ci ] && return 0
    [ -n "${MECLAW_GATE_DRY:-}" ] && return 0
    mkdir -p "$target_dir"
    local s_path s_sha s_dirty list n=0 f
    if [ "$resync" = 1 ] || [ ! -f "$stamp" ]; then
        find crates -path '*/tests/*.rs' -exec touch {} + 2>/dev/null
        if [ "$resync" = 1 ]; then
            report tree-sync "full touch: --resync" 0 NOTE ""
        else
            report tree-sync "full touch: no stamp" 0 NOTE ""
        fi
    else
        s_path=$(sed -n 1p "$stamp"); s_sha=$(sed -n 2p "$stamp"); s_dirty=$(sed -n 3p "$stamp")
        if ! git rev-parse --verify --quiet "$s_sha^{commit}" >/dev/null 2>&1; then
            # A rebase, a pruned branch or a stamp from another repository:
            # `git diff <gone-sha>` fails and the union would be just the dirty
            # files -- a silent UNDER-touch, and ghost binaries survive it. The
            # only honest answer is the full touch.
            find crates -path '*/tests/*.rs' -exec touch {} + 2>/dev/null
            report tree-sync "full touch: stale stamp ${s_sha:0:7}" 0 NOTE ""
        elif [ "$s_path" != "$root" ] || [ "$s_sha" != "$rev" ]; then
            list=$( { git diff --name-only "$s_sha" HEAD 2>/dev/null
                      printf '%s\n' "${s_dirty//,/$'\n'}"
                      printf '%s\n' "$dirty_files"; } | sed '/^$/d' | sort -u)
            while IFS= read -r f; do
                [ -n "$f" ] && [ -e "$f" ] && touch "$f" && n=$((n + 1))
            done <<<"$list"
            report tree-sync "$n files touched, target last built from $s_path@${s_sha:0:7}" \
                0 NOTE ""
        fi
    fi
}

# The stamp says which tree last TOUCHED target/ -- a red or interrupted build
# fills it with artefacts just like a green one, and those artefacts are
# exactly what the next tree would be handed as fresh. So it is written when
# the FIRST cargo command of the run is dispatched, before the call and
# regardless of what the call returns; once per run.
stamp_written=0
write_stamp() {
    [ "$mode" = ci ] && return 0
    [ "$stamp_written" = 1 ] && return 0
    stamp_written=1
    mkdir -p "$target_dir"
    printf '%s\n%s\n%s\n' "$root" "$rev" \
        "$(printf '%s' "$dirty_files" | paste -sd, -)" >"$stamp"
}

# --- run --------------------------------------------------------------------
missing_tool() {   # station -> reason, or empty when everything is there
    case "$1" in
        shellcheck)
            command -v shellcheck >/dev/null 2>&1 || echo "shellcheck not installed" ;;
        deny|deny-advisories)
            command -v cargo-deny >/dev/null 2>&1 || echo "cargo-deny not installed" ;;
    esac
}

for i in ${st_names[@]+"${!st_names[@]}"}; do
    name="${st_names[$i]}"; scope="${st_scopes[$i]}"; cargo="${st_cargo[$i]}"
    log="$logs_dir/$mode-$name.log"
    # The receipt names the log RELATIVE to the repo root when it lives under
    # it (the main worktree), and absolutely when the target is elsewhere.
    case "$log" in
        "$root"/*) log_rel="${log#"$root"/}" ;;
        *)         log_rel="$log" ;;
    esac

    if [ "$mode" = ci ] && [ "$name" = "tests" ]; then
        report "$name" "$scope" 0 SKIP "" "planned-for-shards"
        continue
    fi

    why=$(missing_tool "$name")
    if [ -n "$why" ]; then
        report "$name" "$scope" 0 SKIP "" "$why"
        continue
    fi

    # The lock covers the tree sync as well: it touches sources so that cargo
    # rebuilds them, and another tree building in between is exactly what that
    # is meant to defend against.
    if [ "$cargo" = 1 ]; then
        take_run_lock
        tree_sync
    fi

    : >"$log"
    if [ "$cargo" = 1 ] && [ "$mode" != ci ]; then
        # The conditions a build ran under belong in its log: half-width is a
        # deliberate cap, not an accident, and a slow gate is a fair question.
        printf '# cargo hygiene: CARGO_BUILD_JOBS=%s NEXTEST_TEST_THREADS=%s TMPDIR=%s\n' \
            "${CARGO_BUILD_JOBS:-<unset>}" "${NEXTEST_TEST_THREADS:-<unset>}" \
            "${TMPDIR:-<unset>}" >>"$log"
    fi
    s_start=$(date +%s)
    rc=0
    while IFS=$'\t' read -r cmd cwd; do
        [ -z "$cmd" ] && continue
        printf '$ %s\n' "$cmd" >>"$log"
        [ -n "${MECLAW_GATE_DRY:-}" ] && continue
        argv=()
        eval "argv=( $cmd )"
        # The resolver leaves `{receipt}` in the export-audit argv for us to
        # fill in -- it cannot know where the runner puts its receipt.
        for k in ${argv[@]+"${!argv[@]}"}; do
            [ "${argv[$k]}" = "{receipt}" ] && argv[k]="$receipt"
        done
        # No per-station locking any more, and no exception for test-tier.sh:
        # the run holds the lock from its first cargo station onwards, and the
        # tier script skips its own flock when MECLAW_CARGO_LOCK_HELD is set.
        if [ "$cargo" = 1 ] && [ ${#wrap[@]} -gt 0 ]; then
            argv=("${wrap[@]}" "${argv[@]}")
        fi
        # Under the lock: no other tree may build between the stamp and the
        # build it describes.
        [ "$cargo" = 1 ] && [ -z "${MECLAW_GATE_DRY:-}" ] && write_stamp
        # `exec 9>&-` FIRST: the station and everything it spawns must not hold
        # the lock descriptor -- see the comment where lock_path is set.
        # </dev/null: the command loop reads from a here-string, and a station
        # command that reads stdin would eat the REST OF ITS OWN STATION. That
        # is a station reporting GREEN over work it never did.
        ( [ "$run_lock" = 1 ] && exec 9>&-
          if [ -n "$cwd" ]; then cd "$cwd" || exit 1; fi
          "${argv[@]}" ) >>"$log" 2>&1 </dev/null
        rc=$?
        [ "$rc" -ne 0 ] && break
    done <<<"${st_cmds[$i]}"
    s_secs=$(( $(date +%s) - s_start ))

    if [ "$name" = "deny-advisories" ]; then
        # Advisories move without a commit: a finding here is about the
        # dependencies, not about this tree. It is read, not blocking.
        if [ "$rc" -eq 0 ]; then
            report "$name" "$scope" "$s_secs" NOTE "$log_rel" "clean"
        else
            report "$name" "$scope" "$s_secs" NOTE "$log_rel" "findings -- read $log_rel"
        fi
    elif [ "$rc" -eq 0 ]; then
        report "$name" "$scope" "$s_secs" GREEN "$log_rel"
    else
        report "$name" "$scope" "$s_secs" RED "$log_rel"
        tail -n 20 "$log" | sed 's/^/    | /'
        if [ "$fail_fast" = 1 ]; then
            echo "gate: --fail-fast -- stopping after $name." >&2
            break
        fi
    fi
done

# --- summary ----------------------------------------------------------------
# The end of the run is the end of the lock -- also on the --fail-fast break.
release_run_lock

secs=$(( $(date +%s) - t_start ))
if [ "$red" -gt 0 ]; then verdict=RED; else verdict=GREEN; fi
write_receipt "$verdict" final

if [ -n "$log_dir" ]; then
    # A copy that silently fails leaves the wave without the receipt it asked
    # for. It is not a gate finding, so it does not change the exit code -- but
    # nobody gets to believe the receipt is there when it is not.
    copied=1
    mkdir -p "$log_dir/logs" || copied=0
    cp -f "$receipt" "$gate_dir/last-$mode.json" "$log_dir/" || copied=0
    # A run whose every station was SKIP has no logs at all; an unmatched glob
    # would then look like a copy failure.
    if compgen -G "$logs_dir/$mode-*.log" >/dev/null; then
        cp -f "$logs_dir"/"$mode"-*.log "$log_dir/logs/" || copied=0
    fi
    [ "$copied" = 1 ] || \
        echo "gate: could not copy receipt/logs to $log_dir (the run itself stands)" >&2
fi

printf 'GATE-SUMMARY %s %s %s/%s %ss %s\n' \
    "$mode" "${rev:0:7}" "$green" "$total" "$secs" "$verdict"

if [ "$verdict" = RED ]; then
    echo "gate: RED -- fix everything listed, then run the gate again as a whole."
    exit 1
fi
exit 0
