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
#   changed  Not a tier: an incremental selection. Works out which workspace
#            packages the diff touches and runs the tests of those packages
#            AND of every package that depends on them (nextest `rdeps()`,
#            which reads the real cargo graph).
#
# The tiers are nextest FILTERSETS, defined once, right below. Change them
# here; nothing else hard-codes a test name.
#
# USAGE
# =====
#     scripts/test-tier.sh t0|t1|t2|changed [extra nextest args...]
#
#     MECLAW_TIER_BASE=<git ref>  base for `changed` (default: see pick_base)
#     MECLAW_TIER_PROFILE=<name>  nextest profile (default: default)
#
# Exit 0 = tier green.
#
# NOTE: t2 runs the TESTS only. The release gates (corridor bytes, unwrap
# ratchet, cargo-deny, scenario suite) are run by the CI `gates` job and by the
# maintainer's pre-push script; they are not part of any tier.

set -uo pipefail

root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root" || exit 1

profile=${MECLAW_TIER_PROFILE:-default}

# --- the tier filtersets ----------------------------------------------------
#
# T0: unit tests. In nextest terms these are the test binaries of kind `lib`
# and kind `bin` -- i.e. `#[cfg(test)] mod tests` inside the crates, plus the
# CLI's own inline tests. Nothing in tests/ is T0.
T0='kind(lib) + kind(bin)'

# The scenario class: integration binaries that boot a colony, drive a demo, or
# talk to a mock provider end to end. They are the expensive tail of the suite.
# Matched by binary name, because that is the only stable handle the workspace
# gives us -- the test files are named by topic (gh<N>_, phase_<N>_, w<N>_,
# paket_<N>_), not by cost.
SCENARIO='binary(/_demo$/) + binary(/_demo_/) + binary(/e2e/) + binary(/^workshop_scenario$/) + binary(/^slack_live$/) + binary(/^harness_real_cli_smoke$/) + binary(/^audit_14_/)'

# T1: everything that is not scenario-class. Unit tests included -- a merge
# gate that skips the cheap half to save nothing would be silly.
T1="all() - ( ${SCENARIO} )"

# T2: the whole suite.
T2='all()'

run_nextest() {
    local filter="$1"
    shift
    echo "=== nextest [profile=$profile]"
    echo "=== filter: $filter"
    nice -n 19 ionice -c3 flock /tmp/meclaw-w26-cargo.lock \
        cargo nextest run --workspace --profile "$profile" -E "$filter" "$@"
}

# --- `changed`: which packages does the diff touch? -------------------------
pick_base() {
    if [ -n "${MECLAW_TIER_BASE:-}" ]; then
        echo "$MECLAW_TIER_BASE"
        return
    fi
    # Deliberately NOT `github-main`: that is the published export mirror and
    # sits thousands of commits behind, so a merge-base against it selects the
    # whole workspace and the incremental selection buys nothing.
    for ref in '@{upstream}' origin/master; do
        if git rev-parse --verify --quiet "$ref" >/dev/null; then
            # A base identical to HEAD tells us nothing -- step back one commit
            # so that the last commit plus the working tree is what we look at.
            if [ "$(git rev-parse "$ref")" = "$(git rev-parse HEAD)" ]; then
                git rev-parse --verify --quiet HEAD~1 >/dev/null && echo "HEAD~1" && return
                continue
            fi
            echo "$ref"
            return
        fi
    done
    echo "HEAD~1"
}

changed_files() {
    local base="$1"
    local mb
    mb=$(git merge-base "$base" HEAD 2>/dev/null || echo "$base")
    git diff --name-only "$mb" HEAD
    # Uncommitted work counts too: it is what you are about to be judged on.
    git status --porcelain --untracked-files=all | sed 's/^...//'
}

if [ $# -lt 1 ]; then
    echo "usage: scripts/test-tier.sh t0|t1|t2|changed [nextest args...]" >&2
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
    changed)
        base=$(pick_base)
        echo "=== changed vs. $base"
        files=$(changed_files "$base" | sort -u)
        if [ -z "$files" ]; then
            echo "=== nothing changed -- running t0 as a floor"
            run_nextest "$T0" "$@"
            exit $?
        fi
        # Map file -> workspace package via the manifest paths cargo reports.
        pkgs=$(cargo metadata --no-deps --format-version 1 2>/dev/null \
            | jq -r '.packages[] | "\(.name)\t\(.manifest_path)"' \
            | while IFS=$'\t' read -r name manifest; do
                dir=${manifest%/Cargo.toml}
                dir=${dir#"$root"/}
                echo "$files" | grep -q "^$dir/" && echo "$name"
            done | sort -u)
        if [ -z "$pkgs" ]; then
            # Nothing under crates/ moved. Templates, fixtures, docs and gate
            # data still feed drift locks, so fall back to the merge tier
            # rather than declaring victory.
            echo "=== no crate touched -- falling back to t1"
            run_nextest "$T1" "$@"
            exit $?
        fi
        echo "=== touched packages:"; echo "$pkgs" | sed 's/^/    /'
        # rdeps() = the package and everything that depends on it.
        filter=$(echo "$pkgs" | sed 's/.*/rdeps(&)/' | paste -sd '+' -)
        run_nextest "$filter" "$@"
        ;;
    *)
        echo "unknown tier: $tier (expected t0|t1|t2|changed)" >&2
        exit 2
        ;;
esac
