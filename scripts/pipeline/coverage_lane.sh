#!/usr/bin/env bash
# meclaw -- the coverage run on a lane (W6, GH #1120); `pipeline coverage build` calls it.
#
#     scripts/pipeline/coverage_lane.sh <ssh-target> <strand> <out-dir>
#
# A thin caller of the lane engine (review I1): `lane_sync.sh coverage` pushes
# this tree into the layout element `coverage/tree`, runs the hygiene check
# with `coverage/{tree,target}` as own, the fail-closed floor, the host part
# `coverage_host.sh` and the sweep; it refuses `local` and class egon (I2).
# The map comes back as `<out-dir>/coverage.json`.
#
# The rev of the map is HEAD -- or `<sha>-dirty` when the tree carries an
# overlay (uncommitted, untracked or deleted files, judged at the sync): such
# a map measured something no commit names, so impact (which takes a map only
# for its own rev) never uses
# it. The lane deadline lives with the caller (`timeout` 2 h, the abort
# watchdog); the host part carries its own `timeout` as well.
# One group, parsed whole before its first command runs: an in-place edit of this file under a running shell changes nothing that runs (GH #1223).
{
set -euo pipefail

[ $# -eq 3 ] || [ $# -eq 4 ] || { echo "usage: coverage_lane.sh <ssh-target> <strand> <out-dir> [full|browser]" >&2; exit 2; }
# <scope> `browser` (S1b, #1165): the probe of the browser locks alone, see coverage_host.sh
target=$1 strand=$2 out=$3 scope=${4:-full}
DEADLINE_S=${MECLAW_COVERAGE_DEADLINE_S:-7000}
# This file's directory with builtins only: an external `dirname` missing from
# PATH made it the caller's cwd and the kit library came from there (M7w).
case "${BASH_SOURCE[0]}" in */*) here=${BASH_SOURCE[0]%/*}/ ;; *) here=./ ;; esac
here=$(cd -- "${here}.." && pwd) || exit 2
# The kit's one Python (`KIT_PY`, lane.PYTHON; re-check tv-bn M7t B-M4).
# shellcheck source=scripts/pipeline/kit_python.sh
. "$here/pipeline/kit_python.sh" || exit 2
mkdir -p "$out"

start=$(date +%s)
# The overlay is judged when the tree LEAVES for the lane, not after the run:
# read afterwards it was the local worktree minutes later, where a build or an
# edit had landed meanwhile -- probe run 1 of 10.10. called a clean ab1f6fa11
# rev-dirty (re-check S1b). `files` lists what the overlay sends; a tracked file
# deleted here is missing on the lane as well, so it counts too, and a git that
# cannot say counts as dirty (fail-closed).
overlay=$("$here/lane_sync.sh" files | tr -d '\0')
gone=$(git -C "$here/.." diff --no-renames --name-only --diff-filter=D HEAD --) || gone="?"
# The engine fetches the map itself, under the tree lock (GH #1133 M4).
"$here/lane_sync.sh" coverage "$target" "$strand" "$DEADLINE_S" "$out" "$scope"
if [ -n "$overlay$gone" ]; then
    "${KIT_PY[@]}" - "$out/coverage.json" <<'PY'
import json, sys
p = sys.argv[1]
with open(p, encoding="utf-8") as fh:
    cov = json.load(fh)
cov["rev"] = "%s-dirty" % cov["rev"]
with open(p, "w", encoding="utf-8") as fh:
    json.dump(cov, fh, sort_keys=True)
PY
    echo "coverage: the tree carries an overlay -- the map is rev-dirty, impact never takes it" >&2
fi
echo "coverage: $(( $(date +%s) - start ))s, map in $out/coverage.json"
true
}; exit $?
