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
#
#     MECLAW_TIER_PROFILE=<name>  nextest profile (default: default)
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
#
# TEST HOOK (part of the interface, used by scripts/tests/test_gate_sh.py)
# =======================================================================
#     MECLAW_TIER_DRY=1           print the nextest argv instead of running it.
#                                 The locking still happens: that is what the
#                                 hook exists to make testable without paying
#                                 for a compile.
#
# Exit 0 = tier green.
#
# NOTE: t2 runs the TESTS only. The gates around them (corridor bytes, unwrap
# ratchet, cargo-deny, scenario suites) are stations of `scripts/gate.sh`;
# they are not part of any tier.

set -uo pipefail

root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root" || exit 1

profile=${MECLAW_TIER_PROFILE:-default}

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
SCENARIO=$(python3 "$root/scripts/gate_plan.py" --print scenario) || {
    echo "test-tier: cannot read the scenario filterset from scripts/gate_plan.py" >&2
    exit 2
}

# T1: everything that is not scenario-class. Unit tests included -- a merge
# gate that skips the cheap half to save nothing would be silly.
T1="all() - ( ${SCENARIO} )"

# T2: the whole suite.
T2='all()'

# The build-host cargo lock. Run on its own, this script takes it ITSELF. Run
# from `scripts/gate.sh`, the whole gate run already holds it and says so with
# MECLAW_CARGO_LOCK_HELD=1 -- taking it again would deadlock the gate against
# its own child. Same file as the runner's, same switch to move it.
LOCK="${MECLAW_GATE_LOCK:-/tmp/meclaw-w26-cargo.lock}"

run_nextest() {
    local filter="$1"
    shift
    echo "=== nextest [profile=$profile]"
    echo "=== filter: $filter"

    # Who owns the cargo lock for this call.
    local own_lock=1
    if [ -n "${CI:-}" ]; then
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

    local rc=0
    if [ -n "${MECLAW_TIER_DRY:-}" ]; then
        echo "=== tier-dry: cargo nextest run --workspace --profile $profile" \
             "-E $filter $*"
    elif [ -n "${CI:-}" ]; then
        cargo nextest run --workspace --profile "$profile" -E "$filter" "$@"
        rc=$?
    else
        ( [ "$own_lock" = 1 ] && exec 9>&-
          nice -n 19 ionice -c3 \
              cargo nextest run --workspace --profile "$profile" -E "$filter" "$@" )
        rc=$?
    fi
    [ "$own_lock" = 1 ] && exec 9>&-
    return "$rc"
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
        # costs minutes of rustc instead of seconds of tests.
        run_nextest "$T0" --lib --bins "$@"
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
    changed)
        echo "test-tier: 'changed' is gone -- run 'scripts/gate.sh strand' instead" >&2
        exit 2
        ;;
    *)
        echo "unknown tier: $tier (expected t0|t1|t2|filter)" >&2
        exit 2
        ;;
esac
