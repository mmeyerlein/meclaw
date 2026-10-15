#!/bin/sh
# meclaw -- the target runner of the coverage build (W6, GH #1120).
#
# cargo runs every test binary through CARGO_TARGET_<triple>_RUNNER; nextest
# hands each test process NEXTEST_BINARY_ID and NEXTEST_TEST_NAME. `%p`/`%m`
# in LLVM_PROFILE_FILE alone do not know which test wrote a profile, and
# neither JUnit nor libtest-json carry PIDs -- so this runner names the
# profile directory after the test:
#
#     $MECLAW_COVERAGE_PROFILE_DIR/<binary_id>/<module>/<test>/%p-%m%c.profraw
#
# (`/` in <binary_id> written as `@`, see below).
#
# `<module>` is the first segment of the test name (the old test file of a
# harness binary, the top module of a lib's unit tests). Right after the test
# the raw profiles of that one test (and of every child it spawned, which
# inherits the variable) are merged into `<test>.profdata` (`-sparse`) and the
# raw files go: thousands of raw profiles of large binaries are tens of GB.
#
# `%c` is LLVM's continuous mode: the counters live in the mapped file, so a
# test (or a colony child) ended by SIGKILL -- a nextest timeout, the
# watchdog -- still leaves its profile (review C1). On Linux it needs
# `-C llvm-args=-runtime-counter-relocation` in the build (coverage_host.sh).
#
# Fallback without the nextest variables: one directory per binary (the stem
# of the executable without cargo's hash) -- the map is then per binary.
# A `--list` call (nextest lists through the runner too) keeps no profile.
set -u
base=${MECLAW_COVERAGE_PROFILE_DIR:?MECLAW_COVERAGE_PROFILE_DIR is not set}
profdata=${MECLAW_LLVM_PROFDATA:-llvm-profdata}

for a in "$@"; do
    if [ "$a" = "--list" ]; then
        mkdir -p "$base/_list"
        LLVM_PROFILE_FILE="$base/_list/%p.profraw"
        export LLVM_PROFILE_FILE
        exec "$@"
    fi
done

bin=${NEXTEST_BINARY_ID:-}
name=${NEXTEST_TEST_NAME:-}
if [ -n "$bin" ] && [ -n "$name" ]; then
    module=${name%%::*}
    tname=$(printf '%s' "$name" | tr '/' '_')
else
    exe=$(basename -- "$1")
    bin=$(printf '%s' "$exe" | sed 's/-[0-9a-f]\{16\}$//')
    module=_binary
    tname=run-$$
fi
# `@` is the stand-in for `/` below: an id that carries one itself would map
# back to another binary -- refused, the test counts red, the map incomplete
case "$bin" in
    *@*) echo "coverage_runner: binary id '$bin' carries '@' -- refused (it stands for '/')" >&2
         exit 2 ;;
esac
# a bin target's id carries a `/` (`meclaw::bin/meclaw`): `@` keeps it one
# directory level (ids are crate names, `::` and `/` -- `@` never occurs);
# coverage.profile_parts maps it back
dir="$base/$(printf '%s' "$bin" | tr '/' '@')/$module/$tname"
mkdir -p "$dir"
LLVM_PROFILE_FILE="$dir/%p-%m%c.profraw"
export LLVM_PROFILE_FILE

"$@"
rc=$?

set -- "$dir"/*.profraw
if [ -e "$1" ]; then
    if "$profdata" merge -sparse "$@" -o "$dir.profdata" >/dev/null 2>&1; then
        rm -f "$@"
        rmdir "$dir" 2>/dev/null
    fi
fi
exit $rc
