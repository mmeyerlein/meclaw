#!/bin/bash -p
# meclaw -- the transport of a gate onto a build host (a "lane").
#
#     scripts/lane_sync.sh files                          what the overlay sends (NUL-separated)
#     scripts/lane_sync.sh digest                         the overlay hash of this tree
#     scripts/lane_sync.sh push <ssh-target> <strand>     commit + overlay onto the host, prints `<rev> <hash> <browser>`
#     scripts/lane_sync.sh fetch <ssh-target> <strand> <dir>
#     scripts/lane_sync.sh check <ssh-target> <strand> [cargo check args]
#                                                         (the cargo check of `strand.sh pre`, W4)
#     scripts/lane_sync.sh coverage <ssh-target> <strand> <deadline-s>   the instrumented run in `coverage/{tree,target}` (W6)
#     scripts/lane_sync.sh test <ssh-target> <strand> <filterset> [nextest args]
#                                                         (golden switches: see `cmd_test`)
#     scripts/lane_sync.sh status <ssh-target>            one status line for `strand.sh lanes status`
#     scripts/lane_sync.sh fingerprint <ssh-target>       the lane's fingerprint, one JSON line
#     scripts/lane_sync.sh browsers <ssh-target> check|install [<engine>...]
#                                                         the system packages of a browser engine
#                                                         from the PINNED playwright (GH #1161)
#     scripts/lane_sync.sh env <mode>                     the environment words a run of <mode> gets
#     scripts/lane_sync.sh paths <strand>                 build path and target of a run, one per line
#     scripts/lane_sync.sh guard <ssh-target> <strand>    the guards of a run alone, then `paths`
#     scripts/lane_sync.sh hygiene [--kit] <ssh-target> <command> [args]
#                                                         `scripts/pipeline/hygiene.py <command>` on the host
#     scripts/lane_sync.sh checked ssh|rsync|git          the program the narrow point admits, its absolute path
#
# The pipeline (`scripts/pipeline/lane.py`, `pre.py`) and `scripts/strand.sh`
# are the callers; nobody needs to run this by hand. The verb `run` -- the old
# runner on a host for `strand.sh gate --host` -- is gone with that branch
# (GH #1156). It lives apart from the kit because it is the one part that
# talks to another machine, and the tests drive it through a fake `ssh` on
# PATH (GH #934).
#
# THE LAYOUT ON THE HOST, under `${MECLAW_LANE_ROOT:-/srv/target}` (a
# persistent disk on every build host since GH #942: a reboot is warm, the
# target, its fingerprints and the incremental sessions survive it):
#
#     meclaw.git        a bare repository the strands push into
#     tree/             THE build path of the lane: every run with a cargo
#                       station checks its commit out HERE (one path per lane)
#     target/           the cargo target directory of `tree/`
#     slot/<n>/{tree,target}
#                       further build paths with their own target, only when
#                       the host file grants `slots=<n>` (default 1 = none)
#     wt/<strand>       one worktree per strand, for runs WITHOUT a cargo
#                       station (Python stations, labs without a build)
#     runs/<strand>     receipt and station logs of the last run -- always by
#                       the strand's name, never by the path it built in
#     sccache/          the compile cache, when the lane runs sccache
#     kit/<sha8>/       a copy of `hygiene.py` that a timer on the host can call
#                       (and the core's pipeline kit, lane.push_kit); the
#                       hygiene's collect removes a kit after 24 h unless a
#                       unit or a process still names it (`hygiene.py kits`)
#     bench/<name>/     a managed bench or comparison build (`strand.sh bench`)
#     cache/workshop-tools/node_modules
#                       the browser drivers' npm install, shared by every tree
#
# ONE BUILD PATH PER LANE. Cargo embeds the source path of a member
# (`env!("CARGO_MANIFEST_DIR")`, file names in debug info), so the same
# sources in another directory are another build -- and the shared target
# had to FULL-touch every member whenever a different strand's worktree had
# filled it last (`full touch: foreign tree`, GH #595; measured 1 354 s
# against 817 s warm for a release `tests` station). Checking every strand's
# commit out in ONE directory makes the stamp `.gate-tree` always name that
# directory, and `meclaw_tree_sync` touches only `git diff <stamp> HEAD` plus
# the dirty files. Two cargo runs in one build path at once are an error; the
# lane token keeps one per lane, a slot is the way to a second.
#
# THE TRANSPORT. `<ssh-target>` is `user@host`, or the word `local` for a
# lane on this machine (`<name> local <root> class=egon ...` in the host
# file): no ssh, no network -- the same steps run in a local shell, the bare
# repository borrows this clone's objects (`objects/info/alternates`) and the
# overlay is a local rsync.
#
# THE CLASS of a lane (`class=` in the host file, `MECLAW_LANE_CLASS`): a build
# host runs at full width (or `threads=<n>`); class `egon` is the owner's
# machine beside a live colony -- `nice -n 19 ionice -c3`, build jobs at most
# half the cores, 4 test threads, the cargo lock of the machine, and a
# watchdog trip of the watched unit (`watch=<unit>`) aborts the run. A class
# `egon` lane runs `test` only; a gate there is refused.
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
# `provider.env`, `.netrc`, `.git-credentials`, `.cargo/credentials*`
# (`secret_name()`; the same rule as `scripts/pipeline/register.py`).
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
# THE LANE HYGIENE (W2 of the dev-tempo wave). One day (08.10.2026) four
# disks ran full -- a linker `Bus error` at 100 %, bench leftovers of 124 G,
# five generations of test binaries in one `target/debug/deps` (244 G) -- and
# every one was found by hand, after the run that it cost. So `run` asks the
# host BEFORE the gate whether there is room: `hygiene.py check` clears
# towards the lane's floor (`MECLAW_LANE_FLOOR_G`, default 60 -- the one
# floor, the same `lane_floor` refuses at) plus what the build is expected to
# need (`MECLAW_LANE_EXPECTED_G`, default 35). The check collects leftovers,
# old artifact generations and other slots' warm targets on its own -- never
# the run's own target, unless `MECLAW_LANE_WIPE_OK` is set; when it is still
# below the floor it says `aborted` (its exit 2), the gate does not start and
# `run` exits 6. Floor 0 and `MECLAW_LANE_WIPE_OK` switch the abort off. After the run, `hygiene.py collect` sweeps what the run
# left behind. Both travel the way every hygiene call does: the script goes
# over ssh on stdin (`python3 - <command>`), so a host never needs a tree of
# its own to be checked, and the version that checks it is always the one of
# the tree under test. Their JSONL events go to stderr -- the caller's log,
# never the gate's stdout lines. A tree without `scripts/pipeline/hygiene.py`
# skips both with a note; `MECLAW_HYGIENE=0` switches both off (the way
# back). A check that fails on its own (not exit 2) is reported and the run
# goes on to `lane_floor`, which still refuses below the floor (exit 2) --
# the hygiene never makes a lane worse than it was before it existed.
#
# Exit 0 = done. 2 = the host did not answer (the caller says
# "unreachable"), or it has less than the floor free (`MECLAW_LANE_FLOOR_G`,
# 60 G: the line says to clear old generations, W2), or a class `egon` run
# was aborted by a watchdog trip of the watched unit (a `watchdog_fired`
# line). 5 = refused here: a secret collision or a different tree
# on the host (the caller names the path or the hashes). `run` exits with
# the remote runner's code (0/1/2/4), and 255 when ssh itself lost the host. 6 = `run` only: the host's hygiene check found no room
# for the run, the gate never started. `hygiene` exits with the script's
# code (0 ok, 2 aborted), 3 when this tree has no
# `scripts/pipeline/hygiene.py`, 255 when ssh lost the host.

# One group, parsed whole before its first command runs: an in-place edit of this file under a running shell changes nothing that runs (GH #1223).
{
set -uo pipefail
# A trace of the run (`set -x`) for a test that waits on one of its steps
# (`+ stop_watcher`): the privileged shell ignores SHELLOPTS from the caller.
# The prompt of the trace is fixed here, never the caller's: bash expands PS4
# before every traced line, and a PS4 from the environment (`$((ok=1))`) set
# the admission inside `tool_bin` and ran a hand fake (re-check tv-bn M7m M1).
if [ "${MECLAW_LANE_XTRACE:-}" = 1 ]; then
    PS4='+ '
    unset BASH_XTRACEFD
    set -x
fi

ROOT="${MECLAW_LANE_ROOT:-/srv/target}"
# This file's directory with builtins only: an external `dirname` missing from
# PATH made it the caller's cwd and the kit library came from there (M7w).
case "${BASH_SOURCE[0]}" in */*) KIT_HERE=${BASH_SOURCE[0]%/*}/ ;; *) KIT_HERE=./ ;; esac
KIT_HERE=$(cd -- "$KIT_HERE" && pwd) || exit 2
HYGIENE="$KIT_HERE/pipeline/hygiene.py"
# The kit's one Python (`KIT_PY`, `kit_py_q` in remote command strings; M7t B-M4).
# shellcheck source=scripts/pipeline/kit_python.sh
. "$KIT_HERE/pipeline/kit_python.sh" || exit 2
SSH_OPTS=(-o BatchMode=yes -o "ConnectTimeout=${MECLAW_LANE_CONNECT_TIMEOUT:-10}")

die() { echo "lane_sync: $*" >&2; exit 2; }

q() { printf '%q' "$1"; }

# A file that holds a secret, by its name. No word splitting and no glob
# over the path (review M1): only `case` patterns on the string.
secret_name() {
    local base="${1##*/}"
    case "$base" in
        *.example) return 1 ;;
        .env|.env.*|vault.env|vault-pass*|provider.env|.netrc|.git-credentials) return 0 ;;
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
    done | LC_ALL=C sort -u
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
    { overlay_files; deleted_files; } | LC_ALL=C sort -z -u   # byte order, as lane.py (M7r B-M1)
}

# The browser drivers of a tree on the host, run THERE by `bash -c` with the
# tree and the lane root as arguments; prints own|linked|none|n/a.
# shellcheck disable=SC2016
BROWSER_DEPS='wt=$1; root=$2
nm="$wt/workshop/tools/node_modules"; cache="$root/cache/workshop-tools/node_modules"
[ -d "$wt/workshop/tools" ] || { echo n/a; exit 0; }
if [ -d "$nm/playwright" ] && [ ! -L "$nm" ]; then echo own; exit 0; fi
if [ ! -d "$cache/playwright" ]; then
    for seed in "$root"/tree/workshop/tools/node_modules "$root"/slot/*/tree/workshop/tools/node_modules \
                "$root"/wt/*/workshop/tools/node_modules; do
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

posint() { case "${1:-}" in ''|*[!0-9]*|0) return 1 ;; esac; return 0; }

# The settings of the lane, from the host file through strand.sh (never a
# name in here -- the host file is the one truth, the hosts get renamed).
lane_class() { case "${MECLAW_LANE_CLASS:-}" in egon) echo egon ;; *) echo box ;; esac; }
lane_slots() { if posint "${MECLAW_LANE_SLOTS:-}"; then echo "$MECLAW_LANE_SLOTS"; else echo 1; fi; }

