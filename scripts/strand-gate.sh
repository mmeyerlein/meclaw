#!/usr/bin/env bash
# meclaw -- moved 2026-09-04: scripts/gate.sh strand
# --base is passed through. --corpus is swallowed: corpus regeneration is now
# automatic -- the resolver plans it when the diff touches a corpus source.
set -uo pipefail
args=(); for a in "$@"; do
    if [ "$a" = "--corpus" ]; then
        echo "strand-gate: --corpus ignored (corpus regeneration is now automatic)" >&2
    else
        args+=("$a")
    fi
done
# This file's directory with builtins only: an external `dirname` missing from
# PATH made it the caller's cwd and the kit library came from there (M7w).
case "${BASH_SOURCE[0]}" in */*) here=${BASH_SOURCE[0]%/*}/ ;; *) here=./ ;; esac
here=$(cd -- "$here" && pwd) || exit 2
exec "$here/gate.sh" strand ${args[@]+"${args[@]}"}
