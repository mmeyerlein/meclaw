#!/usr/bin/env bash
# meclaw -- the transport of a gate onto a build host (a "lane").
#
#     scripts/lane_sync.sh files                          what the overlay sends (NUL-separated)
#     scripts/lane_sync.sh digest                         the overlay hash of this tree
#     scripts/lane_sync.sh push <ssh-target> <strand>     commit + overlay onto the host, prints `<rev> <hash> <browser>`
#     scripts/lane_sync.sh run <ssh-target> <strand> <mode> <base> [gate.sh options]
#     scripts/lane_sync.sh fetch <ssh-target> <strand> <dir>
#     scripts/lane_sync.sh test <ssh-target> <strand> <filterset> [nextest args]
#                                                         (golden switches: see `cmd_test`)
#     scripts/lane_sync.sh status <ssh-target>            one status word line for `strand.sh lanes status`
#
# `scripts/strand.sh gate --host <lane>` is the caller; nobody needs to run
# this by hand. It lives apart from the kit because it is the one part that
# talks to another machine, and the tests drive it through a fake `ssh` on
# PATH (GH #934).
#
# THE LAYOUT ON THE HOST, under `${MECLAW_LANE_ROOT:-/srv/target}` (a tmpfs
# on a build host, so everything below is gone after a reboot and the first
# push carries the whole history again):
#
#     meclaw.git        a bare repository the strands push into
#     wt/<strand>       one worktree per strand, checked out detached
#     target/           ONE cargo target directory for all strands of the host
#     runs/<strand>     receipt and station logs of the last run
#     cache/workshop-tools/node_modules
#                       the browser drivers' npm install, shared by every tree
#
# WHAT TRAVELS: exactly the tree a local gate would see. The commit travels
# by `git push` -- the history the host needs for the merge base comes with
# it. The uncommitted rest travels as an rsync overlay measured against
# HEAD, not against the index: every staged or unstaged change to a tracked
# file (`git diff --no-renames --name-only HEAD`), every untracked file that
# is not ignored (`git ls-files -o --exclude-standard`), and every deletion,
# staged or not (`--diff-filter=D`), is deleted there. The first cut listed
# `git ls-files -m -o` / `-d`, which compare against the INDEX: after a
# `git add` -- the normal state before a strand gates -- the host checked a
# different tree than the local gate would have (review I1).
#
# WHAT NEVER LEAVES THIS MACHINE: a file whose NAME says secret --
# `.env`, `.env.<anything>` except `*.example`, `vault.env`, `vault-pass*`,
# `.netrc`, `.git-credentials`, `.cargo/credentials*` (`secret_name()`).
# By name and nothing else: a path COMPONENT `vault` or `.env*` matched
# tracked source (`src/vault/*.rs`, `templates/vault/...`) and kept it at its
# committed state on the host (review I2). An untracked secret -- the `.env`
# link a linked worktree carries -- is left out silently; it is not part of
# the tree under test. A TRACKED secret is a collision: in HEAD it would
# leave by `git push`, changed it would leave by the overlay, and leaving it
# out would gate a different tree. Either way the push is refused with the
# path, before anything reaches the host (exit 5). A tracked `*.example` is
# a template without values and travels like any file.
#
# WHAT THE HOST GETS BESIDE THE TREE (GH #942), so that a lane runs every
# station and the owner's machine none:
#   * a `.env` in the host's tree, written THERE by `scripts/gate_plan.py
#     --print lane-env` -- names with constant non-secret placeholders
#     (a provider key name carries `lane-stub`), never derived from a file
#     of this machine, never a credential;
#   * the refs `github-main` and `master`, when this clone has them: the
#     export audit diffs against the first and a release exports the second;
#   * the URL of `origin` for the export audit's drift check, but only a plain
#     `https://` URL without credentials -- anything else stays here;
#   * `workshop/tools/node_modules`, the browser drivers' one npm install
#     (playwright): a link to the lane's cache, which the first tree of the
#     host that has a real install seeds. Measured 03.10. (OR-DP-83): the
#     install lived in one strand's tree, every other tree was fresh, and the
#     browser proofs there skipped without a word. The third word of `push`
#     says what the tree got -- `own` (its own install), `linked`, `none` (no
#     install on the host: the kit says so) or `n/a` (no drivers in the tree).
#
# THE OVERLAY HASH. After the overlay, both sides hash the same list -- HEAD,
# then per path its sha256 or `absent` -- and the push is refused when the
# two differ (exit 5): no remote gate starts on a tree that is not the local
# one. `push` prints the rev and that hash; the kit names it in its first line.
#
# Exit 0 = done. 2 = the host did not answer (the caller says
# "unreachable"). 5 = refused here: a secret collision or a different tree
# on the host (the caller names the path or the hashes). `run` exits with
# the remote runner's code (0/1/2/4), and 255 when ssh itself lost the host.

