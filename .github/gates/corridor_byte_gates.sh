#!/usr/bin/env bash
# meclaw-os -- the two corridor byte gates, as a command CI can run (GH #115).
#
# WHAT IS FROZEN
# ==============
# `route()` and `handle_cell_died` in crates/meclaw-colony/src/colony.rs are
# byte-frozen. Both carry a `#[rustfmt::skip]` ABOVE the `async fn` line, which
# is deliberately outside the extraction range below -- so `cargo fmt` runs
# freely over the workspace without ever touching a corridor body.
#
# Changing either body or signature needs an explicit sanction plus a
# re-baseline of the matching fixture. Until then, both diffs must be EMPTY.
#
# WHY THE FIXTURES LIVE UNDER .github/
# ====================================
# This gate has to run in the published tree, and the published tree carries
# no `plans/` directory -- that is where the original fixtures live. A gate
# whose reference file does not travel is not a gate, it is a comment. So the
# two reference bodies sit next to this script, and a private drift lock
# (crates/meclaw-colony/tests/a2_corridor_fixture_drift.rs, never exported)
# holds them byte-identical to the originals.
#
# USAGE
# =====
#     .github/gates/corridor_byte_gates.sh
#
# Exit 0 = both corridors are byte-identical to their frozen reference.
# Exit 1 = a corridor moved (the diff is printed) or an input file is missing.

set -uo pipefail

root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
src="$root/crates/meclaw-colony/src/colony.rs"
fixtures="$root/.github/fixtures"

if [ ! -f "$src" ]; then
    echo "corridor gate: source missing: $src" >&2
    exit 1
fi

status=0

# $1 = human name, $2 = sed range start, $3 = fixture file
gate() {
    local name="$1" start="$2" fixture="$fixtures/$3"

    if [ ! -f "$fixture" ]; then
        echo "corridor gate [$name]: fixture missing: $fixture" >&2
        status=1
        return
    fi

    local extracted
    extracted=$(sed -n "/^$start/,/^}\$/p" "$src")

    if [ -z "$extracted" ]; then
        echo "corridor gate [$name]: extracted nothing -- the signature line" >&2
        echo "  '/^$start/' no longer matches. Renaming or reformatting a" >&2
        echo "  corridor signature is itself a corridor change." >&2
        status=1
        return
    fi

    if diff -u "$fixture" - <<<"$extracted"; then
        echo "corridor gate [$name]: OK (byte-identical, $(wc -l <"$fixture") lines)"
    else
        echo "corridor gate [$name]: FAILED -- the frozen body changed." >&2
        echo "  Reference: .github/fixtures/$3" >&2
        echo "  A corridor moves only with an explicit sanction plus a" >&2
        echo "  re-baseline of BOTH the reference above and its original under" >&2
        echo "  plans/ (the private drift lock a2_corridor_fixture_drift.rs" >&2
        echo "  fails until the two agree again)." >&2
        status=1
    fi
}

gate "route" "async fn route(" "expected_route_body.txt"
gate "handle_cell_died" "async fn handle_cell_died(" "expected_handle_cell_died_body.txt"

exit $status
