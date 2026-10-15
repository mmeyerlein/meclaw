#!/usr/bin/env bash
# shellcheck shell=bash
# meclaw -- the shared cargo target directory and the guard that goes with it.
#
# SOURCED, NEVER RUN. `scripts/gate.sh` and `scripts/test-tier.sh` both start
# cargo, and every worktree of this repository shares ONE target directory. The
# rule for which one, and the ghost-binary guard that a shared directory needs,
# lived in the runner alone until 2026-09-21 -- so the tier script built a
# PRIVATE target/ in every linked worktree it ran in, and what that cost is
# measured in `docs/development-rules.md` section 7 (GH #802). One rule with
# two writers is not a rule, so both scripts read it from here.
#
# WHICH TARGET DIRECTORY
# ======================
# A private target/ per linked worktree costs a full cold build each and fills
# the disk; it also leaves the worktree without the `target/debug/meclaw` the
# scenario suites run against (measured 2026-09-04). So:
#
#   * an explicit CARGO_TARGET_DIR always wins;
#   * a LINKED worktree uses the MAIN worktree's `target/` and exports
#     CARGO_TARGET_DIR, so every cargo call below builds there too;
#   * the main worktree keeps `./target`.
#
# GHOST BINARIES
# ==============
# cargo decides freshness by mtime and gives workspace members the same
# metadata hash in every tree, so an artefact built in tree A is handed back as
# fresh in tree B. `<target>/.gate-tree` records which tree filled the
# directory last; a mismatch means the sources that differ between the two
# trees must look newer than the artefacts. The long WHY -- why a foreign tree
# is always a FULL touch, and why a tests-only touch was an under-touch -- sits
# above the callers in `scripts/gate.sh`; the mechanism is here.
#
# The stamp is only as good as its weakest writer: a run that fills the shared
# target/ without writing it leaves the next one believing the directory was
# filled by whoever wrote last. Every script that starts cargo in the shared
# directory therefore syncs before and stamps at the first cargo call.