set -uo pipefail

ROOT="${MECLAW_LANE_ROOT:-/srv/target}"
SSH_OPTS=(-o BatchMode=yes -o "ConnectTimeout=${MECLAW_LANE_CONNECT_TIMEOUT:-10}")

die() { echo "lane_sync: $*" >&2; exit 2; }

q() { printf '%q' "$1"; }

# A file that holds a secret, by its name. No word splitting and no glob
# over the path (review M1): only `case` patterns on the string.
secret_name() {
    local base="${1##*/}"
    case "$base" in
        *.example) return 1 ;;
        .env|.env.*|vault.env|vault-pass*|.netrc|.git-credentials) return 0 ;;
    esac
    case "/$1" in
        */.cargo/credentials|*/.cargo/credentials.*) return 0 ;;
    esac
    return 1
}

# The lists, NUL-separated, paths from the top of the tree. `--no-renames`:
# a rename is a deletion plus a new file, and both must reach the host.
changed_tracked() { git diff --no-renames --name-only -z --diff-filter=d HEAD --; }
deleted_tracked() { git diff --no-renames --name-only -z --diff-filter=D HEAD --; }
untracked() { git ls-files -z -o --exclude-standard; }

# Tracked secrets: in the commit (they would leave by the push) or changed
# against it (they would leave by the overlay). One path per line.
collisions() {
    local f
    { git ls-tree -r -z --name-only HEAD; changed_tracked; } | while IFS= read -r -d '' f; do
        secret_name "$f" && printf '%s\n' "$f"
    done | sort -u
}

overlay_files() {
    local f
    changed_tracked | while IFS= read -r -d '' f; do
        secret_name "$f" || printf '%s\0' "$f"
    done
    untracked | while IFS= read -r -d '' f; do
        secret_name "$f" || printf '%s\0' "$f"
    done
}

deleted_files() { deleted_tracked; }

# The overlay hash: run HERE and THERE over the same list on stdin
# (overlay plus deletions, sorted), in the tree, by `bash -c`.
# shellcheck disable=SC2016
DIGEST='{ git rev-parse HEAD; while IFS= read -r -d "" f; do
    if [ -L "$f" ]; then printf "link %s %s\n" "$f" "$(readlink -- "$f")"
    elif [ -e "$f" ]; then sha256sum -- "$f"
    else printf "absent  %s\n" "$f"; fi
done; } | sha256sum | cut -c1-16'

# The content of the overlay files as a tree holds them, run HERE and THERE
# over the overlay list on stdin, by `bash -c`: one NUL-terminated record
# `<sha256> <path>` per regular file, nothing for a link or an absent file.
# shellcheck disable=SC2016
HASHES='while IFS= read -r -d "" f; do
    if [ -f "$f" ] && [ ! -L "$f" ]; then
        printf "%s %s\0" "$(sha256sum <"$f" | cut -c1-64)" "$f"
    fi
done'

digest_list() {   # the list both sides hash
    { overlay_files; deleted_files; } | sort -z -u
}

