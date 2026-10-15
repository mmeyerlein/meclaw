#!/usr/bin/env bash
# meclaw -- the host part of the coverage run (W6, GH #1120); `lane_sync.sh
# coverage` runs it on the lane, in the layout element `coverage/tree`:
#
#     scripts/pipeline/coverage_host.sh <target-dir> <deadline-s> <rev> [full|browser]
#
# cwd = the pushed tree (`$ROOT/coverage/tree`); <target-dir> is
# `$ROOT/coverage/target`, its own target -- instrumented artefacts never
# meet the warm strand target. The profiles go to `<parent>/profiles`, the
# map to `<parent>/coverage.json` (parent = the directory of <target-dir>).
# Push, hygiene, floor and sweep are the lane engine's (review I1), not this
# script's.
#
# No cargo-llvm-cov: `-C instrument-coverage` in RUSTFLAGS plus nextest is the
# whole build; `--cfg tokio_unstable` stays in the same RUSTFLAGS (RUSTFLAGS
# replaces the `rustflags` of .cargo/config.toml). The tools are the
# toolchain's `llvm-tools` component (llvm-profdata, llvm-cov). The deadline
# is this side's own `timeout`, so a dropped connection leaves no build
# running.
#
# The browser locks (welle-tb S1b, #1165, owner's ruling 10.10.): `#[ignore]`d
# in the tree, so the main run leaves them out and a second call runs every
# lock (`--run-ignored all`, both engines) under the same instrumentation, with
# the nextest profile `coverage-browser`: its junit stores the output of green
# tests, and `aggregate --browser-junit` marks a lock measured only with merged
# profiles and no `SKIP ` line (scripts/pipeline/browser_cover.py). The main run
# excludes the sheet lock too (it is not ignored), so no lock runs twice. Scope
# `browser` is the probe of the locks alone: no main run, the list of the locks
# only -- its map is incomplete by design and `coverage build` never installs it.
set -euo pipefail
# The kit's one Python (`KIT_PY`, lane.PYTHON; re-check tv-bn M7t B-M4).
# This file's directory with builtins only: an external `dirname` missing from
# PATH made it the caller's cwd and the kit library came from there (M7w); a
# library relative to the cwd came from wherever the caller stood (M7y).
case "${BASH_SOURCE[0]}" in */*) kit=${BASH_SOURCE[0]%/*}/ ;; *) kit=./ ;; esac
kit=$(cd -- "${kit}.." && pwd) || exit 2
# shellcheck source=scripts/pipeline/kit_python.sh
. "$kit/pipeline/kit_python.sh" || exit 2

[ $# -eq 3 ] || [ $# -eq 4 ] || { echo "usage: coverage_host.sh <target-dir> <deadline-s> <rev> [full|browser]" >&2; exit 2; }
T=$1 DEADLINE_S=$2 REV=$3
SCOPE=${4:-full}
case "$SCOPE" in full|browser) ;; *) echo "coverage: scope '$SCOPE' is not full|browser" >&2; exit 2 ;; esac
LOCK_SEL=$("${KIT_PY[@]}" scripts/pipeline/browser_cover.py select)
LOCK_FILTER=$("${KIT_PY[@]}" scripts/pipeline/browser_cover.py filter)
# The test-hook feature (GH #1208, scripts/cargo-target.sh): the same flags as
# the gate's build station and test-tier.sh for the same selection, or the
# `#![cfg(feature = "test-hooks")]` tests are empty stubs in the map.
# shellcheck source=scripts/cargo-target.sh
. "$kit/cargo-target.sh" || exit 2
FEAT_WS=$(meclaw_test_features "$PWD" --workspace | tr '\n' ' ')
# shellcheck disable=SC2086 # the selection is words: -p <crate> --test <stem> ...
FEAT_LOCK=$(meclaw_test_features "$PWD" $LOCK_SEL | tr '\n' ' ')
C=${T%/*}
start=$(date +%s)
rm -rf "$C/profiles"
mkdir -p "$C/profiles" "$T"
TOOLS="$(rustc --print sysroot)/lib/rustlib/x86_64-unknown-linux-gnu/bin"
# exit 7 = `coverage.TOOL_EXIT`: `coverage build` journals coverage_tool_missing (V2-A2)
test -x "$TOOLS/llvm-profdata" || { echo "coverage: llvm-tools missing (rustup component add llvm-tools)" >&2; exit 7; }
export CARGO_TARGET_DIR="$T"
# `%c` (continuous mode, see coverage_runner.sh) needs runtime counter
# relocation on Linux: without it every profile write fails ("Neither
# __llvm_profile_counter_bias ...") and the map measures nothing.
export RUSTFLAGS="--cfg tokio_unstable -C instrument-coverage -C llvm-args=-runtime-counter-relocation"
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUNNER="$PWD/scripts/pipeline/coverage_runner.sh"
export MECLAW_COVERAGE_PROFILE_DIR="$C/profiles"
export MECLAW_LLVM_PROFDATA="$TOOLS/llvm-profdata"
export LLVM_PROFILE_FILE="$C/profiles/_build/%p-%m%c.profraw"
# the exit is COUNTED, never swallowed (review C1): anything but 0 -- red
# tests (100), a timeout (124), a kill (137) -- makes the map incomplete;
# red tests make its verdict RED, by name from the log (welle-tb S1 C-Beleg)
rc=0
: >"$C/nextest.log"
if [ "$SCOPE" = full ]; then
    # shellcheck disable=SC2086 # the flags are words
    timeout --kill-after=60 "$DEADLINE_S" nice -n 10 cargo nextest run --workspace $FEAT_WS --no-fail-fast \
        --run-ignored default -E "not ($LOCK_FILTER)" >"$C/nextest.log" 2>&1 || rc=$?
    echo "coverage: nextest exit $rc" >&2
fi
# the browser call gets what is left of the deadline; none left counts as a timeout
left=$(( DEADLINE_S - ($(date +%s) - start) ))
# nextest keeps its store under the WORKSPACE root (`target/nextest/<profile>/`),
# not under CARGO_TARGET_DIR: the probe of 10.10. ran 22 green lock tests and
# measured 0/7 because the junit was looked for under $T
BJ="$PWD/target/nextest/coverage-browser/junit.xml"
brc=124
rm -f "$BJ"
if [ "$left" -gt 60 ]; then
    brc=0
    # shellcheck disable=SC2086 # the selection is words: -p <crate> --test <stem> ...
    timeout --kill-after=60 "$left" nice -n 10 cargo nextest run $LOCK_SEL $FEAT_LOCK --no-fail-fast \
        --profile coverage-browser --run-ignored all >>"$C/nextest.log" 2>&1 || brc=$?
fi
echo "coverage: browser locks nextest exit $brc" >&2
[ "$rc" != 0 ] || rc=$brc
if [ "$SCOPE" = full ]; then
    # shellcheck disable=SC2086
    cargo nextest list --workspace $FEAT_WS --message-format json >"$C/list.json"
else
    # shellcheck disable=SC2086
    cargo nextest list $LOCK_SEL $FEAT_LOCK --message-format json >"$C/list.json"
fi
secs=$(( $(date +%s) - start ))
"${KIT_PY[@]}" scripts/pipeline.py coverage aggregate --profiles "$C/profiles" --objects "$C/list.json" \
    --root "$PWD" --rev "$REV" --runner lane --secs "$secs" --out "$C/coverage.json" \
    --profdata "$TOOLS/llvm-profdata" --llvm-cov "$TOOLS/llvm-cov" --nextest-exit "$rc" \
    --nextest-log "$C/nextest.log" --browser-junit "$BJ"