# The slot of this run: 0 = `tree/` + `target/`, n >= 1 = `slot/<n>/`. A slot
# the host file does not grant is refused (exit 5) -- never a second cargo run
# in a build path that is in use.
lane_slot() {
    local s="${MECLAW_LANE_SLOT:-0}" n
    n=$(lane_slots)
    case "$s" in ''|*[!0-9]*) echo "lane_sync: refused -- slot '$s' is not a number" >&2; exit 5 ;; esac
    if [ "$s" -ge "$n" ]; then
        echo "lane_sync: refused -- slot $s is not granted on this lane (slots=$n in the host file)" >&2
        exit 5
    fi
    echo "$s"
}

# Where a run of <strand> builds: the build path of the slot for a run with a
# cargo station (`MECLAW_LANE_CARGO`, default 1), `wt/<strand>` for one
# without. The receipts never take their name from this path. The coverage
# run (`LANE_COVERAGE=1`, a shell variable only the verb `coverage` sets --
# never read from the environment, GH #1133 M3) has its own
# layout element `coverage/{tree,target}`: instrumented artefacts never meet
# the warm strand target, and the hygiene counts it as a warm target.
LANE_COVERAGE=0

build_path() {   # <strand>
    local s
    if [ "$LANE_COVERAGE" = 1 ]; then echo "$ROOT/coverage/tree"; return 0; fi
    if [ "${MECLAW_LANE_CARGO:-1}" = 0 ]; then echo "$ROOT/wt/$1"; return 0; fi
    s=$(lane_slot) || exit 5
    if [ "$s" = 0 ]; then echo "$ROOT/tree"; else echo "$ROOT/slot/$s/tree"; fi
}

target_path() {
    local s
    if [ "$LANE_COVERAGE" = 1 ]; then echo "$ROOT/coverage/target"; return 0; fi
    s=$(lane_slot) || exit 5
    if [ "$s" = 0 ]; then echo "$ROOT/target"; else echo "$ROOT/slot/$s/target"; fi
}

# The environment of a run on the lane, as words for the remote command line.
#   * CARGO_INCREMENTAL: on in strand and test runs, off in the pass runs
#     (`main-pass`, `m0`): deterministic and smaller. Integration and release
#     keep cargo's default -- a flag flip rebuilds every member in the shared
#     target, twice (OR-DT.W1.1).
#   * jobs and test threads: class `egon` half the cores, never over 4
#     (`$(nproc)` is expanded THERE); a build host at full width or
#     its `threads=<n>` -- full width names `NEXTEST_TEST_THREADS=$(nproc)`
#     for the host to expand (`lane.env_for` puts the lane's vCPUs in).
#   * sccache (`sccache=1`, MECLAW_LANE_SCCACHE) and a linker (`linker=mold|
#     wild`, MECLAW_LANE_LINKER) only where the host file grants them, never in
#     `.cargo/config.toml` (the export and CI build with the toolchain's own).
#     RUSTFLAGS replaces the config's `rustflags`, so `--cfg tokio_unstable`
#     goes along.
#
# VERDICT ENV (GH #1210): ssh carries none of the caller's environment, so a
# name that changes a test's verdict travels as a word of the command line --
# this list, an ALLOWLIST by exact name (never a pattern: a pattern over the
# environment is how a key would leave this machine), set and non-empty here.
#   MECLAW_ALLOW_BUMP            precheck's version-lock (the merge point, W9)
#   MECLAW_GATE_MODE             precheck, testing_bans, slow_tests
#   MECLAW_REQUIRE_BROWSERS      the browser drivers: `1` turns a SKIP (no
#                                engine) into a red test; measured without it
#                                on build03, a driver SKIP graded GREEN
#   MECLAW_TESTING_RULES_RULING  testing_bans (the owner ruling's register id)
# The ONE list of the kit: `lane_env` here (`strand.sh test`, `pre`'s check,
# the gate's lane environment through `lane.env_for`), and on the Python side
# `pipeline/lane.py verdict_env()` reads it from this line (flake_triage's
# Python class, `measure`) and the coverage run. Keep it on one line.
VERDICT_ENV=(MECLAW_ALLOW_BUMP MECLAW_GATE_MODE MECLAW_REQUIRE_BROWSERS MECLAW_TESTING_RULES_RULING)

# The words the transport writes on a lane's command line beside VERDICT_ENV
# and GOLDEN_SWITCHES (GH #1209, re-check tv-bn M7c M3): `lane_env` (the
# lane's name, width, incremental, sccache, linker) and `cmd_test` (target,
# tier lane, the browser brake). ssh carries no environment, so a name
# reaches the host only as one of these words -- a kit test's fake ssh that
# runs the command here would hand it on from the caller instead, and its
# transport pin would be green without a transport. The ONE list:
# `lane_env` and `cmd_test` refuse a word it lacks (`lane_words_known`), and
# the fakes strip what it names (`scripts/tests/fake_ssh.py`, through
# `pipeline/lane.py lane_words()`). Keep it on one line.
LANE_WORDS=(MECLAW_LANE_NAME CARGO_INCREMENTAL CARGO_BUILD_JOBS NEXTEST_TEST_THREADS RUSTC_WRAPPER SCCACHE_DIR SCCACHE_CACHE_SIZE RUSTFLAGS CARGO_TARGET_DIR MECLAW_TIER_LANE MECLAW_BROWSER_CPU_THROTTLE MECLAW_TEST_TIME_FACTOR MECLAW_TEST_CPU_QUOTA)

# `lane_words_known <words>`: dies unless every name <words> sets is named
# by LANE_WORDS, VERDICT_ENV or GOLDEN_SWITCHES -- the words of `lane_env` and
# `cmd_test`, the caller's side (HOST_OWN is never a caller's). Same scan as
# `remote_words_known` (`set_names_known`), fail-closed: a value that looks
# like a further word is refused as well.
lane_words_known() {
    local rest="$1" name n
    # the base's rule, kept (re-check tv-bn M7h A-M3): every `NAME=` at a word
    # start, ANY case -- lower-case names are a host script's own only on the
    # remote side (OR-M7e-2), never a word of the caller
    while [[ $rest =~ (^|[[:space:]])([A-Za-z_][A-Za-z0-9_]*)= ]]; do
        name=${BASH_REMATCH[2]}
        rest=${rest#*"${BASH_REMATCH[0]}"}
        for n in "${LANE_WORDS[@]}" "${VERDICT_ENV[@]}" "${GOLDEN_SWITCHES[@]}" ''; do
            [ "$n" = "$name" ] && break
        done
        [ -n "$n" ] || die "the lane word $name is not in LANE_WORDS (scripts/lane_sync.sh) -- name it there, the kit tests' fake-ssh strips what the list names"
    done
    set_names_known env "$1" "the kit tests' fake-ssh strips what the list names"
}

# `env_word <var> <NAME> <value>`: appends `NAME=<value> ` to the variable
# <var>, the value quoted with `q`; dies (in THIS shell, never a subshell
# that would only drop the word) unless NAME is one of LANE_WORDS,
# VERDICT_ENV or GOLDEN_SWITCHES (orchestrator M7g: the environment of a lane
# command is built from data at one place and checked BEFORE it is written).
env_word() {   # <var> <NAME> <value>
    local n
    for n in "${LANE_WORDS[@]}" "${VERDICT_ENV[@]}" "${GOLDEN_SWITCHES[@]}" ''; do
        [ "$n" = "$2" ] && break
    done
    [ -n "$n" ] || die "the lane word $2 is not in LANE_WORDS, VERDICT_ENV or GOLDEN_SWITCHES (scripts/lane_sync.sh) -- name it there (command not sent)"
    printf -v "$1" '%s%s=%s ' "${!1}" "$2" "$(q "$3")"
}

# `NAME=value ` (quoted) of the VERDICT_ENV names set and non-empty here.
verdict_words() {
    local name w=""
    for name in "${VERDICT_ENV[@]}"; do
        [ -z "${!name:-}" ] || env_word w "$name" "${!name}"
    done
    printf '%s' "$w"
}

# A TEST run outside the gate (`test`, `coverage`) requires the browsers by
# default (GH #1209, re-check tv-bn M7 M4): nobody set the switch there, so a
# browser driver on a lane without its engine SKIPped green -- in `strand.sh
# test`, a TV-F7 run of flake_triage (through it) and the coverage run alike,
# a proof without a run. Unset is `1`; any other value is the caller's opt-out,
# kept and named once as a NOTE. The gate decides per station itself (executor
# `_env`: a booking station `1`, any other without the switch); `lane.py
# test_run_env` is the same rule on the Python side.
require_browsers_default() {
    if [ -z "${MECLAW_REQUIRE_BROWSERS+x}" ]; then
        MECLAW_REQUIRE_BROWSERS=1
    elif [ "$MECLAW_REQUIRE_BROWSERS" != 1 ]; then
        echo "NOTE: MECLAW_REQUIRE_BROWSERS='$MECLAW_REQUIRE_BROWSERS' (opt-out) -- a browser driver without an engine SKIPs green, no browser proof" >&2
    fi
}

# shellcheck disable=SC2016
lane_env() {   # <mode>
    local mode="$1" w="" j t
    [ -z "${MECLAW_LANE_NAME:-}" ] || env_word w MECLAW_LANE_NAME "$MECLAW_LANE_NAME"
    w+=$(verdict_words)
    case "$mode" in
        strand|test) w+="CARGO_INCREMENTAL=1 " ;;
        main-pass|m0) w+="CARGO_INCREMENTAL=0 " ;;
    esac
    if [ "$(lane_class)" = egon ]; then
        # class egon is always THIS machine (`local`): its cores count here --
        # half of them, never over 4 (the executor's EGON_MAX_THREADS, which
        # admission books with, `schedule._runner_cpu`): the lane runs exactly
        # the threads it books. Half the cores alone gave nproc 12 a width and
        # a booking of 6 while every station ran 4, and nproc 6 a booking of 3
        # beside 4 test threads.
        t=$(( $(nproc) / 2 )); [ "$t" -ge 1 ] || t=1; [ "$t" -le 4 ] || t=4
        j=$t
        if posint "${MECLAW_LANE_JOBS:-}" && [ "$MECLAW_LANE_JOBS" -lt "$j" ]; then j=$MECLAW_LANE_JOBS; fi
        w+="CARGO_BUILD_JOBS=$j NEXTEST_TEST_THREADS=$t "
    else
        w+=$(lane_width)
        # A build host without `threads=` runs its tests at full width -- the
        # host expands `$(nproc)`; without the word test-tier.sh's egon floor
        # of 4 threads ran on every lane (DIAG dt-zdiag, x0.29 on tests).
        case "$w" in *NEXTEST_TEST_THREADS=*) ;; *) w+='NEXTEST_TEST_THREADS=$(nproc) ' ;; esac
    fi
    if [ "${MECLAW_LANE_SCCACHE:-}" = 1 ]; then
        env_word w RUSTC_WRAPPER sccache; env_word w SCCACHE_DIR "$ROOT/sccache"; env_word w SCCACHE_CACHE_SIZE 30G
    fi
    case "${MECLAW_LANE_LINKER:-}" in
        mold) env_word w RUSTFLAGS '--cfg tokio_unstable -C link-arg=-fuse-ld=mold' ;;
        wild) env_word w RUSTFLAGS '--cfg tokio_unstable -C linker=clang -C link-arg=--ld-path=wild' ;;
    esac
    lane_words_known "$w"
    printf '%s' "$w"
}