# The browser drivers of a tree on the host, run THERE by `bash -c` with the
# tree and the lane root as arguments; prints own|linked|none|n/a.
# shellcheck disable=SC2016
BROWSER_DEPS='wt=$1; root=$2
nm="$wt/workshop/tools/node_modules"; cache="$root/cache/workshop-tools/node_modules"
[ -d "$wt/workshop/tools" ] || { echo n/a; exit 0; }
if [ -d "$nm/playwright" ] && [ ! -L "$nm" ]; then echo own; exit 0; fi
if [ ! -d "$cache/playwright" ]; then
    for seed in "$root"/wt/*/workshop/tools/node_modules; do
        if [ -d "$seed/playwright" ] && [ ! -L "$seed" ]; then
            # A copy of its own, moved in only while the cache is still absent: two
            # pushes on one host at once would otherwise move one copy INTO the other.
            tmp="$cache.tmp.$$"
            mkdir -p "${cache%/*}" && rm -rf "$tmp" && cp -a "$seed" "$tmp" \
                && { [ -d "$cache" ] || mv -T "$tmp" "$cache"; }; rm -rf "$tmp"
            break
        fi
    done
fi
if [ -d "$cache/playwright" ]; then
    [ -L "$nm" ] || rm -rf "$nm"
    ln -sfn "$cache" "$nm" && echo linked
else
    echo none
fi'

# `MECLAW_LANE_THREADS=<n>` (strand.sh: `threads=<n>` of the lane in the host
# file) caps the test threads and the build jobs on the host; unset or not a
# positive number, the host runs at full width. Prints the environment words.
lane_width() {
    case "${MECLAW_LANE_THREADS:-}" in
        ''|*[!0-9]*|0) return 0 ;;
        *) printf 'NEXTEST_TEST_THREADS=%s CARGO_BUILD_JOBS=%s ' "$MECLAW_LANE_THREADS" "$MECLAW_LANE_THREADS" ;;
    esac
}

remote() {   # target, command string
    local target="$1"; shift
    # SC2029: the command string is built HERE on purpose, every value in it
    # quoted with `q` -- the host runs exactly what this side composed.
    # shellcheck disable=SC2029
    ssh "${SSH_OPTS[@]}" "$target" "$*"
}

cmd_push() {
    local target="$1" strand="$2" sha wt top
    top=$(git rev-parse --show-toplevel 2>/dev/null) || die "not a git repository"
    cd "$top" || die "cannot enter $top"
    sha=$(git rev-parse HEAD) || die "no HEAD to push"
    wt="$ROOT/wt/$strand"

    # Before anything reaches the host: no tracked secret (exit 5, with the
    # paths). Fail closed -- never send it, never gate without it.
    local hits
    hits=$(collisions)
    if [ -n "$hits" ]; then
        echo "lane_sync: refused -- tracked secret file(s), they would leave this machine:" >&2
        printf '  %s\n' "$hits" >&2
        exit 5
    fi

    # The bare repository; a fresh host (tmpfs) has none.
    remote "$target" "mkdir -p $(q "$ROOT/wt") && { test -d $(q "$ROOT/meclaw.git") || git init -q --bare $(q "$ROOT/meclaw.git"); }" \
        || die "the host $target did not answer"
    # The commit, and the refs the export audit reads (GH #942).
    local refs=("$sha:refs/heads/lane/$strand") ref url
    for ref in github-main master; do
        git rev-parse --verify --quiet "refs/heads/$ref" >/dev/null \
            && refs+=("refs/heads/$ref:refs/heads/$ref")
    done
    GIT_SSH_COMMAND="ssh ${SSH_OPTS[*]}" \
        git push -q --force "$target:$ROOT/meclaw.git" "${refs[@]}" \
        || die "git push to $target failed"
    url=$(git remote get-url origin 2>/dev/null || true)
    case "$url" in
        https://*@*|*[[:space:]]*) url="" ;;
        https://*) ;;
        *) url="" ;;
    esac
    if [ -n "$url" ]; then
        remote "$target" "git --git-dir=$(q "$ROOT/meclaw.git") config remote.origin.url $(q "$url")" \
            || die "the host $target did not answer"
    fi

    # GH #1071: what the lane last built from, before the checkout rewrites
    # it -- the content of every overlay path in the host's tree as it stands.
    # rsync `-a` carries the local mtime; a file edited HERE while the lane
    # still compiled the previous sync arrived with a time from BEFORE that
    # build, and cargo kept the stale artefact (measured 07.10.: three edited
    # test files, a lane verdict on code that was no longer under test).
    local list pre post
    list=$(mktemp) || die "no temp file"
    pre=$(mktemp) || die "no temp file"
    post=$(mktemp) || die "no temp file"
    overlay_files >"$list"
    remote "$target" "cd $(q "$wt") 2>/dev/null && bash -c $(q "$HASHES") || true" <"$list" >"$pre" \
        || { rm -f "$list" "$pre" "$post"; die "the host $target did not answer"; }

    # The worktree: checked out at the commit, cleaned of the last overlay.
    # `git clean` without -x keeps the ignored files a run left behind; the
    # target directory lives outside the worktree anyway.
    remote "$target" "set -e
        if git -C $(q "$wt") rev-parse --git-dir >/dev/null 2>&1; then
            git -C $(q "$wt") checkout -q --force --detach $sha
            git -C $(q "$wt") clean -fdq
        else
            rm -rf $(q "$wt")
            git --git-dir=$(q "$ROOT/meclaw.git") worktree prune
            git --git-dir=$(q "$ROOT/meclaw.git") worktree add -q --force --detach $(q "$wt") $sha
        fi" >/dev/null || die "the checkout on $target failed"

    # The overlay: what is not committed yet.
    if [ -s "$list" ]; then
        # `--checksum`: rsync's quick check (size + mtime) skipped a change
        # of equal size when the host's checkout fell into the same second
        # as the local write -- measured on the first build host, the
        # overlay hash below refused it. The overlay is small; compare content.
        rsync -a --checksum --from0 --files-from="$list" -e "ssh ${SSH_OPTS[*]}" ./ "$target:$wt/" \
            || { rm -f "$list" "$pre" "$post"; die "the overlay to $target failed"; }
        # GH #1071: a file whose content the host did not have before this
        # sync gets the host's time, so it is never older than the host's last
        # build; a file the host already had keeps the carried time, so an
        # unchanged tree rebuilds nothing. The overlay hash below compares
        # content only, the stamp does not touch it.
        bash -c "$HASHES" <"$list" >"$post"
        LC_ALL=C comm -z -23 <(LC_ALL=C sort -z "$post") <(LC_ALL=C sort -z "$pre") | LC_ALL=C cut -z -c66- \
            | remote "$target" "cd $(q "$wt") && xargs -0 -r touch -c --" \
            || { rm -f "$list" "$pre" "$post"; die "the stamp on $target failed"; }
    fi
    rm -f "$pre" "$post"
    deleted_files >"$list"
    if [ -s "$list" ]; then
        remote "$target" "cd $(q "$wt") && xargs -0 rm -f --" <"$list" \
            || { rm -f "$list"; die "the deletions on $target failed"; }
    fi
    # The secret-free `.env` of a lane, written there from the tree's own table
    # (`gate_plan.py --print lane-env`). Not part of the overlay hash: its
    # name says secret, so it is in neither list on either side.
    remote "$target" "cd $(q "$wt") && python3 scripts/gate_plan.py --print lane-env >.env" \
        || { rm -f "$list"; die "the lane env on $target failed"; }
    # The browser drivers (OR-DP-83). Ignored by git, so in neither hash list.
    local browser
    browser=$(remote "$target" "bash -c $(q "$BROWSER_DEPS") _ $(q "$wt") $(q "$ROOT")") \
        || { rm -f "$list"; die "the browser drivers on $target failed"; }
    # The same tree on both sides, or no remote gate (exit 5).
    local here there
    digest_list >"$list"
    here=$(bash -c "$DIGEST" <"$list")
    there=$(remote "$target" "cd $(q "$wt") && bash -c $(q "$DIGEST")" <"$list") \
        || { rm -f "$list"; die "the overlay hash on $target failed"; }
    rm -f "$list"
    if [ -z "$here" ] || [ "$here" != "$there" ]; then
        echo "lane_sync: refused -- the overlay on $target is not this tree (here ${here:-?}, there ${there:-?})" >&2
        exit 5
    fi
    printf '%s %s %s\n' "$sha" "$here" "${browser:-none}"
}

cmd_run() {
    local target="$1" strand="$2" mode="$3" base="$4"; shift 4
    local wt="$ROOT/wt/$strand" runs="$ROOT/runs/$strand" args="" a width
    width=$(lane_width)
    for a in "$@"; do args+=" $(q "$a")"; done
    # BLOCKING: the caller waits for the verdict like for a local run. A
    # non-interactive ssh shell does not read the profile that puts rustup's
    # `~/.cargo/bin` on PATH, so the command puts it there itself (measured on
    # the first build host: `cargo: command not found`).
    # shellcheck disable=SC2029
    ssh "${SSH_OPTS[@]}" -o ServerAliveInterval=60 "$target" \
        "export PATH=\"\$HOME/.cargo/bin:\$PATH\" && cd $(q "$wt") && rm -rf $(q "$runs") && mkdir -p $(q "$runs") && CARGO_TARGET_DIR=$(q "$ROOT/target") ${width}scripts/gate.sh $(q "$mode") --lane --base $(q "$base") --log-dir $(q "$runs")$args"
}

# `test`: one nextest filterset in the strand's tree on the host, through the
# tier script -- the same lock, target directory and ghost-binary guard as
# every tier run there. `MECLAW_TIER_LANE` tells the tier it runs ON a lane
# (the lanes check is for the owner's machine), and a build host runs no
# colony, so the tests run at full width. Exit = the remote exit, 255 when
# ssh lost the host; 2 when goldens the run wrote did not come back.
#
# KEEP GOING (GH #1050): a test run is there to name every red test of the
# filterset. `--no-fail-fast` goes along unless the caller asked for the
# opposite (`--fail-fast`, `--max-fail`): measured on a lane, a filterset of
# six locks with five expected red named one per run, a different one each
# time, and the fix took two runs of four and eight minutes to see.
#
# GOLDEN SWITCHES (GH #1051): ssh does not carry the caller's environment, so
# a test that rewrites its goldens under a switch never saw the switch on the
# lane, and what it wrote there stayed there (the owner's builder copied it
# back by hand). The switches below travel on the command line when they are
# set here -- an ALLOWLIST by exact name, never a pattern: a pattern over the
# environment is how a key or a token would leave this machine. When one
# travelled, the golden files the run changed come back into this tree.
GOLDEN_SWITCHES=(GH277_WRITE_GOLDEN MECLAW_GH935_BLESS)

# What counts as a golden: a tracked or new (not ignored) file under a
# `fixtures/` or `golden/` directory. Hashed THERE before and after the run;
# a file whose hash moved is what the run wrote. Nothing else comes back.
# NUL-terminated records `<sha256>  <path>` (`sha256sum -z` escapes no name;
# without it a backslash in a name moved the path one column, review N5).
# shellcheck disable=SC2016
GOLDEN_HASH='spec=(":(glob)**/fixtures/**" ":(glob)**/golden/**")
{ git ls-files -z -- "${spec[@]}"; git ls-files -z -o --exclude-standard -- "${spec[@]}"; } \
    | xargs -0 -r sha256sum -z -- 2>/dev/null | LC_ALL=C sort -z; true'

cmd_test() {
    local target="$1" strand="$2"; shift 2
    local wt="$ROOT/wt/$strand" args="" a envs="" name keep=1 first=1 tail=0 binargs="" width
    width=$(lane_width)
    width=${width% }
    # SC2016: the HOST expands `$(nproc)`, on purpose.
    # shellcheck disable=SC2016
    [ -n "$width" ] || width='NEXTEST_TEST_THREADS=$(nproc)'
    for a in "$@"; do
        # the first word is the filterset, never an option
        if [ "$first" = 1 ]; then first=0; args+=" $(q "$a")"; continue; fi
        # behind a `--` the words belong to the test binary: `--no-fail-fast`
        # goes before it, and a `--fail-fast` there is not nextest's (review N3)
        if [ "$tail" = 1 ]; then binargs+=" $(q "$a")"; continue; fi
        if [ "$a" = -- ]; then tail=1; binargs+=" --"; continue; fi
        args+=" $(q "$a")"
        case "$a" in --fail-fast|--no-fail-fast|--max-fail|--max-fail=*) keep=0 ;; esac
    done
    [ "$keep" = 1 ] && args+=" --no-fail-fast"
    args+="$binargs"
    for name in "${GOLDEN_SWITCHES[@]}"; do
        [ -n "${!name+x}" ] && envs+=" $name=$(q "${!name}")"
    done
    local before="" rc
    if [ -n "$envs" ]; then
        before=$(mktemp) || die "no temp file"
        remote "$target" "cd $(q "$wt") && bash -c $(q "$GOLDEN_HASH")" >"$before" \
            || { rm -f "$before"; die "the golden hashes on $target failed"; }
    fi
    # shellcheck disable=SC2029
    ssh "${SSH_OPTS[@]}" -o ServerAliveInterval=60 "$target" \
        "export PATH=\"\$HOME/.cargo/bin:\$PATH\" && cd $(q "$wt") && CARGO_TARGET_DIR=$(q "$ROOT/target") MECLAW_TIER_LANE=1 ${width}$envs scripts/test-tier.sh filter$args"
    rc=$?
    [ -n "$envs" ] || return "$rc"
    if [ "$rc" = 255 ]; then rm -f "$before"; return "$rc"; fi
    golden_back "$target" "$wt" "$before" || { rm -f "$before"; return 2; }
    rm -f "$before"
    return "$rc"
}

# `golden_back <target> <wt> <hash file before>`: the goldens the run changed,
# from the host's tree into this one, each named on stderr (the kit lifts the
# lines out of the test log). The host's tree held this tree's state when the
# run began (commit + overlay), so a golden whose file HERE no longer matches
# the hash before was edited here during the run: it stays, the line names it
# as a conflict, the exit is 1 -- the lane's copy never overwrites it.
golden_back() {
    local target="$1" wt="$2" before="$3" after top list f rec h rc=0
    local -A had=()
    top=$(git rev-parse --show-toplevel 2>/dev/null) || die "not a git repository"
    after=$(mktemp) || die "no temp file"
    remote "$target" "cd $(q "$wt") && bash -c $(q "$GOLDEN_HASH")" >"$after" \
        || { rm -f "$after"; echo "lane_sync: golden fetch failed -- no hashes from $target" >&2; return 1; }
    while IFS= read -r -d '' rec; do had["${rec:66}"]="${rec:0:64}"; done <"$before"
    list=$(mktemp) || die "no temp file"
    while IFS= read -r -d '' rec; do
        f="${rec:66}"
        secret_name "$f" && continue
        h=""
        [ -f "$top/$f" ] && h=$(sha256sum <"$top/$f" | cut -c1-64)
        if [ "$h" != "${had["$f"]:-}" ]; then
            echo "lane_sync: golden conflict: $f -- changed here during the run, kept;" \
                "the lane's copy stays on $target" >&2
            rc=1
            continue
        fi
        printf '%s\0' "$f"
    done < <(LC_ALL=C comm -z -13 "$before" "$after") >"$list"
    if [ -s "$list" ]; then
        rsync -a --from0 --files-from="$list" -e "ssh ${SSH_OPTS[*]}" "$target:$wt/" "$top/" \
            || { rm -f "$list" "$after"; echo "lane_sync: golden fetch failed -- rsync from $target" >&2; return 1; }
        while IFS= read -r -d '' f; do echo "lane_sync: golden back: $f" >&2; done <"$list"
    elif [ "$rc" = 0 ]; then
        echo "lane_sync: golden back: none -- the run changed no golden" >&2
    fi
    rm -f "$list" "$after"
    return "$rc"
}

cmd_fetch() {
    local target="$1" strand="$2" dest="$3"
    mkdir -p "$dest" || die "cannot create $dest"
    rsync -a -e "ssh ${SSH_OPTS[*]}" "$target:$ROOT/runs/$strand/" "$dest/" \
        || die "could not fetch the receipt from $target"
}

cmd_status() {
    local target="$1" out
    # Line 1: the file system type and the free G of the lane root -- a tmpfs
    # until the host got its disk, then a real file system (GH #942). Line 2:
    # `bare` = the number of strand refs the host holds, `none` without a repo.
    out=$(remote "$target" "df -BG --output=fstype,avail $(q "$ROOT") 2>/dev/null | tail -1; echo; if test -d $(q "$ROOT/meclaw.git"); then git --git-dir=$(q "$ROOT/meclaw.git") for-each-ref refs/heads/lane | wc -l; else echo none; fi" 2>/dev/null) \
        || { echo "unreachable"; return 0; }
    local fs free bare
    fs=$(printf '%s\n' "$out" | sed -n 1p | awk '{print $1}')
    free=$(printf '%s\n' "$out" | sed -n 1p | awk '{print $2}' | tr -dc 0-9)
    bare=$(printf '%s\n' "$out" | sed -n '2,$p' | sed '/^$/d' | head -1 | tr -d ' ')
    [ "$bare" = none ] || bare="$bare refs"
    printf 'reachable  fs %s  free %sG  bare %s\n' "${fs:-?}" "${free:-?}" "$bare"
}

[ $# -ge 1 ] || die "expected files|digest|push|run|test|fetch|status"
sub="$1"; shift
case "$sub" in
    files)  cd "$(git rev-parse --show-toplevel 2>/dev/null)" || die "not a git repository"
            overlay_files ;;
    digest) cd "$(git rev-parse --show-toplevel 2>/dev/null)" || die "not a git repository"
            digest_list | bash -c "$DIGEST" ;;
    push)   [ $# -eq 2 ] || die "push <ssh-target> <strand>"; cmd_push "$@" ;;
    run)    [ $# -ge 4 ] || die "run <ssh-target> <strand> <mode> <base> [options]"; cmd_run "$@" ;;
    test)   [ $# -ge 3 ] || die "test <ssh-target> <strand> <filterset> [nextest args]"; cmd_test "$@" ;;
    fetch)  [ $# -eq 3 ] || die "fetch <ssh-target> <strand> <dir>"; cmd_fetch "$@" ;;
    status) [ $# -eq 1 ] || die "status <ssh-target>"; cmd_status "$@" ;;
    *) die "unknown subcommand: $sub" ;;
esac
