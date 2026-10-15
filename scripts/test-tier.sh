#!/usr/bin/env bash
# meclaw -- run one test tier with cargo-nextest.
#
# WHY TIERS
# =========
# The workspace carries ~600 integration test binaries. Running all of them is
# the right thing before a release and the wrong thing before every commit, so
# the suite is cut into three tiers by cost, not by importance:
#
#   t0  Unit tests only (lib + bin targets). Seconds. Run on every commit.
#   t1  t0 plus the integration tests that are not scenario-class. Minutes.
#       Run before a merge.
#   t2  Everything, including the scenario/demo/e2e binaries. Run for a
#       release -- and paired with the gate scripts, see below.
#
#   filter '<expr>'  Not a tier: run exactly this nextest filterset. This is
#                    how `scripts/gate.sh` runs the `tests` station -- the
#                    resolver `scripts/gate_plan.py` works out from the diff
#                    which binaries have to run and hands the expression over.
#
# The `changed` tier is GONE (2026-09-04). It mapped the diff to cargo
# packages and fell back to t1 whenever nothing under crates/ moved, so a
# docs- or scripts-only change paid for half the suite. Use `scripts/gate.sh
# strand` instead: it selects per test binary, not per package.
#
# The tiers are nextest FILTERSETS, defined once, right below. The scenario
# class is NOT defined here -- it comes from the resolver, so that the runner
# and this script can never disagree about what is expensive.
#
# USAGE
# =====
#     scripts/test-tier.sh t0|t1|t2 [extra nextest args...]
#     scripts/test-tier.sh filter '<nextest filterset>' [extra nextest args...]
#     scripts/test-tier.sh perf [extra nextest args...]   (the ignored perf_* tests, profile perf)
#
# EXIT: nextest's own, with one addition: a `filter` run in which no test ran
# -- a summary of `0 tests run`, no summary at all, nextest's "no tests to
# run", or a filterset nextest refused because an operator matched nothing --
# exits 6 with `no tests matched`. Zero tests prove nothing; a gate station
# and `scripts/strand.sh test` grade it RED. A refused filterset says so in
# its own line (`no tests matched (filterset refused)`): nothing was looked
# at, so the gate's cache cut (GH #1167 i) never reads it as an empty rest.
#
#     MECLAW_TIER_LIST=1          `filter` only: LIST the selection with the very
#                                 same arguments instead of running it, and end
#                                 with `test-tier: LISTED <n>` (exit 0) -- the
#                                 gate's proof that a remainder is empty.
#
#     MECLAW_TIER_PROFILE=<name>  nextest profile (default: default)
#     MECLAW_TEST_CPU_QUOTA=<n>   the quota mark (meclaw_testing::deadline::
#                                 CPU_QUOTA_MARK_VAR), set by `lane_sync.sh`
#                                 inside the CPU-quota scope only: the run
#                                 takes the profile `quota` instead of
#                                 `default` -- the one longer nextest leash of
#                                 a quota run, measured there (K8, OR-T17-15).
#                                 Every other run keeps its profile.
#     CI=<anything>               CI mode: no nice/ionice, no cargo lock, no
#                                 TMPDIR override -- the runner owns the box.
#     MECLAW_CARGO_LOCK_HELD=1    the CALLER already holds the cargo lock; do
#                                 not take it a second time. `scripts/gate.sh`
#                                 sets this: since 2026-09-04 it holds the lock
#                                 for the whole run, not per station, and a
#                                 second flock here would deadlock the gate
#                                 against its own child.
#     MECLAW_GATE_LOCK=<path>     the cargo lock file (default
#                                 /tmp/meclaw-w26-cargo.lock). The same switch
#                                 the runner reads, so both serialise on one
#                                 file -- and the tests can point both at a
#                                 throw-away one.
#     MECLAW_STRAND_TOKENS=<path> the cargo token file of a wave (default: the
#                                 lock path with `.tokens` for `.lock`). Armed
#                                 by `scripts/strand.sh token init`; while it
#                                 exists, a run from a strand branch
#                                 (`<wave>/<name>`) needs a token and exits 3
#                                 without one. Not checked in CI, inside the
#                                 gate (MECLAW_CARGO_LOCK_HELD) or in a tree
#                                 whose branch has no `/` (the main tree).
#     MECLAW_STRAND_TOKEN_SKIP=<reason>
#                                 run without a token anyway -- logged, and
#                                 `strand.sh token who` shows it.
#     MECLAW_TIER_REUSE=<dir>     reuse a finished build (the lane executor,
#                                 `scripts/pipeline/executor.py`): run
#                                 `cargo-nextest nextest run --binaries-metadata
#                                 <dir>/binaries.json --cargo-metadata
#                                 <dir>/cargo-metadata.json` directly. Nothing
#                                 is built, so nothing here touches cargo: no
#                                 lanes probe, no token, no cargo lock, no tree
#                                 sync, no `--workspace`, and t0 drops its
#                                 build-only `--lib --bins`. The station runs
#                                 beside a cargo station instead of queueing
#                                 behind its lock.
#
# TEST HOOK (part of the interface, used by scripts/tests/test_gate_sh.py)
# =======================================================================
#     MECLAW_TIER_DRY=1           print the nextest argv instead of running it.
#     MECLAW_TIER_EXCLUDE=<expr>  drop these tests from the selection (the pipeline's
#                                 quarantine, OR-TV-2; never set by hand).
#                                 The locking still happens: that is what the
#                                 hook exists to make testable without paying
#                                 for a compile. The tree sync does NOT -- it
#                                 touches sources, and a dry run that
#                                 invalidated the whole build would be a
#                                 footgun for the sake of a test.
#
# Exit 0 = tier green. Exit 3 = refused: the host's cargo tokens are armed
# and this strand holds none (`scripts/strand.sh token take`).
#
# NOTE: t2 runs the TESTS only. The gates around them (corridor bytes, unwrap
# ratchet, cargo-deny, scenario suites) are stations of `scripts/gate.sh`;
# they are not part of any tier.

