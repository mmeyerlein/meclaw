#!/bin/bash -p
# Thin entry for scripts/release.py (wave release-tempo, RT4): one command from
# the release gate to the website, `prepare` then `publish`. The docstring of
# release.py is the manual; this file only starts it with the kit's Python
# (`KIT_PY`, isolated), found next to this file, never under the caller's cwd.

# One group, parsed whole before its first command runs: an in-place edit of this file under a running shell changes nothing that runs (GH #1223).
{
# This file's directory with builtins only (M7w), as the other kit entries.
case "${BASH_SOURCE[0]}" in */*) here=${BASH_SOURCE[0]%/*}/ ;; *) here=./ ;; esac
here=$(cd -- "$here" && pwd) || exit 2

# The kit's one Python (`KIT_PY`, lane.PYTHON; re-check tv-bn M7t B-M4).
# shellcheck source=scripts/pipeline/kit_python.sh
. "$here/pipeline/kit_python.sh" || exit 2
exec "${KIT_PY[@]}" "$here/release.py" "$@"
}; exit $?