# `git status --porcelain=v1 -z` on stdin, one path per line.
#
# Porcelain is NOT one path per line. `sed 's/^...//'` over it was wrong twice:
# a rename or copy reads `R  old -> new`, so the arrow travelled INSIDE a single
# "path" all the way into the nextest filterset (`failed to parse filterset`,
# the tests station RED with 0 tests run, measured 2026-09-04); and a path with
# a space comes back quoted (`"two words.rs"`). `-z` has neither problem: every
# field is NUL-terminated and never quoted, and a rename/copy entry carries the
# ORIGIN as a second field right after the destination.
#
# Both halves of a rename are emitted, as separate paths. The old one is a
# deletion -- the resolver drops test targets that no longer exist by itself --
# and the new one is the file to touch and to select.
#
# $1 = "tracked" drops the untracked (`??`) entries -- `scripts/gate.sh` passes
# it for the narrower "would an export carry this" question; the reader below
# passes "all". Both callers pass one, so no shellcheck directive is needed
# here -- the file is checked as part of `scripts/*.sh` and comes back clean.
# The kit's one Python (`KIT_PY`, lane.PYTHON; re-check tv-bn M7t B-M4), beside this file.
# shellcheck source=scripts/pipeline/kit_python.sh
case "${BASH_SOURCE[0]}" in */*) _meclaw_ct_dir=${BASH_SOURCE[0]%/*}/ ;; *) _meclaw_ct_dir=./ ;; esac  # builtins only (M7w), see gate.sh
. "${_meclaw_ct_dir}pipeline/kit_python.sh" || exit 2

meclaw_porcelain_paths() {
    local only="${1:-all}" entry x path orig
    while IFS= read -r -d '' entry; do
        [ "${#entry}" -ge 4 ] || continue
        x=${entry:0:1}; path=${entry:3}
        orig=""
        case "$entry" in
            R*|C*|?R*|?C*) IFS= read -r -d '' orig || orig="" ;;
        esac
        [ "$only" = tracked ] && [ "$x" = "?" ] && continue
        printf '%s\n' "$path"
        [ -n "$orig" ] && printf '%s\n' "$orig"
    done
    return 0
}

# The full dirty list -- tracked AND untracked -- of the tree at $1, one path
# per line. It feeds the tree sync and the stamp, where the question is "which
# files must look newer than the artefacts".
meclaw_dirty_files() {   # <root>
    git -C "$1" status --porcelain=v1 -z --untracked-files=all 2>/dev/null \
        | meclaw_porcelain_paths all
}

# Settle on the target directory of the tree at $1.
#
# It SETS `MECLAW_TARGET_DIR` instead of printing it, because the linked case
# has a second effect -- the exported CARGO_TARGET_DIR -- and a command
# substitution would leave that behind in a subshell.
#
# `--git-common-dir` is `.git` in the main worktree and an absolute path to the
# main worktree's `.git` in a linked one -- that difference IS the test.
meclaw_resolve_target_dir() {   # <root>
    local root="$1" common
    if [ -n "${CARGO_TARGET_DIR:-}" ]; then
        MECLAW_TARGET_DIR="$CARGO_TARGET_DIR"
    elif [ "$(git -C "$root" rev-parse --git-common-dir 2>/dev/null || echo .git)" != ".git" ]; then
        common=$(git -C "$root" rev-parse --path-format=absolute --git-common-dir 2>/dev/null || echo "")
        if [ -n "$common" ]; then
            MECLAW_TARGET_DIR="$(dirname -- "$common")/target"
            export CARGO_TARGET_DIR="$MECLAW_TARGET_DIR"
        else
            MECLAW_TARGET_DIR="$root/target"
        fi
    else
        MECLAW_TARGET_DIR="$root/target"
    fi
}

# Touch every build input of every workspace member of the tree at $1 and print
# what was done. `find crates -name '*.rs'` covers src, tests, benches, examples
# and build.rs in one sweep; the manifests sit one level below `crates/`, and
# the root manifest plus the lock file are inputs to every member as well.
meclaw_full_touch() {   # <root> <reason>
    local root="$1" reason="$2" list n=0
    list=$( cd "$root" 2>/dev/null &&
            { find crates -type f -name '*.rs' 2>/dev/null
              find crates -mindepth 2 -maxdepth 2 -type f -name Cargo.toml 2>/dev/null
              [ -f Cargo.toml ] && printf 'Cargo.toml\n'
              [ -f Cargo.lock ] && printf 'Cargo.lock\n'
              true; } | sed '/^$/d' | sort -u)
    if [ -n "$list" ]; then
        n=$(printf '%s\n' "$list" | wc -l | tr -d ' ')
        ( cd "$root" && printf '%s\n' "$list" | tr '\n' '\0' | xargs -0 touch 2>/dev/null )
    fi
    printf 'full touch: %s, %s files' "$reason" "$n"
}

# Make the shared target/ safe for a build of the tree at $1, and print a note
# about what it took. An EMPTY note means there was nothing to do -- the stamp
# already names this tree at this commit.
meclaw_tree_sync() {   # <root> <target_dir> <rev> <dirty_files> [resync]
    local root="$1" target_dir="$2" rev="$3" dirty_files="$4" resync="${5:-0}"
    local stamp="$target_dir/.gate-tree" s_path s_sha s_dirty list n=0 f
    mkdir -p "$target_dir"
    if [ "$resync" = 1 ]; then
        meclaw_full_touch "$root" "--resync"
        return 0
    fi
    if [ ! -f "$stamp" ]; then
        meclaw_full_touch "$root" "no stamp"
        return 0
    fi
    s_path=$(sed -n 1p "$stamp"); s_sha=$(sed -n 2p "$stamp"); s_dirty=$(sed -n 3p "$stamp")
    if ! git -C "$root" rev-parse --verify --quiet "$s_sha^{commit}" >/dev/null 2>&1; then
        # A rebase, a pruned branch or a stamp from another repository:
        # `git diff <gone-sha>` fails and the union would be just the dirty
        # files -- a silent UNDER-touch, and ghost binaries survive it. The
        # only honest answer is the full touch.
        meclaw_full_touch "$root" "stale stamp ${s_sha:0:7}"
    elif [ "$s_path" != "$root" ]; then
        # Another worktree filled target/. Byte-identical sources are the
        # dangerous case, not the differing ones (GH #595).
        meclaw_full_touch "$root" "foreign tree $s_path"
    elif [ "$s_sha" != "$rev" ]; then
        list=$( { git -C "$root" diff --name-only "$s_sha" HEAD 2>/dev/null
                  printf '%s\n' "${s_dirty//,/$'\n'}"
                  printf '%s\n' "$dirty_files"; } | sed '/^$/d' | sort -u)
        while IFS= read -r f; do
            [ -n "$f" ] && [ -e "$root/$f" ] && touch "$root/$f" && n=$((n + 1))
        done <<<"$list"
        # On a lane every cargo run builds in the one build path of the lane
        # (`scripts/lane_sync.sh`), so the stamp names this path and only the
        # diff is touched -- the note says so by the lane's name.
        if [ -n "${MECLAW_LANE_NAME:-}" ]; then
            printf '%s files touched, own path (%s), target last built from %s@%s' \
                "$n" "$MECLAW_LANE_NAME" "$s_path" "${s_sha:0:7}"
        else
            printf '%s files touched, target last built from %s@%s' "$n" "$s_path" "${s_sha:0:7}"
        fi
    fi
    return 0
}

# Record which tree filled target/ last.
#
# The stamp says which tree last TOUCHED target/ -- a red or interrupted build
# fills it with artefacts just like a green one, and those artefacts are
# exactly what the next tree would be handed as fresh. So it is written when the
# FIRST cargo command is dispatched, before the call and regardless of what the
# call returns.
meclaw_write_stamp() {   # <root> <target_dir> <rev> <dirty_files>
    local root="$1" target_dir="$2" rev="$3" dirty_files="$4"
    mkdir -p "$target_dir"
    printf '%s\n%s\n%s\n' "$root" "$rev" \
        "$(printf '%s' "$dirty_files" | paste -sd, -)" >"$target_dir/.gate-tree"
}

# Are the build lanes of a wave armed on this host? (`scripts/strand.sh token
# init --lanes ...`, GH #942.) The token file lives beside the cargo lock --
# `${lock%.lock}.tokens`, or `MECLAW_STRAND_TOKENS` -- and an armed one names
# its lanes. No file, a broken one or one without lanes: not armed.
#
# The runner and the tier both refuse local cargo on this answer, and each
# carried its own copy of the probe, word for word (review of GH #942, M-2) --
# here once, like the target rule above, so the two cannot drift apart.
meclaw_lanes_active() {   # <cargo lock path>
    # Armed with lanes either way: the token file of the legacy kit, or the
    # pool of the pipeline store (`pipeline.py lanes init --lanes`, which
    # writes no token file -- C-C1 of the W3 review).
    local tokens="${MECLAW_STRAND_TOKENS:-${1%.lock}.tokens}"
    local db="${MECLAW_PIPELINE_DB:-$HOME/.local/state/meclaw/pipeline/pipeline.db}"
    # Z15 (M4): a store decision (init, off, migrate) at or after the file's
    # `armed` stamp outlives the file, as in the store's own migration.
    if [ -f "$tokens" ] && "${KIT_PY[@]}" -c 'import calendar, json, os, sqlite3, sys, time, urllib.parse
st = json.load(open(sys.argv[1])) or {}
if not st.get("lanes"):
    sys.exit(1)
def epoch(s):
    return calendar.timegm(time.strptime(s, "%Y-%m-%dT%H:%M:%SZ"))
# Review Z-A I1: an armed stamp that is no time falls back on the mtime (as
# in lease_migrate), and any other fault counts as armed (fail-closed).
try:
    armed = epoch(st["armed"])
except (KeyError, TypeError, ValueError):
    armed = int(os.stat(sys.argv[1]).st_mtime)
try:
    db = sqlite3.connect("file:%s?mode=ro" % urllib.parse.quote(sys.argv[2]), uri=True, timeout=5)
    rows = db.execute("SELECT at, data FROM journal WHERE event = ?", ("lease_noted",)).fetchall()
    for at, data in rows:
        verb = (json.loads(data or "{}").get("verb") or "").split(" ")[0]
        if verb in ("init", "off", "migrate") and epoch(at) >= armed:
            sys.exit(1)
except Exception:
    sys.exit(0)
sys.exit(0)' "$tokens" "$db" 2>/dev/null; then
        return 0
    fi
    [ -f "$db" ] || return 1
    # Fail-closed (Delta C M-neu-4): a store this probe cannot read, or a
    # lanes column that is no JSON, counts as armed; only a store without
    # the lease tables (never armed) is not.
    "${KIT_PY[@]}" -c 'import json, sqlite3, sys, urllib.parse
try:
    db = sqlite3.connect("file:%s?mode=ro" % urllib.parse.quote(sys.argv[1]), uri=True, timeout=5)
    row = db.execute("SELECT lanes FROM lease_pools WHERE pool = ? AND armed IS NOT NULL",
                     ("cargo",)).fetchone()
except sqlite3.OperationalError as exc:
    sys.exit(1 if str(exc).startswith(("no such table", "no such column")) else 0)
except sqlite3.Error:
    sys.exit(0)
try:
    lanes = json.loads(row[0] or "[]") if row else []
except (TypeError, ValueError):
    sys.exit(0)
sys.exit(0 if lanes else 1)' "$db" 2>/dev/null
}

# THE TEST-HOOK FEATURE
# =====================
# `meclaw-colony` carries a `test-hooks` feature: a crash-injection hook that
# parks a mutation between rename(2) and the spawn loop. The tests that use it
# (`#![cfg(feature = "test-hooks")]`, e.g. `tests/phase/phase_6_crash_recovery.rs`)
# compiled to empty stubs in every gate and in CI, because no test build turned
# the feature on (GH #1208). Every build of the gate and the tests -- the
# runner's build station with its `cargo build --bin meclaw`, clippy, the
# doctests, `scripts/test-tier.sh`, the CI test jobs, the coverage run
# (`scripts/pipeline/coverage_host.sh`, own target) -- carries it, so the
# shared target/ holds one variant and one `target/debug/meclaw`. The product
# binary (the release build, the export's cargo check) never does.
#
# `meclaw_test_features <root> [cargo args...]` prints the flags, one per line,
# or nothing. cargo refuses `--features meclaw-colony/test-hooks` for a
# selection in which no package depends on meclaw-colony ("does not contain
# this feature"), so the flag goes with `--workspace` (no `-p` given) and with
# a `-p` set that names meclaw-colony or a member with a direct dependency on
# it. The same answer for the same selection in every caller keeps one variant
# of meclaw-colony in the shared target/ instead of two.
MECLAW_TEST_FEATURE="meclaw-colony/test-hooks"
meclaw_test_features() {   # <root> [cargo args...]
    local root="$1" arg next="" any="" want=""
    shift
    for arg in "$@"; do
        if [ -n "$next" ]; then
            any=1
            meclaw_knows_test_hooks "$root" "$arg" && want=1
            next=""
            continue
        fi
        case "$arg" in
            -p|--package) next=1 ;;
            --package=*) any=1; meclaw_knows_test_hooks "$root" "${arg#--package=}" && want=1 ;;
            -p?*) any=1; meclaw_knows_test_hooks "$root" "${arg#-p}" && want=1 ;;
            --) break ;;
        esac
    done
    if [ -z "$any" ] || [ -n "$want" ]; then
        printf '%s\n' --features "$MECLAW_TEST_FEATURE"
    fi
}

meclaw_knows_test_hooks() {   # <root> <package>
    [ "$2" = meclaw-colony ] && return 0
    grep -qE '^[[:space:]]*meclaw-colony[[:space:]]*=' "$1/crates/$2/Cargo.toml" 2>/dev/null
}