# The free G of the lane root, or nothing when df cannot tell.
lane_free_g() {   # <target>
    remote "$1" "df -BG --output=avail $(q "$ROOT") 2>/dev/null | tail -1" 2>/dev/null | tr -dc 0-9
}

# The floor (W1 until W2 clears generations): below `MECLAW_LANE_FLOOR_G`
# (60) G free the run is refused, exit 2 -- never build into ENOSPC. The
# runner's wipe of the whole target (`lane_wipe_target` in gate.sh) stays
# reachable only on purpose: `MECLAW_LANE_WIPE_OK=<reason>` -- without it
# the pipeline hands the runner a floor of 0 (`lane.env_for`), so no df
# error, low floor or race reaches the wipe. A floor of 0 is off (the
# self-tests' fake lane lies in a small temp directory). When df cannot
# tell, a NOTE line says so -- never a silent pass.
lane_floor() {   # <target>
    local floor="${MECLAW_LANE_FLOOR_G:-60}" free
    [ "$floor" = 0 ] && return 0
    posint "$floor" || floor=60
    [ -z "${MECLAW_LANE_WIPE_OK:-}" ] || return 0
    free=$(lane_free_g "$1")
    if [ -z "$free" ]; then
        echo "lane_sync: NOTE free space on $ROOT unknown (df gave nothing) -- floor not checked" >&2
        return 0
    fi
    if [ "$free" -lt "$floor" ]; then
        echo "lane_sync: only ${free}G free on $ROOT -- floor ${floor}G: clear old generations (W2)" >&2
        exit 2
    fi
}

# A `local` lane is the owner's machine, beside the live colony: it runs only
# as class `egon` (half the cores, 4 test threads, nice, the watchdog --
# BUILD-PREAMBLE 8). Without it a test ran unguarded at full width; refused,
# exit 5 (review I2). The other direction (egon => local) is checked in
# `cmd_test`.
local_needs_egon() {   # <target>
    if [ "$1" = local ] && [ "$(lane_class)" != egon ]; then
        echo "lane_sync: refused -- a local lane is class egon (class=egon watch=<unit> in the host file)" >&2
        exit 5
    fi
}

# Names a command for the lane may set beside LANE_WORDS, VERDICT_ENV and
# GOLDEN_SWITCHES (re-check tv-bn M7d B-H1): the host's own -- `PATH` extends
# the host's own $PATH (`export PATH="$HOME/.cargo/bin:...:$PATH"`, expanded
# there), `IFS` is a `read`'s field separator in the kit's host scripts,
# `TMPDIR` the kit's constant tmpfs of a measured run (`pipeline/measure.py
# timed_script`, OR-TV.tv-bn.M7e-1), `LC_ALL` the byte order of the host's
# `sort` in GOLDEN_HASH (M7e2: without it the narrow point refused every
# blessing run of `cmd_test`, exit 2). No value of the caller rides on them,
# so a fake ssh has nothing of them to strip. Keep it on one line.
HOST_OWN=(PATH IFS TMPDIR LC_ALL)

# The host's own names a command for a lane keeps (orchestrator OR-T10-2,
# re-check tv-bn M7h A-M2): `remote` and `remote_long` run the command under
# `env -i` with these names -- expanded THERE, on the host -- and nothing
# else; every other name the command needs it carries as a word (`env_word`,
# LANE_WORDS, VERDICT_ENV, GOLDEN_SWITCHES). So it does not matter how a name
# was set on this side: a fake ssh that runs the command here hands on
# nothing of the caller either. The list is the environment of a lane's
# non-interactive ssh shell (measured on build01 10/12: these plus the
# shell's own PWD/SHLVL/_, SSH_CLIENT/SSH_CONNECTION and XDG_SESSION_*);
# `systemd-run --user` needs XDG_RUNTIME_DIR and the bus. `pipeline/lane.py
# remote_command` reads it from this line. Keep it on one line.
REMOTE_BASE=(HOME USER LOGNAME SHELL PATH LANG XDG_RUNTIME_DIR DBUS_SESSION_BUS_ADDRESS)

# `remote_env_i <command>`: the command line the host runs for <command> --
# `exec env -i <REMOTE_BASE, each if set there> bash -c '<command>'`, the
# command in single quotes (each `'` as `'"'"'`), the form of
# `pipeline/lane.py remote_command` (shlex.quote).
# shellcheck disable=SC2016
remote_env_i() {
    local sq="'\"'\"'"
    printf "exec %sbash -c '%s'" "$(remote_env_keep)" "${1//\'/$sq}"
}

# `env -i <REMOTE_BASE, each if set there> `: the front of every program a
# host runs for a lane -- the bash of `remote_env_i`, and the rsync
# (`--rsync-path`) and git-receive-pack (`--receive-pack`) the transport
# starts there (re-check tv-bn M7j A-M4, orchestrator OR-T10-11). The rule of
# `pipeline/lane.py remote_env_i`.
remote_env_keep() {
    local n keep=""
    for n in "${REMOTE_BASE[@]}"; do keep+="\${$n+\"$n=\$$n\"} "; done
    printf 'env -i %s' "$keep"
}

# `remote_words_known <command>` (re-check tv-bn M7d B-H1, M7e B-M2): the
# check of the words at the ONE narrow point every command for a lane passes
# (`remote`, `remote_long`, `run_guarded`). M7e looked for a name only behind
# a blank, `;&|(` or a literal `\n`; eight forms passed (`'FOO=`, `"FOO=`,
# `FOO+=`, `$'FOO=..\n..'`, a backtick, `--setenv=FOO=`, `q`/`shlex.quote` of
# a word or a script) -- the forms the kit's own quoting writes. Now the scan
# has no left context (orchestrator M7g, fail-closed instead of a form list):
# EVERY upper-case name directly before `=`, `+=`, `:=` or an index, and every
# upper-case name a setting builtin takes (export declare typeset readonly
# local read readarray mapfile for, `printf -v`), is one of the three lists or
# HOST_OWN, or the command never leaves: exit 2. `pipeline/lane.py
# set_names` is the same rule; `test_lane_engstelle` pins both on one corpus.
# Lower-case names are a host script's own variables (OR-M7e-2).
remote_words_known() {
    set_names_known remote "$1" "the kit tests' fake-ssh strips what the list names (command not sent)"
}