set -uo pipefail

# This file's directory with builtins only: an external `dirname` missing from
# PATH made it the caller's cwd and the kit library came from there (M7w).
case "${BASH_SOURCE[0]}" in */*) root=${BASH_SOURCE[0]%/*}/ ;; *) root=./ ;; esac
root=$(cd -- "${root}.." && pwd) || exit 2
# The kit's one Python (`KIT_PY`, lane.PYTHON; re-check tv-bn M7t B-M4).
# shellcheck source=scripts/pipeline/kit_python.sh
. "$root/scripts/pipeline/kit_python.sh" || exit 2
cd "$root" || exit 1

# --- where cargo builds, and the guard that comes with sharing it -----------
# Both this script and `scripts/gate.sh` start cargo, and every worktree of
# this repository shares ONE target directory. The rule lived in the runner
# alone until 2026-09-21, so a targeted test run from a LINKED worktree built a
# private target/ there; what that cost is measured in
# `docs/development-rules.md` section 7 (GH #802). Both scripts now read the
# rule -- and the ghost-binary guard that goes with it -- from one file.
# shellcheck source=scripts/cargo-target.sh
. "$root/scripts/cargo-target.sh" || exit 2
meclaw_resolve_target_dir "$root"
target_dir="$MECLAW_TARGET_DIR"
echo "=== target: $target_dir"

# tv-br M10y (re-check bundle R, R-D1): the quota mark is believed only where
# the quota bites. It is taken again HERE from the `cpu.max` of this process's
# own cgroup (v2, `0::<path>` of /proc/self/cgroup) -- the reading of
# `lane_sync.sh` QUOTA_MARK_SH -- and never from the inherited value: a mark
# set by hand, or carried into a scope whose user manager has no cpu
# controller delegated, would swap in the long quota leash and widen every
# hang mark of a run that is not throttled at all (fail-open). No effective
# quota: no mark, and a time factor other than 1 is refused (OR-T12-13).
# MECLAW_LANE_CGROUP_FS is the cgroup mount (default /sys/fs/cgroup; the pins
# point it at a fake tree).
effective_quota_mark() {
    local cg q="" p="" m
    cg=$(sed -n 's/^0:://p' /proc/self/cgroup 2>/dev/null)
    { read -r q p <"${MECLAW_LANE_CGROUP_FS:-/sys/fs/cgroup}$cg/cpu.max"; } 2>/dev/null || :
    case "$q:$p" in
        *[!0-9:]*|:*|*:) return 0 ;;
    esac
    [ "$p" -gt 0 ] || return 0
    m=$((q * 100 / p))
    [ "$m" = 0 ] || echo "$m"
}
claimed_mark=${MECLAW_TEST_CPU_QUOTA:-}
quota_mark=$(effective_quota_mark)
if [ -n "$claimed_mark" ] && [ "$claimed_mark" != "$quota_mark" ]; then
    echo "=== quota mark: MECLAW_TEST_CPU_QUOTA=$claimed_mark is not the enforced cpu.max (${quota_mark:-none}): the enforced one counts" >&2
fi
if [ -z "$quota_mark" ] && [ -n "${MECLAW_TEST_TIME_FACTOR+x}" ] && [ "$MECLAW_TEST_TIME_FACTOR" != 1 ]; then
    echo "test-tier: MECLAW_TEST_TIME_FACTOR=$MECLAW_TEST_TIME_FACTOR but no effective cpu quota (cpu.max of the own cgroup): a factor belongs to a quota run only (OR-T12-13)" >&2
    exit 2
fi
if [ -n "$quota_mark" ]; then
    export MECLAW_TEST_CPU_QUOTA="$quota_mark"
    # tv-bn FN5 (OR-T12-13, OR-T18-7): every quota run measures under F =
    # QUOTA_TIME_FACTOR -- the quota profile's leashes are laid out for it. A
    # caller that names no factor got F = 1 under the quota: the leashes grew,
    # the hang marks did not, and a 30 s marker of the browser driver turned a
    # throttled page change into a red (NM4 f7w, B-15 at 80 %). Unset is that
    # factor, read from its one line; a factor the caller names stays theirs.
    if [ -z "${MECLAW_TEST_TIME_FACTOR+x}" ]; then
        qf=$(sed -n 's/^pub const QUOTA_TIME_FACTOR: f64 = \([0-9.]*\);$/\1/p' \
            "$root/crates/meclaw-testing/src/deadline.rs" 2>/dev/null)
        if [ -z "$qf" ]; then
            echo "test-tier: a quota run (cpu.max $quota_mark %) without a time factor, and QUOTA_TIME_FACTOR is not in crates/meclaw-testing/src/deadline.rs (OR-T12-13)" >&2
            exit 2
        fi
        export MECLAW_TEST_TIME_FACTOR="$qf"
        echo "=== quota run: time factor MECLAW_TEST_TIME_FACTOR=$qf (QUOTA_TIME_FACTOR, none named)"
    else
        echo "=== quota run: time factor MECLAW_TEST_TIME_FACTOR=$MECLAW_TEST_TIME_FACTOR (named by the caller)"
    fi
else
    unset MECLAW_TEST_CPU_QUOTA
fi

profile=${MECLAW_TIER_PROFILE:-default}
# K8 (OR-T17-15): a quota run is slower by design -- 80 % of a core shared by
# the whole selection. Its hang marks scale with the time factor; the nextest
# kill (`slow-timeout` x `terminate-after`) lives in ONE place, the profile
# `quota` in .config/nextest.toml, chosen here by the effective quota mark and
# by nothing else. Only `default` is swapped: a named profile is the caller's.
if [ -n "$quota_mark" ] && [ "$profile" = default ]; then
    profile=quota
    echo "=== quota run (enforced cpu.max: $quota_mark %): nextest profile quota"
fi

if [ -z "${CI:-}" ]; then
    # A live colony shares this machine: its watchdog reads a long CPU gap as
    # a wedge, and the test fsyncs are what trips it. Half the cores plus a
    # tmpfs TMPDIR keeps both off its back.
    export NEXTEST_TEST_THREADS=${NEXTEST_TEST_THREADS:-4}
    export TMPDIR=/dev/shm/meclaw-tests
    mkdir -p "$TMPDIR" 2>/dev/null || true
fi

# --- the tier filtersets ----------------------------------------------------
#
# T0: unit tests. In nextest terms these are the test binaries of kind `lib`
# and kind `bin` -- i.e. `#[cfg(test)] mod tests` inside the crates, plus the
# CLI's own inline tests. Nothing in tests/ is T0.
T0='kind(lib) + kind(bin)'

# The scenario class: integration binaries that boot a colony, drive a demo, or
# talk to a mock provider end to end. They are the expensive tail of the suite.
# It is defined in scripts/gate_plan.py and read from there -- one list, one
# owner. A second copy here is exactly how the two used to drift. The resolver
# travels with the public export (it is listed in the export's ROOT_FILES), so
# this call resolves in the public tree too.
SCENARIO=$("${KIT_PY[@]}" "$root/scripts/gate_plan.py" --print scenario) || {
    echo "test-tier: cannot read the scenario filterset from scripts/gate_plan.py" >&2
    exit 2
}

# T1: everything that is not scenario-class. Unit tests included -- a merge
# gate that skips the cheap half to save nothing would be silly.
T1="all() - ( ${SCENARIO} )"

# T2: the whole suite.
T2='all()'
# TV-P2 (tv-p1c): the perf tests -- `#[ignore]`, so no tier above runs them
# (none passes `--run-ignored`); only `perf` below, in the profile `perf`.
PERF='test(/(^|::)perf_/)'

# The build-host cargo lock. Run on its own, this script takes it ITSELF. Run
# from `scripts/gate.sh`, the whole gate run already holds it and says so with
# MECLAW_CARGO_LOCK_HELD=1 -- taking it again would deadlock the gate against
# its own child. Same file as the runner's, same switch to move it.
LOCK="${MECLAW_GATE_LOCK:-/tmp/meclaw-w26-cargo.lock}"
# The build this run reuses instead of building (empty: build as always).
reuse="${MECLAW_TIER_REUSE:-}"
# The exit of a `filter` run in which no test ran (GH #997).
EXIT_NO_TESTS=6

# `workspace_scope <nextest args...>`: `--workspace`, unless the caller named a
# package. cargo reads `--workspace` next to `-p` as "every member", so a
# single `strand.sh test '<expr>' -p <pkg>` built 1388 test binaries and ran
# 11246 tests until its timeout (lane probe 2026-10-08, GH #1127).
workspace_scope() {
    local arg
    for arg in "$@"; do
        case "$arg" in
            -p|-p?*|--package|--package=*) return 0 ;;
            --) break ;;
        esac
    done
    echo "--workspace"
}

# K9 (welle-validierung-2): the lease's `seen` once more after the run, as
# `strand.sh gate` does -- checked in only before it, a long tier left `seen`
# at its start and the idle reclaim (15 min) took the lane back as "idle"
# seconds after the run ended. A token released meanwhile is no refusal.
# Said exactly once: the first call clears `token_held`.
#
# On the abort as well (re-check v2-fix-b M4): a tier stopped by ^C or a kill
# left `seen` at its start, and the dead process no longer guarded the lane.
# Once the token is checked in, `token_traps` makes every end say it -- EXIT
# keeps the run's exit, INT/TERM/HUP say `seen` and die of the same signal.
token_held=0
token_seen_after_run() {
    [ "$token_held" = 1 ] || return 0
    token_held=0
    "$root/scripts/strand.sh" token check --pid 0 >/dev/null 2>&1 || true
}
token_seen_on_signal() {
    token_seen_after_run
    trap - "$1"
    kill -s "$1" $$
}
token_traps() {
    trap token_seen_after_run EXIT
    trap 'token_seen_on_signal INT' INT
    trap 'token_seen_on_signal TERM' TERM
    trap 'token_seen_on_signal HUP' HUP
}

run_nextest() {
    local filter="$1"
    shift
    # The runtime quarantine (OR-TV-2, VG TV-F3): a quarantined test leaves the
    # blocking selection; the pipeline runs it in its NOTE station `quarantine`.
    if [ -n "${MECLAW_TIER_EXCLUDE:-}" ]; then
        filter="($filter) - ($MECLAW_TIER_EXCLUDE)"
    fi
    echo "=== nextest [profile=$profile]"
    echo "=== filter: $filter"

    # The cargo token of a wave, before the lock. The lock serialises builds
    # but not how many builders queue behind it: eleven of them waited 20-90
    # minutes per single test and woke 103 times to a cold prompt cache in
    # three hours (the pipeline lesson of the wave that measured it). Checked
    # in dry mode as well, or the refusal could not be tested; skipped in CI
    # and inside a gate, which checked for itself or belongs to the
    # orchestrator; and a throw-away repo without the kit has nothing to ask.
    # The lanes, before the token (GH #942): while a wave builds on the build
    # hosts (`strand.sh token init --lanes ...`), this machine builds nothing
    # -- it runs the owner's live and lab colonies, and a lab colony died at
    # load 15 while strands built beside it (2026-10-02). Every form of the
    # tier is refused with exit 3; a single test goes to the held lane with
    # `scripts/strand.sh test '<filter>'`. Not asked: CI, a tier inside a gate
    # (the gate decided for itself), the tier ON a lane (`MECLAW_TIER_LANE`,
    # set by the kit's remote call), and `MECLAW_LANE_LOCAL_OK=<reason>`. The
    # probe is `meclaw_lanes_active` in `scripts/cargo-target.sh`, shared with
    # the runner. CI passes, but on an armed host it says so in a NOTE line --
    # a `CI=1` typed here is local cargo all the same (C1 finding, L Nit).
    # A reuse set by hand is asked as well (Delta C M-neu-3): the gate's
    # executor sets MECLAW_TIER_LANE beside its own MECLAW_TIER_REUSE.
    if [ -z "${MECLAW_CARGO_LOCK_HELD:-}" ] \
       && [ -z "${MECLAW_TIER_LANE:-}" ] && meclaw_lanes_active "$LOCK"; then
        if [ -n "${CI:-}" ]; then
            echo "=== NOTE lanes armed on this host, cargo runs here under CI"
        elif [ -n "${MECLAW_LANE_LOCAL_OK:-}" ]; then
            echo "=== lanes active, local cargo allowed: $MECLAW_LANE_LOCAL_OK"
        else
            echo "test-tier: lanes active: use scripts/strand.sh test|gate --host <lane>" \
                 "(MECLAW_LANE_LOCAL_OK=<reason> overrides)" >&2
            return 3
        fi
    fi

    if [ -z "$reuse" ] && [ -z "${CI:-}" ] && [ -z "${MECLAW_CARGO_LOCK_HELD:-}" ] \
       && [ -x "$root/scripts/strand.sh" ]; then
        "$root/scripts/strand.sh" token check --pid $$ || return $?
        token_held=1
        token_traps
    fi

    # Who owns the cargo lock for this call.
    local own_lock=1
    if [ -n "$reuse" ]; then
        own_lock=0
        echo "=== cargo lock: none (reusing the build in $reuse)"
    elif [ -n "${CI:-}" ]; then
        own_lock=0
        echo "=== cargo lock: none (CI owns the machine)"
    elif [ -n "${MECLAW_CARGO_LOCK_HELD:-}" ]; then
        own_lock=0
        echo "=== cargo lock: held by the caller (inside scripts/gate.sh)"
    else
        echo "=== cargo lock: taken here ($LOCK)"
    fi

    if [ "$own_lock" = 1 ]; then
        # NOT `flock <file> cargo …`: that form hands the open lock descriptor
        # to cargo, and from there to every process the tests leave behind. A
        # test that spawns `sleep 300` and outlives its runner inherited the
        # lock fd and made the next cargo station wait for it (measured
        # 2026-09-04: 209 s and 263 s for a 0.5 s cargo-deny; `lsof` on the
        # lock file named an orphaned `sleep` with PPID 1). `flock --close` is
        # not the fix either -- it closes the fd before the exec and so
        # releases the lock straight away.
        #
        # Instead: this shell holds the lock on fd 9, and the subshell closes
        # that descriptor before nextest is exec'd. Nothing below this line can
        # inherit it; closing our own copy afterwards is what releases it.
        exec 9>"$LOCK" || {
            echo "test-tier: cannot open the cargo lock $LOCK" >&2
            return 2
        }
        flock 9
    fi

    # The ghost-binary guard, under the lock and before anything is built --
    # the same order `scripts/gate.sh` keeps. Several worktrees share one
    # target/, cargo decides freshness by mtime, and an artefact built in tree
    # A comes back as fresh in tree B; `<target>/.gate-tree` says which tree
    # filled the directory last, and a run that fills it without writing the
    # stamp leaves the next one reading somebody else's answer.
    #
    # Three runs skip it, each for a reason of its own: inside the gate
    # (MECLAW_CARGO_LOCK_HELD) the run has already synced and stamped for the
    # whole chain, CI builds in a fresh checkout with an empty target/, and the
    # dry hook builds nothing that could be a ghost.
    if [ -z "$reuse" ] && [ -z "${CI:-}" ] && [ -z "${MECLAW_CARGO_LOCK_HELD:-}" ] \
       && [ -z "${MECLAW_TIER_DRY:-}" ]; then
        local rev dirty note
        rev=$(git rev-parse HEAD 2>/dev/null || echo "")
        dirty=$(meclaw_dirty_files "$root")
        note=$(meclaw_tree_sync "$root" "$target_dir" "$rev" "$dirty")
        [ -n "$note" ] && echo "=== tree-sync: $note"
        meclaw_write_stamp "$root" "$target_dir" "$rev" "$dirty"
    fi

    # The slow lock (`scripts/slow_tests.py`, station `slow-tests`) reads the JUnit
    # file of THIS run; one left over from an earlier run would let it judge times
    # nobody measured now (GH #1046).
    # Where the file lands is asked of the lock itself (`junit_path`: nextest's
    # store, not cargo's target dir), so a moved store cannot leave it an old
    # file. Should the call fail, the station fails the same way -- red, not
    # silent.
    if [ -z "${MECLAW_TIER_DRY:-}" ] && [ -z "${MECLAW_TIER_LIST:-}" ]; then
        local junit
        junit=$("${KIT_PY[@]}" "$root/scripts/slow_tests.py" --print-junit-path \
                    --profile "$profile") && [ -n "$junit" ] && rm -f "$junit"
    fi

    local rc=0 seen=""
    if [ -n "${MECLAW_TIER_DRY:-}" ] && [ -n "$reuse" ]; then
        echo "=== tier-dry: cargo-nextest nextest run --profile $profile" \
             "--binaries-metadata $reuse/binaries.json" \
             "--cargo-metadata $reuse/cargo-metadata.json -E $filter $*"
    elif [ -n "${MECLAW_TIER_DRY:-}" ]; then
        echo "=== tier-dry: cargo nextest run $(workspace_scope "$@")" \
             "$(meclaw_test_features "$root" "$@" | tr '\n' ' ')--profile $profile -E $filter $*"
    elif [ -n "${MECLAW_TIER_LIST:-}" ] && [ "$tier" = filter ]; then
        # GH #1167 (i): the proof of an empty remainder -- one line per test
        # on stdout (`--message-format oneline`), counted; any error is the
        # list's own exit, never a count.
        seen=$(mktemp) || {
            [ "$own_lock" = 1 ] && exec 9>&-
            echo "test-tier: no temp file" >&2
            return 2
        }
        cargo_nextest "$filter" "$@" > "$seen" 9>&-
        rc=$?
        [ "$own_lock" = 1 ] && exec 9>&-
        token_seen_after_run
        cat "$seen"
        if [ "$rc" = 0 ]; then
            echo "test-tier: LISTED $(grep -c . "$seen")"
        else
            echo "test-tier: LIST FAILED (exit $rc): $filter" >&2
        fi
        rm -f "$seen"
        return "$rc"
    elif [ "$tier" = filter ]; then
        # A filter is judged on what ran, not only on the exit (GH #997): the
        # output goes through `tee` so the summary can be read afterwards.
        seen=$(mktemp) || {
            [ "$own_lock" = 1 ] && exec 9>&-
            echo "test-tier: no temp file" >&2
            return 2
        }
        cargo_nextest "$filter" "$@" 2>&1 | tee "$seen" 9>&-
        rc=${PIPESTATUS[0]}
    else
        cargo_nextest "$filter" "$@"
        rc=$?
    fi
    [ "$own_lock" = 1 ] && exec 9>&-
    token_seen_after_run
    if [ -n "$seen" ]; then
        if filterset_refused "$rc" "$seen"; then
            echo "test-tier: RED -- no tests matched (filterset refused): $filter" >&2
            rc=$EXIT_NO_TESTS
        elif no_tests_matched "$rc" "$seen"; then
            echo "test-tier: RED -- no tests matched: $filter" >&2
            rc=$EXIT_NO_TESTS
        fi
        rm -f "$seen"
    fi
    return "$rc"
}

cargo_nextest() {
    local filter="$1"
    shift
    local scope=() verb=run
    if [ -n "${MECLAW_TIER_LIST:-}" ]; then
        verb=list
        set -- --message-format oneline "$@"
    fi
    [ -n "$(workspace_scope "$@")" ] && scope=(--workspace)
    # The test-hook feature of the test builds (GH #1208, scripts/cargo-target.sh):
    # the same flags as the gate's build station for the same selection. A
    # reused build already carries them.
    local feat
    while IFS= read -r feat; do
        scope+=("$feat")
    done < <(meclaw_test_features "$root" "$@")
    if [ -n "$reuse" ]; then
        # No cargo in this call at all: `cargo nextest` would start cargo for
        # the build-dir lock and the metadata; the plain binary reads both from
        # the files of the build it reuses.
        set -- --profile "$profile" --binaries-metadata "$reuse/binaries.json" \
               --cargo-metadata "$reuse/cargo-metadata.json" -E "$filter" "$@"
        if [ -n "${CI:-}" ]; then
            cargo-nextest nextest "$verb" "$@"
        else
            nice -n 19 ionice -c3 cargo-nextest nextest "$verb" "$@"
        fi
    elif [ -n "${CI:-}" ]; then
        cargo nextest "$verb" "${scope[@]}" --profile "$profile" -E "$filter" "$@"
    else
        ( [ "$own_lock" = 1 ] && exec 9>&-
          nice -n 19 ionice -c3 \
              cargo nextest "$verb" "${scope[@]}" --profile "$profile" -E "$filter" "$@" )
    fi
}

# `no_tests_matched <rc> <output>`: did a filter run no test at all? Three
# shapes of the same thing -- a zero in nextest's summary, a clean exit
# without any summary, nextest's own "no tests to run" (exit 4).
# `filterset_refused <rc> <output>`: nextest refused the filterset (an
# operator matched nothing, or it did not parse) -- also exit 6, in a line of
# its own, because nothing was looked at. A failing test or a failed build
# has its own exit and keeps it -- and so does a run that passed: the refusal
# text is read only off any OTHER exit, else a test that merely prints it
# would turn 0 or 100 into 6.
filterset_refused() {
    case "$1" in
        0|4|100|101) return 1 ;;
    esac
    grep -qE "failed to parse filterset|didn't match any" "$2"
}

no_tests_matched() {
    local rc="$1" out="$2" total
    [ "$rc" = 4 ] && return 0
    total=$(grep -oE '[0-9]+ tests? run:' "$out" | tail -1 | grep -oE '^[0-9]+')
    [ "$total" = 0 ] && return 0
    [ "$rc" = 0 ] && [ -z "$total" ] && return 0
    return 1
}

if [ $# -lt 1 ]; then
    echo "usage: scripts/test-tier.sh t0|t1|t2 [nextest args...]" >&2
    echo "       scripts/test-tier.sh filter '<nextest filterset>' [nextest args...]" >&2
    exit 2
fi

tier="$1"
shift

case "$tier" in
    t0)
        # `--lib --bins` is not a duplicate of the filterset: the filter decides
        # what RUNS, this decides what gets BUILT. Without it nextest compiles
        # all ~660 integration binaries before running 13 of them, and the tier
        # costs minutes of rustc instead of seconds of tests. A reused build
        # builds nothing, and nextest refuses build flags there.
        if [ -n "$reuse" ]; then
            run_nextest "$T0" "$@"
        else
            run_nextest "$T0" --lib --bins "$@"
        fi
        ;;
    t1) run_nextest "$T1" "$@" ;;
    t2) run_nextest "$T2" "$@" ;;
    filter)
        if [ $# -lt 1 ]; then
            echo "test-tier: 'filter' needs a nextest filterset" >&2
            exit 2
        fi
        expr="$1"
        shift
        run_nextest "$expr" "$@"
        ;;
    perf)
        # Never a gate tier (TV-P2): one thread, no retries, the ignored perf_*
        # tests only; a measurement, never a verdict of a pass.
        profile=perf
        run_nextest "$PERF" --run-ignored only "$@"
        ;;
    changed)
        echo "test-tier: 'changed' is gone -- run 'scripts/gate.sh strand' instead" >&2
        exit 2
        ;;
    *)
        echo "unknown tier: $tier (expected t0|t1|t2|filter|perf)" >&2
        exit 2
        ;;
esac
