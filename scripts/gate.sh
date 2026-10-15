#!/bin/bash -p
# meclaw -- the gate entry point. Since W3 (GH #1117) a shell over two runners:
#
#     scripts/gate.sh <strand|integration|release|ci> [options]
#
#   strand|integration|release
#       -> python3 scripts/pipeline.py gate --mode <mode> [options]
#          The pipeline: stations in parallel, cache, leases, archive. Every
#          option is passed through unchanged; it takes all the options of
#          the old runner plus `--no-pre --reason <text>`, `--strand <name>`
#          and `--wave <name>`.
#   ci
#       -> scripts/gate_legacy.sh ci [options], for good. The public CI
#          workflow runs `gate.sh ci --plan-only` and `gate.sh ci`; that
#          runner stays as it is.
#   --lane stays with its mode: `ci --lane` is the old runner's,
#          `strand|integration|release --lane` the pipeline's, with its
#          pre-stamp rule (GH #1139). There is no switch back to the old
#          runner for the pipeline's modes (GH #1156).
#   -h|--help, no mode, an unknown mode
#       -> scripts/gate_legacy.sh: its header is the manual (options, output
#          lines, receipt, exit codes -- the pipeline keeps them), and it
#          refuses an unknown mode with exit 2.
#
# Both runners are found next to this file, never under the caller's working
# directory. The exit code is the runner's (exec).

set -uo pipefail

# This file's directory with builtins only: an external `dirname` missing from
# PATH made it the caller's cwd and the kit library came from there (M7w).
case "${BASH_SOURCE[0]}" in */*) here=${BASH_SOURCE[0]%/*}/ ;; *) here=./ ;; esac
here=$(cd -- "$here" && pwd) || exit 2
legacy="$here/gate_legacy.sh"

case "${1:-}" in
    strand|integration|release) ;;
    *) exec /bin/bash -p "$legacy" "$@" ;;
esac

mode="$1"
shift
# The kit's one Python (`KIT_PY`, lane.PYTHON; re-check tv-bn M7t B-M4).
# shellcheck source=scripts/pipeline/kit_python.sh
. "$here/pipeline/kit_python.sh" || exit 2
exec "${KIT_PY[@]}" "$here/pipeline.py" gate --mode "$mode" "$@"