# `set_names_known env|remote <text> <why>`: dies unless every name <text>
# sets (the scan above) is in the lists of its side -- `env`: LANE_WORDS,
# VERDICT_ENV, GOLDEN_SWITCHES; `remote`: those and HOST_OWN.
set_names_known() {
    local side="$1" rest="$2" why="$3" name n seg names=()
    local re_a='([A-Z_][A-Z0-9_]*)(\[[^]]*\])?[+:]?='
    local re_k='(^|[^A-Za-z0-9_-])(export|declare|typeset|readonly|local|read|readarray|mapfile|for|printf[[:space:]]+-v)[[:space:]]([^;&|'$'\n'']*)'
    # `{` before `$` in the bracket: the same set, and no `${` that reads as an
    # expansion in single quotes (SC2016)
    local re_t='(^|[^A-Za-z0-9_{$/.:-])([A-Z_][A-Z0-9_]*)([^A-Za-z0-9_]|$)'
    # the value of a NAME=value word is no name (lane.py _SET_VALUE, M7h)
    local re_v="=('[^']*'|\"[^\"]*\"|[^[:space:]'\";&|])+"
    local re_n='(^|[[:space:]])-[A-Za-z]*n'
    while [[ $rest =~ $re_a ]]; do
        names+=("${BASH_REMATCH[1]}")
        rest=${rest#*"${BASH_REMATCH[0]}"}
    done
    rest="$2"
    while [[ $rest =~ $re_k ]]; do
        seg=${BASH_REMATCH[3]}
        rest=${rest#*"${BASH_REMATCH[0]}"}
        # ... except behind a nameref option: its value is a name (M7i)
        [[ $seg =~ $re_n ]] || while [[ $seg =~ $re_v ]]; do
            seg=${seg/"${BASH_REMATCH[0]}"/=}
        done
        while [[ $seg =~ $re_t ]]; do
            names+=("${BASH_REMATCH[2]}")
            seg=${seg#*"${BASH_REMATCH[1]}${BASH_REMATCH[2]}"}
        done
    done
    for name in ${names[@]+"${names[@]}"}; do
        for n in "${LANE_WORDS[@]}" "${VERDICT_ENV[@]}" "${GOLDEN_SWITCHES[@]}" ''; do
            [ "$n" = "$name" ] && break
        done
        if [ -z "$n" ] && [ "$side" = remote ]; then
            for n in "${HOST_OWN[@]}" ''; do [ "$n" = "$name" ] && break; done
        fi
        [ -n "$n" ] || die "the lane word $name is not in LANE_WORDS (scripts/lane_sync.sh) -- name it there, $why"
    done
}

# The ssh program of every way to a lane (re-check tv-bn M7h A-M1,
# orchestrator OR-T10-2; `pipeline/lane.py ssh_program` is the same rule,
# `test_lane_engstelle` pins both on one set of probes). The check runs
# ALWAYS, and what it checked is what runs: `ssh_bin` leaves the resolved
# ABSOLUTE path in SSH_CHECKED, the program word of every way (re-check tv-bn
# M7j A-M1: the name `ssh` ran whatever the exec found -- an exported
# function `ssh`, a hand fake under another PATH). The `ssh` of the first
# absolute entry of an EXPORTED PATH (none, or no ssh: exit 2) is admitted
# only as (a) the system's own -- the realpath of `ssh` in one of
# SYSTEM_SSH_DIRS, that file and every directory above it up to `/` root's
# and writable by neither group nor others (M7j A-M2: momentary writability
# admitted the user's own 0555 file) -- or (b) a fake of
# `scripts/tests/fake_ssh.py` whose path AND body hash (64 hex digits) stand
# in MECLAW_FAKE_SSH_REGISTRY (`:`-separated `<realpath>@<sha256>`). No
# registry admits no fake (fail-closed). The check takes its tools by
# absolute path (bash never imports a function whose name holds a `/`) and
# uses keywords and expansions only, no `command`/`[`/`read`/`echo` a
# caller's PATH or exported function could replace (M7j A-M3, measured: a
# `realpath` on PATH or `BASH_FUNC_realpath%%` answered for the hand fake,
# and a `<path>@x` entry matched an unreadable body).
#
# The same check admits the local `rsync` and `git` of a remote way
# (`tool_bin`, re-check tv-bn M7l A-H5, orchestrator OR-T10-25): their
# `--rsync-path`/`--receive-pack` carry the host side's `env -i`, which a
# PATH shadow skipped. A candidate is a regular executable file in both
# rules (M7l A-M2: `-f` skipped a FIFO named `ssh` that Python took), the
# modes are tested arithmetically. The shebang `#!/bin/bash -p` keeps the
# caller's code out of this shell (M7l A-M1, measured: `BASH_FUNC_exit%%`
# made `die` return, BASH_ENV defined `exit()` or set SSH_STAT and
# SYSTEM_SSH_DIRS readonly beforehand, and `#!/usr/bin/env bash` took the
# interpreter from PATH): privileged mode imports no function and reads
# neither BASH_ENV/ENV nor SHELLOPTS/BASHOPTS.
SYSTEM_SSH_DIRS=(/usr/bin /bin /usr/sbin /sbin /usr/local/bin)
SSH_REALPATH=/usr/bin/realpath
SSH_SHA256=/usr/bin/sha256sum
SSH_STAT=/usr/bin/stat
SSH_CHECKED=""
TOOL_CHECKED=""
# The checked absolute path of the `name` PATH names in TOOL_CHECKED, or exit 2.
tool_bin() {   # name
    local name=$1 rest d found="" real="" chain=() x modes m ok="" sha="" e p
    TOOL_CHECKED=""
    [[ -n ${PATH:-} && ${PATH@a} == *x* ]] \
        || die "no PATH to find $name on -- the narrow point runs only the program it checked (command not sent)"
    rest=$PATH:
    while [[ -z $found && -n $rest ]]; do
        d=${rest%%:*}
        rest=${rest#*:}
        [[ $d == /* && -f $d/$name && -x $d/$name ]] && found=$d/$name
    done
    [[ -n $found ]] || die "no $name on PATH ($PATH) -- the narrow point runs only the program it checked (command not sent)"
    real=$("$SSH_REALPATH" -e -- "$found" 2>/dev/null)
    [[ $real == /* ]] || die "unregistered $name on PATH: $found -- its path does not resolve (command not sent)"
    for d in "${SYSTEM_SSH_DIRS[@]}"; do   # rule a: the system's own, root's, written by nobody else
        if [[ -z $ok && -f $d/$name && -x $d/$name && $("$SSH_REALPATH" -eq -- "$d/$name") == "$real" ]]; then
            x=$real
            chain=("$x")
            while [[ $x != / ]]; do
                x=${x%/*}
                [[ -n $x ]] || x=/
                chain+=("$x")
            done
            modes=$("$SSH_STAT" -c '%u:%a' -- "${chain[@]}" 2>/dev/null) || modes=""
            # one `0:<octal>` per link of the chain, no group/other write bit
            ok=1
            rest=$modes$'\n'
            e=0
            while [[ -n $rest ]]; do
                m=${rest%%$'\n'*}
                rest=${rest#*$'\n'}
                e=$((e + 1))
                [[ $m =~ ^0:[0-7]+$ ]] && (( (8#${m#0:} & 8#22) == 0 )) || ok=""
            done
            [[ -n $modes && $e -eq ${#chain[@]} ]] || ok=""
        fi
    done
    rest=${MECLAW_FAKE_SSH_REGISTRY:-}:
    while [[ -z $ok && -n $rest ]]; do   # rule b: a registered fake, its body unchanged
        e=${rest%%:*}
        rest=${rest#*:}
        p=${e%@*}
        if [[ -f $real && $p == /* && $p != "$e" && ${e##*@} =~ ^[0-9a-f]{64}$
              && $("$SSH_REALPATH" -e -- "$p" 2>/dev/null) == "$real" ]]; then
            [[ -n $sha ]] || sha=$("$SSH_SHA256" -- "$real" 2>/dev/null) || sha=""
            sha=${sha%% *}
            [[ $sha =~ ^[0-9a-f]{64}$ && $sha == "${e##*@}" ]] && ok=1
        fi
    done
    [[ -n $ok ]] || die "unregistered $name on PATH: $found -- neither the system's nor a fake with its hash in MECLAW_FAKE_SSH_REGISTRY (scripts/tests/fake_ssh.py registers a test's fake; command not sent)"
    TOOL_CHECKED=$real
}
# shellcheck disable=SC2120
ssh_bin() {
    SSH_CHECKED=""
    tool_bin ssh
    SSH_CHECKED=$TOOL_CHECKED
}

remote() {   # target, command string
    local target="$1"; shift
    remote_words_known "$*"
    if [ "$target" = local ]; then
        bash -c "$*"
        return
    fi
    # SC2029: the command string is built HERE on purpose, every value in it
    # quoted with `q` -- the host runs exactly what this side composed.
    ssh_bin
    # shellcheck disable=SC2029
    "$SSH_CHECKED" "${SSH_OPTS[@]}" "$target" "$(remote_env_i "$*")"
}

# git push to a lane's bare repository: over ssh with the checked program,
# git-receive-pack on the host under `env -i` like every remote command (re-check
# tv-bn M7j A-M4, orchestrator OR-T10-11); a `local` lane pushes to the path.
lane_git_push() {   # target, bare repository path, refspecs
    local target="$1" bare="$2"; shift 2
    if [ "$target" = local ]; then
        git push -q --force "$bare" "$@"
        return
    fi
    ssh_bin
    tool_bin git
    GIT_SSH_COMMAND="$(q "$SSH_CHECKED") ${SSH_OPTS[*]}" \
        "$TOOL_CHECKED" push -q --force --receive-pack="$(remote_env_keep)git-receive-pack" "$target:$bare" "$@"
}

# rsync with the lane's transport: ssh for a host, nothing for `local`. A
# path on the lane is `<target>:<path>`, or the plain path for `local`.
lane_rsync() {   # target, rsync args (with %LANE% before a lane path)
    local target="$1" a rargs=(); shift
    for a in "$@"; do
        case "$a" in
            %LANE%*) if [ "$target" = local ]; then rargs+=("${a#%LANE%}"); else rargs+=("$target:${a#%LANE%}"); fi ;;
            *) rargs+=("$a") ;;
        esac
    done
    if [ "$target" = local ]; then
        rsync "${rargs[@]}"
    else
        ssh_bin
        tool_bin rsync
        "$TOOL_CHECKED" -e "$(q "$SSH_CHECKED") ${SSH_OPTS[*]}" --rsync-path="$(remote_env_keep)rsync" "${rargs[@]}"
    fi
}

# A long run on the lane: ssh with keep-alive, or a local shell.
remote_long() {   # target, command string
    local target="$1"; shift
    remote_words_known "$*"
    if [ "$target" = local ]; then
        bash -c "$*"
        return
    fi
    ssh_bin
    # shellcheck disable=SC2029
    "$SSH_CHECKED" "${SSH_OPTS[@]}" -o ServerAliveInterval=60 "$target" "$(remote_env_i "$*")"
}

# A hold on the lane (Z5): the lock's connection idles on stdin for a whole
# gate run, which is exactly what a middlebox drops -- and with the
# connection the far side's `cat` ends and the build path is unlocked under
# a running gate. The three holds (`hold`, the push lock, the coverage tree
# lock) went over `remote()` without a keep-alive; they take remote_long's.
remote_hold() {   # target, command string
    remote_long "$@"
}

# The build path's lock (W1 hand-over, Delta C I-neu-1): `<build path>.lock`,
# the one a gate holds from its push to its fetch (`lane.hold`). Whoever
# rewrites the build path takes it with `flock -n` -- busy is exit 5, and no
# token skip reaches it. The holder leaves a nonce in `<lock>.who`; a caller
# that holds the lock already (`hold` below, `strand.sh pre`) names it in
# MECLAW_LANE_HOLD, and the push checks that the lock is held under that name.
# shellcheck disable=SC2016
HOLD_SH='mkdir -p "$(dirname "$1")" && exec 9>>"$1" && flock -n 9 || { echo busy; exit 0; }
printf %s "$2" >"$1.who"; echo "held $2"; exec cat >/dev/null'

hold_nonce() { printf '%s-%s-%s' "$$" "$(date +%s%N)" "$RANDOM$RANDOM"; }

cmd_hold() {   # <ssh-target> <strand>: prints `held <nonce>`, holds until stdin closes
    local target="$1" wt
    wt=$(build_path "$2") || exit 5
    remote_hold "$target" "bash -c $(q "$HOLD_SH") hold $(q "$wt.lock") $(q "$(hold_nonce)")"
}

# Take (or check) the lock of build path $2 on host $1 for this push.
push_lock() {
    local target="$1" lock="$2.lock" word
    if [ -n "${MECLAW_LANE_HOLD:-}" ]; then
        remote "$target" "! flock -n $(q "$lock") true && [ \"\$(cat $(q "$lock.who") 2>/dev/null)\" = $(q "$MECLAW_LANE_HOLD") ]" \
            && return 0
        echo "lane_sync: refused -- MECLAW_LANE_HOLD does not hold $lock on $target" >&2
        exit 5
    fi
    coproc PUSH_HOLD { remote_hold "$target" "bash -c $(q "$HOLD_SH") hold $(q "$lock") $(q "$(hold_nonce)")"; }
    read -r word _ <&"${PUSH_HOLD[0]}" || word=""
    [ "$word" = held ] && return 0
    [ "$word" = busy ] || die "the host $target did not answer"
    echo "lane_sync: refused -- the build path $2 on $target is in use (another push+run holds $lock)" >&2
    exit 5
}

# The end of this push's own lock (an exit lets go of it as well).
push_unlock() {
    [ -n "${PUSH_HOLD_PID:-}" ] || return 0
    local pid="$PUSH_HOLD_PID"
    eval "exec ${PUSH_HOLD[1]}>&-"
    wait "$pid" 2>/dev/null || true
}

cmd_push() {
    local target="$1" strand="$2" sha wt top
    top=$(git rev-parse --show-toplevel 2>/dev/null) || die "not a git repository"
    cd "$top" || die "cannot enter $top"
    sha=$(git rev-parse HEAD) || die "no HEAD to push"
    wt=$(build_path "$strand") || exit 5
    push_lock "$target" "$wt"

    # Before anything reaches the host: no tracked secret (exit 5, with the
    # paths). Fail closed -- never send it, never gate without it.
    local hits
    hits=$(collisions)
    if [ -n "$hits" ]; then
        echo "lane_sync: refused -- tracked secret file(s), they would leave this machine:" >&2
        printf '  %s\n' "$hits" >&2
        exit 5
    fi

    # The bare repository; a fresh host has none. A local lane borrows the
    # objects of this clone instead of copying the history.
    remote "$target" "mkdir -p $(q "$ROOT/wt") && { test -d $(q "$ROOT/meclaw.git") || git init -q --bare $(q "$ROOT/meclaw.git"); }" \
        || die "the host $target did not answer"
    if [ "$target" = local ]; then
        local objects
        objects=$(git rev-parse --path-format=absolute --git-common-dir)/objects
        [ -f "$ROOT/meclaw.git/objects/info/alternates" ] \
            || printf '%s\n' "$objects" >"$ROOT/meclaw.git/objects/info/alternates"
    fi
    # The commit, and the refs the export audit reads (GH #942).
    local refs=("$sha:refs/heads/lane/$strand") ref url
    for ref in github-main master; do
        git rev-parse --verify --quiet "refs/heads/$ref" >/dev/null \
            && refs+=("refs/heads/$ref:refs/heads/$ref")
    done
    lane_git_push "$target" "$ROOT/meclaw.git" "${refs[@]}" || die "git push to $target failed"
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
            # The last run touched files after its checkout (tree-sync, the
            # overlay stamp): their stat in the index is stale, and a forced
            # checkout rewrote each one -- new mtimes, every workspace member
            # rebuilt. The refresh compares content and keeps what is equal.
            git -C $(q "$wt") update-index -q --refresh >/dev/null || true
            git -C $(q "$wt") checkout -q --force --detach $sha
            git -C $(q "$wt") clean -fdq
        else
            rm -rf $(q "$wt")
            mkdir -p $(q "${wt%/*}")
            git --git-dir=$(q "$ROOT/meclaw.git") worktree prune
            git --git-dir=$(q "$ROOT/meclaw.git") worktree add -q --force --detach $(q "$wt") $sha
        fi" >/dev/null || die "the checkout on $target failed"

    # The overlay: what is not committed yet.
    if [ -s "$list" ]; then
        # `--checksum`: rsync's quick check (size + mtime) skipped a change
        # of equal size when the host's checkout fell into the same second
        # as the local write -- measured on the first build host, the
        # overlay hash below refused it. The overlay is small; compare content.
        lane_rsync "$target" -a --checksum --from0 --files-from="$list" ./ "%LANE%$wt/" \
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
    remote "$target" "cd $(q "$wt") && $(kit_py_q) scripts/gate_plan.py --print lane-env >.env" \
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
    push_unlock
    printf '%s %s %s\n' "$sha" "$here" "${browser:-none}"
}

# The guards of a gate run, before the runner starts: a local lane without
# class egon (exit 5), a class egon lane (exit 2), the lane hygiene (W2,
# exit 6 when the host has no room even after collecting), the floor (exit 2).
# `run` and `guard` (the pipeline's lane path, W3) share them.
run_guards() {   # <target> <build path> <target dir>
    local_needs_egon "$1"
    # Until the gate on the owner's machine is proven (M0-A13, accepted in
    # Z), a class `egon` lane runs single tests only.
    if [ "$(lane_class)" = egon ] && [ -z "${MECLAW_LANE_EGON_GATE_OK:-}" ]; then
        echo "lane_sync: a class egon lane runs 'test' only -- no gate there" \
             "(MECLAW_LANE_EGON_GATE_OK=<reason> overrides)" >&2
        exit 2
    fi
    # The lane hygiene first (W2): it collects and evicts before the floor
    # below judges the free space, so the floor only refuses what the
    # hygiene could not clear.
    hygiene_before "$1" "$2" "$3" || return $?
    lane_floor "$1"
}

# THE WATCHDOG OF A CLASS `egon` RUN: the journal of the watched unit
# (`MECLAW_LANE_WATCH`, `watch=<unit>` in the host file) is FOLLOWED -- a
# stream, not a poll -- for the run's lifetime; the first trip line -- the
# colony's own `meclaw: watchdog trip` or systemd's `Watchdog timeout` -- kills the run's process group and leaves a `watchdog_fired` line
# (V0 section 3: `run` from `MECLAW_LANE_RUN`, the kit's run directory, and
# `at` in UTC; `reason` egon is additive). `MECLAW_LANE_WATCH_CMD` replaces
# the journal command (the tests feed it a line).
# shellcheck disable=SC2016
watch_cmd() {
    if [ -n "${MECLAW_LANE_WATCH_CMD:-}" ]; then
        printf '%s' "$MECLAW_LANE_WATCH_CMD"
    else
        printf 'exec journalctl --user -u %s -f -n0 -o cat' "$(q "$MECLAW_LANE_WATCH")"
    fi
}

# The stop of the watcher waits for its handshake (#1185): the watcher
# writes one line into a FIFO once it runs in its own session. A tier that
# ended before the watcher's setsid() used to send the group's TERM into
# nothing (ESRCH, swallowed), and `wait` then hung on the journal follower
# for good -- 5/240 runs of the guard's pin under load, a gate shard stalled
# up to the station timeout, an orphan left behind. Without the handshake
# within MECLAW_LANE_WATCH_HANDSHAKE_S (default 10 s) the watcher has no
# group to stop: the TERM goes to its PID, and to the group in case it was
# formed meanwhile; either way it reaches the process `wait` waits for.
# With the handshake the group's TERM is the whole stop: a group that is
# gone means a watcher that ended and may be reaped, and a TERM to its PID
# would hit a free PID or whatever process got it since (re-check 7d).
stop_watcher() {   # <pid> <handshake fd> <seconds to wait for the handshake>
    if read -r -t "$3" -u "$2" _; then
        kill -TERM -- "-$1" 2>/dev/null
    else
        kill -TERM "$1" 2>/dev/null; kill -TERM -- "-$1" 2>/dev/null
    fi
    # Only once the TERM is out: an abort during the handshake wait still stops.
    watcher_stopped=1
}

# An abort of lane_sync.sh during run_guarded (TERM, INT, HUP) stops the
# tier's group and the watcher and removes the FIFO; without a trap all
# three stayed behind (re-check 7d). Called from the trap while run_guarded
# runs, so its locals are in scope. A watcher already stopped is not stopped
# again: a TERM during `wait "$wp"` found its handshake read and sent the
# TERM to the bare PID of a watcher that may be reaped (tb-s7 Restfenster i).
# SC2317: only the trap of run_guarded calls it, shellcheck sees no caller in the group (GH #1223)
# shellcheck disable=SC2154,SC2317
guard_abort() {   # <exit code>
    [ -n "$tp" ] && kill -TERM -- "-$tp" 2>/dev/null
    [ -n "$wp" ] && [ -z "$watcher_stopped" ] && stop_watcher "$wp" "$hfd" 0
    rm -f "$fired.line" "$hs"
    exit "$1"
}

run_guarded() {   # <command string>; class egon only, always local
    local cmd="$1" fired hs hfd tp="" wp="" rc watcher_stopped=""
    # The narrow point's check (re-check tv-bn M7e B-M1): class egon runs its
    # command here, outside `remote`, and only the early `lane_words_known`
    # of `cmd_test` saw its words.
    remote_words_known "$cmd"
    local hs_s="${MECLAW_LANE_WATCH_HANDSHAKE_S:-10}"
    fired=$(mktemp) || die "no temp file"
    rm -f "$fired"
    hs="$fired.hs"
    mkfifo -m 600 "$hs" || die "no handshake FIFO"
    # Read-write: the open never blocks and the watcher's write never does.
    exec {hfd}<>"$hs"
    trap 'guard_abort 143' TERM
    trap 'guard_abort 130' INT
    trap 'guard_abort 129' HUP
    setsid bash -c "$cmd" {hfd}>&- &
    tp=$!
    # The handshake comes after the follower and grep exist: bash blocks
    # TERM while it forks, so a group TERM landing in that window spared the
    # child born from it -- a follower left in the stopped group, 6/90 runs
    # of the guard's pins at load 12 (re-check 7d series).
    # shellcheck disable=SC2016
    setsid bash -c 'bash -c "$1" | grep -m1 -E "Watchdog timeout|watchdog trip" >"$2.line" &
        echo ready >"$4"
        wait "$!" || exit 0
        mv "$2.line" "$2"; kill -TERM -- "-$3" 2>/dev/null || kill -TERM "$3" 2>/dev/null' \
        _ "$(watch_cmd)" "$fired" "$tp" "$hs" {hfd}>&- &
    wp=$!
    wait "$tp"; rc=$?
    stop_watcher "$wp" "$hfd" "$hs_s"
    wait "$wp" 2>/dev/null
    trap - TERM INT HUP
    exec {hfd}<&-
    rm -f "$fired.line" "$hs"
    if [ -f "$fired" ]; then
        printf 'lane_sync: watchdog_fired {"run": "%s", "at": "%s", "runner": "%s", "kind": "process", "reason": "egon", "value": %s, "threshold": 1, "action": "killed"}\n' \
            "${MECLAW_LANE_RUN:-?}" "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "${MECLAW_LANE_NAME:-local}" \
            "$("${KIT_PY[@]}" -c 'import json,sys; print(json.dumps(sys.stdin.read().strip()))' <"$fired")" >&2
        rm -f "$fired"
        return 2
    fi
    return "$rc"
}

# `hygiene [--kit] <ssh-target> <command> [args]`: `scripts/pipeline/hygiene.py`
# of THIS tree on the host, the script on ssh's stdin -- no copy, no version
# skew between the tree under test and the hygiene that judges its host.
# `--root` of the lane goes along. `--kit` is for a command that leaves a
# timer behind on the host (`bench new`): a timer needs a script PATH to call
# when it fires, so the script is first written to `kit/<sha8>/hygiene.py`
# under the lane root (named by its content, so two trees never overwrite
# each other's copy with a different one) and run from there.
cmd_hygiene() {
    local kit=0 target args="" a sha dir
    if [ "${1:-}" = --kit ]; then kit=1; shift; fi
    [ $# -ge 2 ] || die "hygiene [--kit] <ssh-target> <command> [args]"
    target="$1"; shift
    if [ ! -f "$HYGIENE" ]; then
        echo "lane_sync: no $HYGIENE in this tree -- the lane hygiene is skipped" >&2
        return 3
    fi
    for a in "$@" --root "$ROOT"; do args+=" $(q "$a")"; done
    if [ "$kit" = 0 ]; then
        remote "$target" "$(kit_py_q) -$args" <"$HYGIENE"
        return $?
    fi
    sha=$(sha256sum <"$HYGIENE" | cut -c1-8)
    dir="$ROOT/kit/$sha"
    remote "$target" \
        "mkdir -p $(q "$dir") && cat >$(q "$dir/hygiene.py").part.\$\$ && mv -f $(q "$dir/hygiene.py").part.\$\$ $(q "$dir/hygiene.py") && $(kit_py_q) $(q "$dir/hygiene.py")$args" \
        <"$HYGIENE"
}

# The check before a gate (THE LANE HYGIENE). Returns 0 to go on, 6 when the
# host has no room, 255 when ssh lost it. The run's target (`$3`, the lane's
# `target/` or `slot/<n>/target` beside the build path `tree/`) is handed
# over as own: the check never evicts it. Wiping it stays the runner's job
# and is reachable only with `MECLAW_LANE_WIPE_OK` -- then it is not
# protected here either (W2 review C1).
#
# ONE floor (review M1): the reserve is `lane_floor`'s `MECLAW_LANE_FLOOR_G`
# (never the runner's `MECLAW_GATE_LANE_MIN_FREE_G`, which the pipeline sets
# to 0), and the check aborts only below it -- reserve + expected is how far it
# clears, not a second, higher floor. Floor 0 (off) and `MECLAW_LANE_WIPE_OK`
# are mirrored as `lane_floor` treats them: then the check never aborts.
hygiene_before() {
    local target="$1" wt="$2" tdir="$3" hrc own=() floor="${MECLAW_LANE_FLOOR_G:-60}" abort
    [ "${MECLAW_HYGIENE:-1}" != 0 ] || return 0
    [ -n "${MECLAW_LANE_WIPE_OK:-}" ] || own=(--own-target "$tdir")
    [ "$floor" = 0 ] || posint "$floor" || floor=60
    abort="$floor"
    [ -z "${MECLAW_LANE_WIPE_OK:-}" ] || abort=0
    cmd_hygiene "$target" check \
        --reserve-g "$floor" \
        --expected-g "${MECLAW_LANE_EXPECTED_G:-35}" \
        --own-tree "$wt" "${own[@]}" --abort-below-g "$abort" \
        --runner "${MECLAW_LANE_NAME:-${target#*@}}" >&2
    hrc=$?
    case "$hrc" in
        0|3) return 0 ;;
        2)   echo "lane_sync: the hygiene check on $target ABORTED the run before the gate --" \
                  "not enough room under $ROOT even after collecting (events above;" \
                  "MECLAW_HYGIENE=0 skips the check)" >&2
             return 6 ;;
        255) return 255 ;;
        *)   echo "lane_sync: the hygiene check on $target failed (exit $hrc) --" \
                  "the run goes on to the lane floor" >&2
             return 0 ;;
    esac
}

# The sweep after a gate: what the run left behind. Never part of the verdict.
hygiene_after() {
    local target="$1" hrc
    [ "${MECLAW_HYGIENE:-1}" != 0 ] || return 0
    cmd_hygiene "$target" collect --runner "${MECLAW_LANE_NAME:-${target#*@}}" >&2
    hrc=$?
    case "$hrc" in
        0|3) ;;
        *) echo "lane_sync: the hygiene collect on $target ended with exit $hrc -- the verdict stands" >&2 ;;
    esac
    return 0
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

# tv-br M10u (re-check R-delta M1): the quota mark the hang marks read
# (meclaw_testing::deadline::CPU_QUOTA_MARK_VAR), taken INSIDE the scope from
# the quota its cgroup enforces -- `cpu.max` of the scope's own cgroup (v2,
# `0::<path>` of /proc/self/cgroup), never from the quota asked for. A lane
# whose user manager has no cpu controller delegated runs the scope
# unthrottled: no `cpu.max` (or `max`), no mark -- and a time factor other than
# 1 is refused there before the tier starts. MECLAW_LANE_CGROUP_FS is the
# cgroup mount (default /sys/fs/cgroup; the pins point it at a fake tree).
# shellcheck disable=SC2016
QUOTA_MARK_SH='cg=$(sed -n "s/^0:://p" /proc/self/cgroup 2>/dev/null)
mark="" q="" p=""
{ read -r q p <"${MECLAW_LANE_CGROUP_FS:-/sys/fs/cgroup}$cg/cpu.max"; } 2>/dev/null || :
case "$q:$p" in
    *[!0-9:]*|:*|*:) ;;
    *) [ "$p" -gt 0 ] && mark=$((q * 100 / p)) ;;
esac
[ "$mark" = 0 ] && mark=""
if [ -z "$mark" ] && [ -n "${MECLAW_TEST_TIME_FACTOR+x}" ] && [ "$MECLAW_TEST_TIME_FACTOR" != 1 ]; then
    echo "lane_sync: MECLAW_TEST_TIME_FACTOR=$MECLAW_TEST_TIME_FACTOR but no effective cpu quota in the scope (cpu.max ${q:-missing} ${p}): the cpu controller is not delegated, the quota would not bite (OR-T12-13)" >&2
    exit 2
fi
export MECLAW_TEST_CPU_QUOTA="$mark"
exec "$@"'

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
    require_browsers_default
    local wt tdir args="" a envs="" name keep=1 first=1 tail=0 binargs="" width cls
    wt=$(build_path "$strand") || exit 5
    tdir=$(target_path) || exit 5
    cls=$(lane_class)
    local_needs_egon "$target"
    if [ "$cls" = egon ]; then
        [ "$target" = local ] || die "class egon is the lane of this machine -- its target is 'local'"
        [ -n "${MECLAW_LANE_WATCH:-}${MECLAW_LANE_WATCH_CMD:-}" ] \
            || die "class egon needs the unit it guards (watch=<unit> in the host file)"
    fi
    lane_floor "$target"
    width=$(lane_env test) || exit $?
    width=${width% }
    # SC2016: the HOST expands `$(nproc)`, on purpose.
    # shellcheck disable=SC2016
    case "$width" in *NEXTEST_TEST_THREADS=*) ;; *) width="${width:+$width }"'NEXTEST_TEST_THREADS=$(nproc)' ;; esac
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
    # OR-T12-13: the one time factor of the hang marks (meclaw_testing::deadline)
    # travels by exact name -- a measurement driver sets it for a quota run
    # (flake_triage.py --time-factor) -- and the run's log names it (receipt).
    # tv-br M10o (re-check B-M4): a factor other than 1 belongs to a quota run
    # only -- exported into a gate or strand run without a quota it would
    # stretch every mark and product deadline unseen. Refused before anything runs.
    local tf=""
    if [ -n "${MECLAW_TEST_TIME_FACTOR+x}" ] && [ "$MECLAW_TEST_TIME_FACTOR" != 1 ] \
        && [ -z "${MECLAW_LANE_CPU_QUOTA:-}" ]; then
        die "MECLAW_TEST_TIME_FACTOR=$MECLAW_TEST_TIME_FACTOR without MECLAW_LANE_CPU_QUOTA: a factor belongs to a quota run only (OR-T12-13)"
    fi
    if [ -n "${MECLAW_TEST_TIME_FACTOR+x}" ]; then
        tf=" MECLAW_TEST_TIME_FACTOR=$(q "$MECLAW_TEST_TIME_FACTOR")"
        echo "# time factor: MECLAW_TEST_TIME_FACTOR=$MECLAW_TEST_TIME_FACTOR"
    fi
    local before="" rc
    if [ -n "$envs" ]; then
        before=$(mktemp) || die "no temp file"
        remote "$target" "cd $(q "$wt") && bash -c $(q "$GOLDEN_HASH")" >"$before" \
            || { rm -f "$before"; die "the golden hashes on $target failed"; }
    fi
    # TV-F7 (flake_triage.py --throttle): the tier in a user scope with a CPU
    # quota (systemd's unit, 100 = one core); a build lane only, never egon.
    local wrap=""
    if [ -n "${MECLAW_LANE_CPU_QUOTA:-}" ]; then
        case "$MECLAW_LANE_CPU_QUOTA" in
            *[!0-9]*|0*) die "MECLAW_LANE_CPU_QUOTA: a whole percent above 0, got '$MECLAW_LANE_CPU_QUOTA'" ;;
        esac
        [ "$cls" = egon ] && die "MECLAW_LANE_CPU_QUOTA: a build lane only, never class egon"
        wrap="systemd-run --user --scope -q -p CPUQuota=${MECLAW_LANE_CPU_QUOTA}% -- bash -c $(q "$QUOTA_MARK_SH") quota-mark "
    fi
    # TV-B7 (GH #1210): the Chromium brake of the browser drivers
    # (workshop/tools/lib/cpu-throttle.mjs) is an environment word, and ssh
    # does not carry it -- measured: `strand.sh test` with it set ran the
    # drivers unbraked. It travels by name, a number only, and the log says
    # so once at the top; each braked driver's own THROTTLE line is printed by
    # its Rust test (re-check tv-bn-drv2-fix1 § 5). Not a golden switch: it
    # brings nothing back.
    local brake=""
    if [ -n "${MECLAW_BROWSER_CPU_THROTTLE:-}" ]; then
        # The drivers' own rule (`throttleRate`): a number, and >= 1 -- `0` or
        # `0.5` would only die on the lane, after the sync (re-check
        # tv-bn-drv2-fix1 § 5). Only zeros before the point is below 1.
        [[ "$MECLAW_BROWSER_CPU_THROTTLE" =~ ^[0-9]+(\.[0-9]+)?$ \
           && ! "$MECLAW_BROWSER_CPU_THROTTLE" =~ ^0+(\.[0-9]+)?$ ]] \
            || die "MECLAW_BROWSER_CPU_THROTTLE: a slowdown factor >= 1, got '$MECLAW_BROWSER_CPU_THROTTLE'"
        brake=" MECLAW_BROWSER_CPU_THROTTLE=$MECLAW_BROWSER_CPU_THROTTLE"
        echo "=== browser cpu brake: x$MECLAW_BROWSER_CPU_THROTTLE (MECLAW_BROWSER_CPU_THROTTLE, Chromium CDP)"
    fi
    # tv-br M10q-a (re-check R K3): the quota mark the hang marks read
    # (meclaw_testing::deadline::CPU_QUOTA_MARK_VAR) -- set inside the scope
    # that applies the quota (QUOTA_MARK_SH, from the enforced `cpu.max`, tv-br
    # M10u) and nowhere else; empty in front of it, so a stray export never
    # crosses as one. `time_factor()` refuses a factor other than 1 without
    # it, which also covers every run that never passes through here (a local
    # tier, a main-tree gate, cargo by hand).
    local qmark
    qmark=" MECLAW_TEST_CPU_QUOTA="
    local cmd words
    words=""; env_word words CARGO_TARGET_DIR "$tdir"; env_word words MECLAW_TIER_LANE 1
    words+="${width}$envs$brake$tf$qmark"
    lane_words_known "$words"
    cmd="export PATH=\"\$HOME/.cargo/bin:\$HOME/.local/bin:\$PATH\" && cd $(q "$wt") && $words"
    if [ "$cls" = egon ]; then
        # The protection class of the owner's machine (BUILD-PREAMBLE 8): the
        # tier takes the machine's cargo lock itself (never a second flock
        # around it -- that deadlocks), and the watchdog guards the run.
        echo "# cargo hygiene: class egon -- nice -n 19 ionice -c3, $width, cargo lock ${MECLAW_GATE_LOCK:-/tmp/meclaw-w26-cargo.lock} (taken by the tier), watch ${MECLAW_LANE_WATCH:-cmd}"
        run_guarded "$cmd nice -n 19 ionice -c3 scripts/test-tier.sh filter$args"
    else
        remote_long "$target" "$cmd ${wrap}scripts/test-tier.sh filter$args"
    fi
    rc=$?
    [ -n "$envs" ] || return "$rc"
    if [ "$rc" = 255 ]; then rm -f "$before"; return "$rc"; fi
    golden_back "$target" "$wt" "$before" || { rm -f "$before"; return 2; }
    rm -f "$before"
    return "$rc"
}

# `check`: the `cargo check` of `strand.sh pre` (W4, GH #1118), in the build
# path the push just checked out -- the same `build_path`/`target_path`, the
# floor and the lane's environment (`lane_env strand`: width, incremental,
# sccache, linker) as a gate run, never paths of its own (review I2). The
# check has no tier, so the host runs the ghost-binary guard of
# `scripts/cargo-target.sh` in front of cargo: the shared target is synced
# for this tree and `.gate-tree` names the build path BEFORE cargo fills it,
# the order `scripts/test-tier.sh` keeps. A class `egon` lane is refused like
# a gate (`MECLAW_LANE_EGON_GATE_OK` overrides). The lane hygiene runs
# first, then the floor, as in `run` (W2): the check protects the run's own
# target and exits 6 when the host has no room; the sweep after (`collect`)
# stays with the gate. The library is sourced fail-closed like every kit
# source (`|| exit 2`; test_lane_engstelle KitShellClass reads this string
# too): a tree without it stops before cargo.
# shellcheck disable=SC2016
CHECK_ON_HOST='root=$PWD && { . "$root/scripts/cargo-target.sh" || exit 2; } && rev=$(git rev-parse HEAD) &&
dirty=$(meclaw_dirty_files "$root") &&
note=$(meclaw_tree_sync "$root" "$CARGO_TARGET_DIR" "$rev" "$dirty") &&
{ [ -z "$note" ] || echo "=== tree-sync: $note"; } &&
meclaw_write_stamp "$root" "$CARGO_TARGET_DIR" "$rev" "$dirty"'

cmd_check() {
    local target="$1" strand="$2"; shift 2
    local wt tdir args="" a envs
    wt=$(build_path "$strand") || exit 5
    tdir=$(target_path) || exit 5
    local_needs_egon "$target"
    if [ "$(lane_class)" = egon ] && [ -z "${MECLAW_LANE_EGON_GATE_OK:-}" ]; then
        echo "lane_sync: a class egon lane runs 'test' only -- no check there" \
             "(MECLAW_LANE_EGON_GATE_OK=<reason> overrides)" >&2
        exit 2
    fi
    hygiene_before "$target" "$wt" "$tdir" || return $?
    lane_floor "$target"
    envs=$(lane_env strand) || exit $?
    for a in "$@"; do args+=" $(q "$a")"; done
    remote_long "$target" \
        "export PATH=\"\$HOME/.cargo/bin:\$HOME/.local/bin:\$PATH\" && cd $(q "$wt") && export CARGO_TARGET_DIR=$(q "$tdir") && bash -c $(q "$CHECK_ON_HOST && exec env ${envs}nice -n 19 ionice -c3 cargo check$args")"
}

# The identity of a machine: `/etc/machine-id`, then the host name, each on a
# prefixed line. The prefixes let the reader tell the answer from noise a login
# shell prints around it (W6 review M1: a banner line or a glued line read as
# the machine-id opened the check).
# shellcheck disable=SC2016  # expanded on the machine that runs it, here or there
# Two printf calls (re-check M7d B-H1): `\nHOST=` in one format read as a word
# of the command at the narrow point (`remote_words_known`); the answer is the same.
IDENTITY='printf "%s=%s\n" MID "$(cat /etc/machine-id 2>/dev/null)" HOST "$(hostname 2>/dev/null)"'
# A clean answer is exactly these two lines, each value non-empty and without
# blanks -- anything else (a banner, two ids, an empty id or name) is no proof.
IDENTITY_RE=$'^MID=([^[:space:]]+)\nHOST=([^[:space:]]+)$'

# `not_this_machine <target>` (W6 review I2): the label check of the host file
# is not enough -- a lane entry may point at this very machine under another
# name. The target's machine-id and host name are compared with this side's;
# either one equal refuses with exit 5. Fail-closed on both sides (review M1):
# this side's identity or the target's that is not exactly one non-empty
# machine-id line and one non-empty host name line refuses with exit 5 too --
# an empty or polluted identity can never prove another machine. ssh exit 255
# on the probe is an unreachable lane, exit 2 (GH #1133 M2).
not_this_machine() {   # <target>
    local here there here_id here_name there_id there_name rc
    here=$(bash -c "$IDENTITY")
    if ! [[ $here =~ $IDENTITY_RE ]]; then
        echo "lane_sync: refused -- the identity of this machine cannot be read (exactly one" \
             "non-empty machine-id and host name each); nothing proves $1 is another machine" >&2
        exit 5
    fi
    here_id=${BASH_REMATCH[1]}; here_name=${BASH_REMATCH[2]%%.*}
    there=$(remote "$1" "$IDENTITY"); rc=$?
    # ssh's own failure (255) is a lane that is not there, not a refusal (GH #1133 M2).
    if [ "$rc" = 255 ]; then
        echo "lane_sync: $1 is unreachable (ssh exit 255 on the identity probe)" >&2  # lane-engstelle-ausnahme: message text, no process
        exit 2
    fi
    if [ "$rc" != 0 ] || ! [[ $there =~ $IDENTITY_RE ]]; then
        echo "lane_sync: refused -- the identity of $1 cannot be read (exactly one non-empty" \
             "machine-id and host name each, no other output); the coverage run goes to a" \
             "proven build lane only" >&2
        exit 5
    fi
    there_id=${BASH_REMATCH[1]}; there_name=${BASH_REMATCH[2]%%.*}
    if [ "$there_id" = "$here_id" ] || [ "${there_name,,}" = "${here_name,,}" ]; then
        echo "lane_sync: refused -- $1 is this machine (machine-id or host name" \
             "'$there_name' equal); the coverage run never runs on the owner's machine" >&2
        exit 5
    fi
}

# `coverage`: the instrumented coverage run of the impact graph (W6, GH
# #1120) through the lane engine (review I1) -- push into the layout element
# `coverage/tree`, the hygiene check with that tree and `coverage/target` as
# own, the fail-closed floor, the host part `scripts/pipeline/coverage_host.sh`
# (cwd = the tree; it leaves the map in `coverage/coverage.json`), the sweep.
# Never on the owner's machine: `local` and class `egon` are refused, exit 5
# (review I2) -- before anything leaves this tree -- and so is a target whose
# machine-id or host name is this machine's (`not_this_machine`). Exit = the
# host part's, 255 when ssh lost the host. `coverage/tree.lock` is held from
# before the push until the map is fetched into <out-dir> (GH #1133 M4, as a
# gate holds from push to fetch); busy is exit 5.
cmd_coverage() {
    local target="$1" strand="$2" deadline="$3" out="${4:-}" scope="${5:-full}" wt tdir sha rc word nonce
    LANE_COVERAGE=1
    require_browsers_default
    if [ "$target" = local ] || [ "$(lane_class)" = egon ]; then
        echo "lane_sync: refused -- the coverage run never runs on the owner's machine" \
             "(local or class egon); a build lane of the wave only" >&2
        exit 5
    fi
    posint "$deadline" || die "coverage: the deadline '$deadline' is not a number of seconds"
    # S1b (#1165): `browser` probes the browser locks alone; `full` keeps the host call as it was
    case "$scope" in full|browser) ;; *) die "coverage: the scope '$scope' is not full|browser" ;; esac
    not_this_machine "$target"
    wt=$(build_path "$strand") || exit 5
    tdir=$(target_path) || exit 5
    sha=$(git rev-parse HEAD) || die "no HEAD"
    # The guards of a gate run (W3): hygiene, then the floor. The refusal of
    # the owner's machine above stays first and is not overridable.
    run_guards "$target" "$wt" "$tdir" || return $?
    nonce=$(hold_nonce)
    coproc COV_HOLD { remote_hold "$target" "bash -c $(q "$HOLD_SH") hold $(q "$wt.lock") $(q "$nonce")"; }
    read -r word _ <&"${COV_HOLD[0]}" || word=""
    if [ "$word" != held ]; then
        [ "$word" = busy ] || die "the host $target did not answer"
        echo "lane_sync: refused -- the build path $wt on $target is in use" >&2
        exit 5
    fi
    MECLAW_LANE_HOLD=$nonce cmd_push "$target" "$strand" >&2
    remote_long "$target" \
        "export PATH=\"\$HOME/.cargo/bin:\$HOME/.local/bin:\$PATH\" && cd $(q "$wt") && $(verdict_words)scripts/pipeline/coverage_host.sh $(q "$tdir") $(q "$deadline") $(q "$sha")$([ "$scope" = full ] || printf ' %s' "$(q "$scope")")"
    rc=$?
    if [ "$rc" = 0 ] && [ -n "$out" ]; then
        if ! { mkdir -p "$out" && lane_rsync "$target" -a "%LANE%$ROOT/coverage/coverage.json" \
                   "$out/coverage.json"; }; then
            echo "lane_sync: the coverage map did not come back from $target" >&2
            rc=1
        elif [ "$MECLAW_REQUIRE_BROWSERS" != 1 ]; then
            # Re-check tv-bn M7c M1: the opt-out stood only on stderr, the map
            # read like a full run -- the hole of a SKIPped driver is named in
            # the receipt (`browsers_opt_out`, kept and journaled by
            # `pipeline coverage build`).
            "${KIT_PY[@]}" -c 'import json, sys
p, v = sys.argv[1], sys.argv[2]
cov = json.load(open(p, encoding="utf-8"))
cov["browsers_opt_out"] = v
json.dump(cov, open(p, "w", encoding="utf-8"), sort_keys=True)' "$out/coverage.json" "$MECLAW_REQUIRE_BROWSERS" || rc=1
        fi
    fi
    # A hold that ended early is already reaped and its variables gone (as in
    # push_unlock), or `wait` reports its non-zero end (ssh 255). Either way
    # the map came back without the tree lock (coverage_lane.sh fetches it
    # "under the tree lock"): the lost lock is said aloud and a green run
    # turns red (re-check 8d26c0ec4 I-1); the hygiene below still runs.
    local hold_lost=""
    if [ -n "${COV_HOLD_PID:-}" ] && [ -n "${COV_HOLD[1]:-}" ]; then
        local hold_pid="$COV_HOLD_PID"
        eval "exec ${COV_HOLD[1]}>&-"
        wait "$hold_pid" 2>/dev/null || hold_lost=1
    else
        hold_lost=1
    fi
    if [ -n "$hold_lost" ]; then
        echo "lane_sync: the hold on $wt.lock on $target ended before the run did" >&2
        if [ "$rc" = 0 ]; then rc=1; fi
    fi
    [ "$rc" = 255 ] || hygiene_after "$target"
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
        lane_rsync "$target" -a --from0 --files-from="$list" "%LANE%$wt/" "$top/" \
            || { rm -f "$list" "$after"; echo "lane_sync: golden fetch failed -- rsync from $target" >&2; return 1; }  # lane-engstelle-ausnahme: message text, no process
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
    lane_rsync "$target" -a "%LANE%$ROOT/runs/$strand/" "$dest/" \
        || die "could not fetch the receipt from $target"
}

# The fingerprint of the lane (V0 section 2.3) as one JSON line: the script
# `scripts/pipeline/lane_fingerprint.sh` of THIS tree runs there on stdin.
cmd_fingerprint() {
    local target="$1" fp here pol words rpath
    here="$KIT_HERE/pipeline/lane_fingerprint.sh"
    [ -f "$here" ] || die "no fingerprint script at $here"
    pol=""   # the policy itself is the script's (re-check tv-bn M7q, one source)
    [ "$(lane_class)" = egon ] && pol="class=egon"
    # under the stations' words (`lane_env`): the linker and the build environment
    # are read from them there, never sent as a word of their own (M7r A-M2)
    words=$(lane_env test) || die "no lane environment for $target"
    # under the stations' PATH too (`lane.REMOTE_PATH`, Runde 3 R3 (4)): the register
    # measured the login PATH, every run the stations' -- a `lane-drift [build_env]`;
    # the one spelling is lane.py's, read from there (review v2-r3-kit M3)
    rpath=$("${KIT_PY[@]}" -c 'import sys; sys.path.insert(0, sys.argv[1]); from pipeline import lane; print(lane.REMOTE_PATH)' "$KIT_HERE") \
        || rpath=""
    [ -n "$rpath" ] || die "no lane.REMOTE_PATH under $KIT_HERE/pipeline"
    fp=$(remote "$target" "$rpath && env $words sh -s -- $(q "$ROOT") $(q "$pol")" <"$here") \
        || die "the host $target did not answer"
    printf '%s\n' "$fp"
}

# The provisioning of a browser engine (GH #1161): `pipeline/lane_browser_deps.sh`
# runs on the lane with the playwright version this tree pins in
# `workshop/tools/package.json` -- the package list is that version's
# `install-deps --dry-run`, never one kept here. Its exit code is the answer.
cmd_browsers() {
    local target="$1" mode="$2" here pkg pin bound words=""
    shift 2
    # `install` is apt as root: never on this machine or the owner's (re-check
    # tb-s3 h2 M-1; strand.sh refuses first, this is the second line).
    if [ "$mode" = install ] && { [ "$target" = local ] || [ "$(lane_class)" = egon ]; }; then
        die "browsers: never installs on $target (class $(lane_class)) -- packages there are the owner's call"
    fi
    here="$KIT_HERE"
    pkg="$here/../workshop/tools/package.json"
    pin=$(sed -n 's/^ *"playwright": *"\([^"]*\)".*/\1/p' "$pkg" 2>/dev/null | head -n 1)
    [ -n "$pin" ] || die "no playwright pin in $pkg"
    for e in "$@"; do words="$words $(q "$e")"; done
    # every tool call of the recipe under the probe bound (re-check v2-r3-shell M1):
    # the block goes ahead of it on stdin, a call past it is exit 2 (cannot tell)
    bound=$(bound_block) || die "no bounded block in $here/pipeline/lane_fingerprint.sh"
    { printf 'fp_who=lane_browser_deps.sh fp_exit=2\n%s\n' "$bound"; cat "$here/pipeline/lane_browser_deps.sh"; } \
        | remote "$target" "sh -s -- $(q "$ROOT") $(q "$pin") $(q "$mode")$words"
}

# The code of the `bounded` block of lane_fingerprint.sh, without its comments
# (the narrow point reads the words of a command, prose too): the one bound of a
# lane probe's tool calls (GH #1224), for INVENTORY ($2) and the browser recipe.
bound_block() {
    local b
    b=$(sed -n '/^# >>> bounded/,/^# <<< bounded/{/^[[:space:]]*#/d;p;}' "$KIT_HERE/pipeline/lane_fingerprint.sh")
    [ -n "$b" ] && printf '%s\n' "$b"
}

# The short form: the first 8 hex of the hash over the fingerprint WITHOUT its
# free space -- the same lane gives the same short form from run to run.
fp_short() {
    sed 's/"target_free_g": [0-9a-z]*, //' | sha256sum | cut -c1-8
}

# The inventory of the lane, run THERE by `bash -c` with the root as $1:
# `key value` lines. Generations: test binaries (`<stem>-<16 hex>`, executable)
# in target/debug/deps against their distinct stems -- 5 binaries per stem are
# 5 generations of artefacts nobody clears yet (W2). Every tool call runs under
# the bound of `lane_fingerprint.sh` (its `bounded` block, $2, GH #1224):
# a hung call is said by name with its process tree and the inventory ends with
# exit 3 -- `cmd_status` says `state hung` and passes the words on.
# shellcheck disable=SC2016
INVENTORY='root=$1; t="$root/target/debug"; fp_who="lane_sync.sh status"; eval "$2"
bounded df -BG --output=fstype,avail "$root" 2>/dev/null | tail -1 | awk "{print \"fs \" \$1; gsub(/[^0-9]/, \"\", \$2); print \"free \" \$2}"
g() { [ -d "$1" ] && bounded du -s -BG "$1" 2>/dev/null | awk "{gsub(/G/, \"\", \$1); print \$1}" || echo 0; }
echo "deps $(g "$t/deps")"
echo "incr $(g "$t/incremental")"
if [ -d "$t/deps" ]; then
    bounded find "$t/deps" -maxdepth 1 -type f -perm -u+x -regextype posix-extended -regex ".*/[a-z0-9_]+-[0-9a-f]{16}" -printf "%f\n" 2>/dev/null \
        | awk "{ n++; sub(/-[0-9a-f]+\$/, \"\"); s[\$0] = 1 } END { k = 0; for (x in s) k++; print \"gen \" n + 0 \"/\" k }"
else echo "gen 0/0"; fi
echo "wt $(bounded find "$root/wt" -mindepth 1 -maxdepth 1 -type d 2>/dev/null | wc -l)"
if test -d "$root/meclaw.git"; then echo "bare $(bounded git --git-dir="$root/meclaw.git" for-each-ref refs/heads/lane | wc -l)"; else echo "bare none"; fi
PATH="$HOME/.local/bin:$HOME/.cargo/bin:$PATH"
if command -v sccache >/dev/null 2>&1 && [ -d "$root/sccache" ]; then
    SCCACHE_DIR="$root/sccache" bounded sccache --show-stats 2>/dev/null | awk "/^Cache hits rate  / { gsub(/ /, \"\"); sub(/Cachehitsrate/, \"\"); print \"sccache \" \$0; f = 1 } END { if (!f) print \"sccache on\" }"
else echo "sccache -"; fi
if bounded pgrep -x cargo >/dev/null 2>&1 || bounded pgrep -x cargo-nextest >/dev/null 2>&1 || bounded pgrep -x rustc >/dev/null 2>&1; then echo "state busy"; else echo "state idle"; fi'

cmd_status() {
    local target="$1" out err rc fp bound
    # One line per lane for `strand.sh lanes status`: reachable or not, the
    # file system and free G of the lane root, the bare repository, and the
    # inventory (GH #942, W1): deps-G, incremental-G, generations
    # (binaries/stems), trees in wt/, slots, sccache, fingerprint, state.
    # Unreachable is `off` (no answer) or `booting` (the host refuses ssh:
    # it is up, sshd is not yet).
    err=$(mktemp) || die "no temp file"
    bound=$(bound_block) || { rm -f "$err"; die "no bounded block in $KIT_HERE/pipeline/lane_fingerprint.sh"; }
    out=$(remote "$target" "bash -c $(q "$INVENTORY") _ $(q "$ROOT") $(q "$bound")" 2>"$err")
    rc=$?
    if [ "$rc" = 3 ] && grep -q ' hung past ' "$err"; then
        cat "$err" >&2   # the hung call by name, with its process tree (GH #1224)
        rm -f "$err"
        echo "reachable  state hung"
        return 0
    fi
    if [ "$rc" != 0 ] || [ -z "$out" ]; then
        if grep -q 'Connection refused' "$err"; then echo "unreachable  state booting"
        else echo "unreachable  state off"; fi
        rm -f "$err"
        return 0
    fi
    rm -f "$err"
    # a hung probe call or a toolchain the probe will not install names itself on
    # stderr (GH #1224); the rest stays quiet
    fp=$(cmd_fingerprint "$target" 2> >(grep ' hung past \| gave no answer within \| toolchain not installed ' >&2) | fp_short)
    kv() { printf '%s\n' "$out" | awk -v k="$1" '$1 == k { $1 = ""; sub(/^ /, ""); print; exit }'; }
    local bare fs free
    bare=$(kv bare); fs=$(kv fs); free=$(kv free)
    [ "$bare" = none ] || bare="${bare:-0} refs"
    printf 'reachable  fs %s  free %sG  bare %s  deps %sG  incr %sG  gen %s  wt %s  slots %s  sccache %s  fp %s  state %s\n' \
        "${fs:-?}" "${free:-?}" "$bare" "$(kv deps)" "$(kv incr)" \
        "$(kv gen)" "$(kv wt)" "$(lane_slots)" "$(kv sccache)" "${fp:-?}" "$(kv state)"
}

# A strand's name becomes a ref (`refs/heads/lane/<strand>`), a path
# (`wt/<strand>`, `runs/<strand>`) and a lock name. Kit find tv-br M5 (Welle
# Testing M7d): the name of a second lease (`tv-br:m2`) reached the push
# refspec, whose colon splits src:dst, and the push failed on the lane.
# What a ref cannot carry is refused before anything travels (exit 2); the
# Python twin is `pipeline/lane.py _check_name`.
strand_name_ok() {   # <strand>
    local s="$1"
    if [ -z "$s" ] || [[ "$s" =~ [:~^?*\[\\[:space:][:cntrl:]] ]] || [[ "$s" == *@\{* ]] \
        || [[ "$s" == -* ]] || [[ "$s" == /* ]] || [[ "$s" == *.lock ]] || [[ "$s" == *. ]] \
        || [[ "/$s/" == */../* ]] || [[ "$s" == *//* ]]; then
        die "not a strand name: '$s' -- it becomes a ref and a path on the lane" \
            "(a second lease's holder goes as --holder, never in the name)"
    fi
}

[ $# -ge 1 ] || die "expected files|digest|push|hold|check|coverage|test|fetch|status|fingerprint|browsers|env|hygiene|paths|guard|checked"
sub="$1"; shift
case "$sub" in
    push|hold|check|coverage|test|fetch|guard) [ $# -lt 2 ] || strand_name_ok "$2" ;;
    paths) [ $# -lt 1 ] || strand_name_ok "$1" ;;
esac
case "$sub" in
    files)  cd "$(git rev-parse --show-toplevel 2>/dev/null)" || die "not a git repository"
            overlay_files ;;
    digest) cd "$(git rev-parse --show-toplevel 2>/dev/null)" || die "not a git repository"
            digest_list | bash -c "$DIGEST" ;;
    push)   [ $# -eq 2 ] || die "push <ssh-target> <strand>"; cmd_push "$@" ;;
    hold)   [ $# -eq 2 ] || die "hold <ssh-target> <strand>"; cmd_hold "$@" ;;
    check)  [ $# -ge 2 ] || die "check <ssh-target> <strand> [cargo check args]"; cmd_check "$@" ;;
    coverage) { [ $# -ge 3 ] && [ $# -le 5 ]; } || die "coverage <ssh-target> <strand> <deadline-s> [<out-dir> [full|browser]]"
              cmd_coverage "$@" ;;
    test)   [ $# -ge 3 ] || die "test <ssh-target> <strand> <filterset> [nextest args]"; cmd_test "$@" ;;
    fetch)  [ $# -eq 3 ] || die "fetch <ssh-target> <strand> <dir>"; cmd_fetch "$@" ;;
    status) [ $# -eq 1 ] || die "status <ssh-target>"; cmd_status "$@" ;;
    fingerprint) [ $# -eq 1 ] || die "fingerprint <ssh-target>"; cmd_fingerprint "$@" ;;
    browsers) case "$#:${2:-}" in [2-9]:check|[2-9]:install|[1-9][0-9]:check|[1-9][0-9]:install) ;;
                  *) die "browsers <ssh-target> check|install [<engine>...]" ;; esac
              cmd_browsers "$@" ;;
    env)    [ $# -eq 1 ] || die "env <mode>"; lane_env "$1"; echo ;;
    hygiene) cmd_hygiene "$@" ;;
    paths)  [ $# -eq 1 ] || die "paths <strand>"
            wt=$(build_path "$1") || exit 5; tdir=$(target_path) || exit 5
            printf '%s\n%s\n' "$wt" "$tdir" ;;
    guard)  [ $# -eq 2 ] || die "guard <ssh-target> <strand>"
            wt=$(build_path "$2") || exit 5; tdir=$(target_path) || exit 5
            run_guards "$1" "$wt" "$tdir" || exit $?
            printf '%s\n%s\n' "$wt" "$tdir" ;;
    checked) [ $# -eq 1 ] || die "checked ssh|rsync|git"
            case "$1" in ssh|rsync|git) ;; *) die "checked ssh|rsync|git" ;; esac
            tool_bin "$1"; printf '%s\n' "$TOOL_CHECKED" ;;
    *) die "unknown subcommand: $sub" ;;
esac
}; exit $?
