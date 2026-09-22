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
        printf '%s files touched, target last built from %s@%s' "$n" "$s_path" "${s_sha:0:7}"
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
