#!/bin/sh
# meclaw -- the fingerprint of a lane (V0 section 2.3), one JSON object.
#
#     sh scripts/pipeline/lane_fingerprint.sh [<lane root>]
#     sh scripts/pipeline/lane_fingerprint.sh --tree <tree> <target> [<lane root>]
#     sh scripts/pipeline/lane_fingerprint.sh --browsers <tree> [<lane root>]
#     sh scripts/pipeline/lane_fingerprint.sh --build-env <tree>
#
# THE one producer of a fingerprint (re-check tv-bn M7q M1, OR-T11-22 "one fact,
# one source"): `lanes fingerprint` writes its answer into the register, and the
# executor's `lane_fingerprint` -- the run's own, which the register keeps after
# the run and the cache keys and the drift NOTE compare -- is this script too,
# with `--tree`: the playwright of the tree the gate runs in (or the lane core
# tree's for a tree without `workshop/tools`, #1263), the file system of its target; the browsers it asks with `--browsers` (`executor.browser_probe`). Two producers gave two forms (`Python 3.12.3`
# here, `3.12.3` there; `ext4` here, `ext2/ext3` there): after every
# `lanes fingerprint` a cache miss, a cache doubt and a `lane-drift`.
# `--browsers` prints only the `browsers` field, for the tree the executor runs a
# gate in (`executor.browser_probe`): one probe, the same answer from both sides.
#
# Runs ON the lane, POSIX sh, no Python package: `lane_sync.sh fingerprint`
# sends it over ssh on stdin (`sh -s -- <root>`), so the lane needs no copy of
# the kit. Every field is present; a tool the lane does not have is `null`.
# A drift from the register is a NOTE `lane-drift` of the executor (W3), not
# a refusal here -- this script only reports.
#
# Fields: rustc cargo nextest node playwright python kernel vcpu cores ram_g
# target_fs target_free_g ulimit_n incremental_policy linker build_env cargo_config
# sccache linger pdftotext portal_venv browsers.
#
# The toolchain is the TREE's (re-check bundle N kit-fp M1, N-K1): rustup picks
# rustc and cargo by the directory they start in (`rust-toolchain.toml` of the
# tree). The caller starts this script in the tree (`executor`: `cwd=` the tree)
# or, on a host before the tree is there, in a directory holding the tree's
# toolchain file (`executor.stations_build`) -- never in a login directory, where
# `rustup default` answers. The register's `lanes fingerprint` runs in $HOME: its
# rustc and cargo are the lane's default, overlaid by the build part before a
# cache lookup (`gate.keyed_fingerprint`).
#
# `linker`, `build_env` and `incremental_policy` are read from the environment the script runs in --
# the stations' (re-check tv-bn M7r A-M2): the executor's `base_env`, the gate's
# station environment before its cache lookup (`--build-env`, only these two),
# `lanes fingerprint` under the lane's `lane_sync.sh env` words. Never echoed by
# a caller, never measured a second time.

# One group, parsed whole before its first command runs: an in-place edit of this file under a running shell changes nothing that runs (GH #1223).
{
only_browsers=""
tree_dir=""
only_build=""
lane_core=""
if [ "${1:-}" = "--browsers" ]; then only_browsers=1; shift; lane_core="${2:-}"; fi
if [ "${1:-}" = "--build-env" ]; then only_build=1; tree_dir="${2:-}"; shift; [ $# -eq 0 ] || shift; fi
if [ "${1:-}" = "--tree" ]; then tree_dir="${2:-}"; shift 2; lane_core="${2:-}"; fi
root="${1:-/srv/target}"
# Whether the stations build incrementally: CARGO_INCREMENTAL of the environment
# the script runs in, `default` when it is unset (cargo's profile default) --
# measured, never a literal, never a caller's echo (re-check tv-bn M7t A-M3: the
# literal policy said the passes built without increments while integration and release built
# incrementally). Which mode sets what is lane_sync.sh `lane_env`'s alone.
if [ -n "${CARGO_INCREMENTAL+x}" ]; then incremental="$CARGO_INCREMENTAL"; else incremental="default"; fi

PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"
export PATH
# The probe measures, it never installs (re-check v2-r3-shell I1): a tree whose
# pinned toolchain the host lacks made rustup's proxy download it under the bound
# of the first `rustc --version` -- said as a hang, killed mid-install. Without
# auto-install the proxy answers at once that the toolchain is not installed, and
# the probe says so by name (`not_installed`). Only this process and its calls:
# the stations build with their own environment and install as before.
RUSTUP_AUTO_INSTALL=0
export RUSTUP_AUTO_INSTALL

# >>> bounded -- the bound of a lane probe's tool calls; lane_sync.sh runs this
# block before its INVENTORY (one source, GH #1224). POSIX sh, lower-case names.
# Every tool call of a lane probe runs under its own bound: one call that hangs
# (build01 15.10.: `cargo --version`, 79 min until the station's hang mark) is
# said by name the moment it passes the bound, with the process tree it hangs in
# (`ps --forest` with the wait channel: what it waits on), and the probe ends with
# exit 3 -- no answer, fail-closed like any other (fp_exit: a caller whose 3 means
# something else names its own code, lane_browser_deps.sh 2). The tool runs in a session of
# its own: its tree is one group, read whole and killed whole. Two lines on
# stderr, the last names the call and carries the tree (a caller keeps the last).
# MECLAW_FP_TOOL_SECS: the bound in seconds (default 20 -- the executor gives the
# whole script 60; measured 16.10. on build03 and mm-os01, the slowest probe call,
# `install-deps --dry-run` of lane_browser_deps.sh, had a P95 of 1.4 s: 20 s is over
# 3x P95 with room for load; a download is no probe call, the probe never installs).
# Tests keep the hang marker and fire the bound from the hung fake. Without
# `timeout` or `setsid` on the host: unbounded.
fp_secs="${MECLAW_FP_TOOL_SECS:-20}"
case "$fp_secs" in ''|*[!0-9]*|0) fp_secs=20 ;; esac
fp_who="${fp_who:-lane_fingerprint.sh}"
fp_exit="${fp_exit:-3}"
fp_main=$$
exec 9>&2
trap 'exit "$fp_exit"' USR2
fp_bounded=""
if command -v timeout >/dev/null 2>&1 && command -v setsid >/dev/null 2>&1; then fp_bounded=1; fi
# The watch of one call ($1 who, $2 bound, $3 the probe's pid, then the call): the
# call in its own session, the wait for it ended by the bound's USR1.
# shellcheck disable=SC2016
fp_watch='w=$1; s=$2; m=$3; shift 3; x=$*; c=""
fp_hung() {
    kill -s USR2 "$m" 2>/dev/null
    echo "$w: '"'"'$x'"'"' gave no answer within ${s}s -- the probe ends without an answer (GH #1224)" >&9
    t=$(ps -o pid=,ppid=,stat=,etime=,wchan:24=,args= --forest -s "$c" 2>/dev/null | sed "s/^ *//; s/  */ /g" | tr "\n" ";" | sed "s/;$//")
    echo "$w: '"'"'$x'"'"' hung past ${s}s, its process tree: ${t:-gone}" >&9
    env kill -s KILL -- "-$c" 2>/dev/null || kill -s KILL "$c" 2>/dev/null
    exit 124
}
trap fp_hung USR1
setsid "$@" </dev/null &
c=$!
wait "$c"'
# <tool> [<arg>...]: the call's output and exit, under the bound.
bounded() {
    if [ -z "$fp_bounded" ]; then "$@" </dev/null; return; fi
    timeout --foreground -s USR1 -k 5 "$fp_secs" sh -c "$fp_watch" fp_watch "$fp_who" "$fp_secs" "$fp_main" "$@"
}
# <<< bounded

# A JSON string, also for an empty value; a control character is a space
# (one in the output made the whole fingerprint no JSON, re-check tv-bn M7r A-M1).
jstr() {
    printf '"%s"' "$(printf '%s' "$1" | sed 's/\\/\\\\/g; s/"/\\"/g' | tr '\001-\037' '[ *]')"
}
# A JSON string, or null for an empty value.
js() {
    if [ -z "$1" ]; then printf 'null'; return; fi
    jstr "$(printf '%s' "$1" | tr -d '\n\r\t')"
}
# A JSON number, or null.
jn() {
    case "$1" in ''|*[!0-9]*) printf 'null' ;; *) printf '%s' "$1" ;; esac
}
# The first line of a command's output, or nothing when it is missing.
# (`--browsers` asks none of them: the executor has its own.)
first() { [ -n "$only_browsers" ] && return; bounded "$@" 2>/dev/null | head -n 1; }
# <answer> <tool> [<arg>...]: an empty answer of a rustup proxy, asked again for its
# reason -- a toolchain the tree pins and the host lacks (RUSTUP_AUTO_INSTALL=0
# above) ends the probe by name, exit 3: no answer, and no hang either.
not_installed() {
    if [ -n "$1" ] || [ -n "$only_browsers" ]; then return 0; fi
    shift
    ni_why=$(bounded "$@" 2>&1 >/dev/null | sed -n '/is not installed/{p;q;}')
    [ -n "$ni_why" ] || return 0
    echo "$fp_who: '$*' toolchain not installed -- the probe never installs (GH #1224): $ni_why" >&2
    exit 3
}

# The build environment -- THE one rule of it (re-check tv-bn M7r A-M2, OR-T11-22):
# the cache key's `build_env` (`cache.keys_for_plan`) is this script's answer, never
# a list of the kit's. Since orchestrator OR-T12-18 A-M1 (re-check tv-bn M7w A-M1)
# it is the WHOLE environment the script runs in -- the stations' -- minus the
# closed exclusion list BUILD_ENV_NOT below: a build-script name of a dependency
# (LIBSQLITE3_FLAGS, SQLITE_MAX_*, ICU4X_DATA_DIR, RING_PREGENERATE_ASM), a target
# form with dashes (CC_x86_64-unknown-linux-gnu), the C driver's CPATH or
# LIBRARY_PATH, a name nobody ever heard of: all in it. An unknown name is a cache
# miss, never a false hit; nothing to keep up to date. The names come from the
# environment block the kernel handed this process (/proc/$$/environ): dash drops a
# name that is no shell identifier from its own variables, `env` here would never
# see a dashed name. Each value enters as the first 16 hex digits of its sha256
# (a newline in it as \036): the whole environment carries session tokens, and a
# token never enters a key, a register or a receipt in clear. No /proc or no
# sha256sum: `null` -- no key (fail-closed, M7r A-M1).
#
# THE closed exclusion list: one name or glob per line, each with the one reason it
# never changes what a station builds or answers. Nothing else is left out.
BUILD_ENV_NOT='
CARGO_INCREMENTAL          how fast: incremental artefacts of the same code (lane_sync.sh sets it per mode)
CARGO_BUILD_JOBS           how fast: the number of parallel rustc jobs
NEXTEST_TEST_THREADS       how fast: the number of parallel tests (executor THREAD_KEYS)
RUST_TEST_THREADS          how fast: the number of libtest threads (executor THREAD_KEYS)
RUSTC_WRAPPER              how fast: a compiler cache (sccache) in front of the same rustc
CARGO_BUILD_RUSTC_WRAPPER  how fast: the config form of RUSTC_WRAPPER
SCCACHE_*                  how fast: where and how big the compiler cache is, never what rustc makes
CARGO_TARGET_DIR           where: the directory the artefacts land in
CARGO_BUILD_TARGET_DIR     where: the config form of CARGO_TARGET_DIR
CARGO_BUILD_BUILD_DIR      where: the directory of cargo intermediate artefacts
CARGO_HOME                 where: cargo own home; the toolchain it holds is measured by version, the config it holds as cargo_config
HOME                       where: the user home of cargo and rustup; their toolchain is measured by version, cargo config in it as cargo_config
CARGO_LOG                  what cargo prints about itself
CARGO_TERM_*               what cargo prints on the terminal (colour, progress)
CARGO_NET_*                how cargo fetches; tokens among them
CARGO_HTTP_*               how cargo fetches; tokens among them
CARGO_REGISTRY_*           how cargo fetches; tokens among them
CARGO_REGISTRIES_*         how cargo fetches; tokens among them
PWD                        the shell directory; a station runs in the tree its key names
OLDPWD                     the shell previous directory
SHLVL                      the shell nesting depth, one more per wrapper
_                          the last argument of the shell that started the script
TERM                       the terminal type of the caller; stations have no terminal
TERM_*                     the terminal program of the caller
COLORTERM                  the colour depth of the caller terminal
COLUMNS                    the width of the caller terminal
LINES                      the height of the caller terminal
LS_COLORS                  colours of ls in the caller terminal
LESS*                      the pager of the caller terminal
PAGER                      the pager of the caller terminal
MANPAGER                   the pager of man in the caller terminal
EDITOR                     the editor of the caller; nothing a station starts
VISUAL                     the editor of the caller; nothing a station starts
GIT_EDITOR                 the editor of a git commit; no station commits
GIT_PAGER                  the pager of git in the caller terminal
HIST*                      the history of the caller shell
SSH_*                      the login over ssh: client port, agent socket, tty -- new on every connection
XDG_SESSION_*              the login session: its number changes on every login
XDG_RUNTIME_DIR            the login session runtime directory
DBUS_SESSION_BUS_ADDRESS   the login session message bus socket
TMUX                       the terminal multiplexer session of the caller
TMUX_PANE                  the terminal multiplexer pane of the caller
STY                        the screen session of the caller
WINDOW                     the screen window of the caller
WINDOWID                   the X window of the caller terminal
CLAUDE*                    the agent harness of the caller: session id, process id, socket, token
VSCODE_*                   the editor server of the caller: ipc socket and session of the window
INVOCATION_ID              systemd: the id of the unit that started the executor, new per run
JOURNAL_STREAM             systemd: the journal stream of that unit
SYSTEMD_EXEC_PID           systemd: the process id of that unit
MANAGERPID                 systemd: the process id of the user manager
NOTIFY_SOCKET              systemd: the notification socket of that unit
MECLAW_GATE_ARCHIVE        the kit: where strand.sh archives the receipt, after the stations
MECLAW_GATE_WAVE           the kit: which wave directory receives the receipt
MECLAW_GATE_LOCK           the kit: the lock file of the gate queue
MECLAW_GATE_LANE_MIN_FREE_G the kit: the free-space floor of the runner on a lane (lane.env_for), where not what
MECLAW_CARGO_LOCK_HELD     the kit: whether the runner already holds the cargo lock
MECLAW_PIPELINE_DB         the kit: where the pipeline store lies
MECLAW_STRAND_TOKEN_SKIP   the kit: the ruling that lets a pass run without a lease
MECLAW_STRAND_TOKENS       the kit: where the lane leases lie
MECLAW_TOKEN_WAIT_SECS     the kit: how long a lease is waited for
MECLAW_ADMISSION_WAIT_SECS the kit: how long the admission waits
MECLAW_LANE_NAME           the kit: which lane runs it; what the lane builds with is measured
MECLAW_LANE_THREADS        the kit: a lane setting (strand.sh lane_settings); how fast, via the names above
MECLAW_LANE_JOBS           the kit: a lane setting; how fast, it becomes CARGO_BUILD_JOBS
MECLAW_LANE_SLOTS          the kit: a lane setting; how many build paths the lane grants
MECLAW_LANE_WATCH          the kit: a lane setting; the unit whose watchdog an egon run follows
MECLAW_LANE_SCCACHE        the kit: a lane setting; how fast, the compiler cache (SCCACHE_*)
MECLAW_LANE_LINKER         the kit: a lane setting; the linker is measured in its own field
MECLAW_LANE_CLASS          the kit: a lane setting; the policy it chooses is measured (incremental_policy)
MECLAW_LANE_ROOT           the kit: a lane setting; where the lane builds (target_fs measures it)
'
build_env_json() {
    [ -r "/proc/$$/environ" ] || { printf 'null'; return; }
    command -v sha256sum >/dev/null 2>&1 || { printf 'null'; return; }
    be_pats=$(printf '%s\n' "$BUILD_ENV_NOT" | sed -n 's/^\([^ ]\{1,\}\) .*/\1/p')
    be_body=$(tr '\n\0' '\036\n' <"/proc/$$/environ" | LC_ALL=C sort | {
        set -f
        be_sep=""
        be_prev=""
        while IFS= read -r be_l; do
            be_n=${be_l%%=*}
            be_v=${be_l#*=}
            if [ -z "$be_n" ] || [ "$be_n" = "$be_l" ]; then continue; fi
            # the same name twice: which one a program reads is its own choice
            [ "$be_n" != "$be_prev" ] || { printf '!dup'; exit 0; }
            be_prev="$be_n"
            be_out=""
            for be_p in $be_pats; do
                # the pattern IS a glob of BUILD_ENV_NOT (SCCACHE_*, LESS*): unquoted on purpose
                # shellcheck disable=SC2254
                case "$be_n" in $be_p) be_out=1; break ;; esac
            done
            [ -z "$be_out" ] || continue
            case "$be_n" in
                *[!A-Za-z0-9_.-]*) be_js=$(jstr "$be_n") ;;
                *) be_js="\"$be_n\"" ;;
            esac
            printf '%s%s: "%s"' "$be_sep" "$be_js" "$(printf '%s' "$be_v" | sha256sum | cut -c1-16)"
            be_sep=", "
        done
    })
    case "$be_body" in *'!dup'*) printf 'null'; return ;; esac
    printf '{%s}' "$be_body"
}
# The CARGO_TARGET_<TRIPLE>_ prefix of the triple cargo builds for:
# CARGO_BUILD_TARGET, else the host of the rustc cargo runs (`rustc -vV`); empty
# when no rustc answers. Asked only when a CARGO_TARGET_*_RUSTFLAGS/_LINKER is set.
target_prefix() {
    tp_t="${CARGO_BUILD_TARGET:-}"
    [ -n "$tp_t" ] || tp_t=$(bounded "$rustc_bin" -vV 2>/dev/null | sed -n 's/^host: //p')
    [ -n "$tp_t" ] || return 0
    printf 'CARGO_TARGET_%s_' "$(printf '%s' "$tp_t" | tr 'a-z.-' 'A-Z__')"
}
# The linker the stations link with: the one their rustflags name (cargo's order:
# CARGO_ENCODED_RUSTFLAGS, RUSTFLAGS, CARGO_TARGET_<TRIPLE>_RUSTFLAGS,
# CARGO_BUILD_RUSTFLAGS; `--ld-path=` before `-fuse-ld=` before `-C linker=`), else
# CARGO_TARGET_<TRIPLE>_LINKER, else the toolchain's own, rust-lld (re-check tv-bn
# M7t A-M1: the target's flags and linker were not followed). Read from the
# environment, never a caller's word: lane_sync.sh `lane_env` writes RUSTFLAGS from
# the host file's `linker=`, so the lane's linker is in it. A target name set while
# no triple is known: no linker (null, no key -- fail-closed).
linker_of() {
    lk_tp=""
    lk_tflags=""
    lk_tlinker=""
    lk_tset=""
    if env | grep -Eq '^CARGO_TARGET_[A-Za-z0-9_]*_(RUSTFLAGS|LINKER)='; then
        lk_tp=$(target_prefix)
        [ -n "$lk_tp" ] || return 0
        eval "lk_tset=\${${lk_tp}RUSTFLAGS+x}"
        eval "lk_tflags=\${${lk_tp}RUSTFLAGS:-}"
        eval "lk_tlinker=\${${lk_tp}LINKER:-}"
    fi
    if [ -n "${CARGO_ENCODED_RUSTFLAGS+x}" ]; then
        lk_flags=$(printf '%s' "$CARGO_ENCODED_RUSTFLAGS" | tr '\037' ' ')
    elif [ -n "${RUSTFLAGS+x}" ]; then
        lk_flags="$RUSTFLAGS"
    elif [ -n "${lk_tset:-}" ]; then
        lk_flags="$lk_tflags"
    else
        lk_flags="${CARGO_BUILD_RUSTFLAGS:-}"
    fi
    lk_ld=""; lk_fuse=""; lk_cl=""; lk_prev=""
    set -f
    for lk_w in $lk_flags; do
        case "$lk_w" in
            *--ld-path=*) lk_ld="${lk_w##*--ld-path=}" ;;
            *-fuse-ld=*) lk_fuse="${lk_w##*-fuse-ld=}" ;;
            -Clinker=*) lk_cl="${lk_w#-Clinker=}" ;;
            linker=*) [ "$lk_prev" = "-C" ] && lk_cl="${lk_w#linker=}" ;;
        esac
        lk_prev="$lk_w"
    done
    set +f
    lk="${lk_ld:-${lk_fuse:-${lk_cl:-$lk_tlinker}}}"
    lk="${lk##*/}"
    printf '%s' "${lk:-rust-lld}"
}
# The cargo configuration the stations build with from OUTSIDE the tree (re-check
# bundle N kit-fp M2, N-K2; the tree's own `.cargo/config.toml` is its source):
# cargo reads `.cargo/config{,.toml}` in every directory from the one it starts in
# up to `/`, then `$CARGO_HOME/config{,.toml}` (CARGO_HOME unset: `$HOME/.cargo`)
# -- `[build] rustflags`, `build.rustc`, `[target.<triple>] linker`, `[profile.*]`,
# `[env]`: what the stations build. CARGO_HOME and HOME stay out of `build_env`
# (where), the files they choose are this digest: the first 16 hex digits of the
# sha256 over the sha256 of each file found, in cargo's order (the paths do not
# enter: the same configuration in another home is the same build). `$1`: the
# directory the scan starts in -- the parent of the tree. A file that is there and
# cannot be read, or no sha256sum: nothing (null, no key -- fail-closed).
cargo_config_of() {
    command -v sha256sum >/dev/null 2>&1 || return 0
    case "$1" in /*) cc_d="$1" ;; *) cc_d="$PWD/$1" ;; esac
    cc_home="${CARGO_HOME:-${HOME:-}/.cargo}"
    case "$cc_home" in /*) ;; *) cc_home="$PWD/$cc_home" ;; esac
    cc_list=""
    cc_dirs=""
    while :; do
        cc_dirs="$cc_dirs$cc_d/.cargo
"
        [ "$cc_d" = "/" ] && break
        cc_d=$(dirname -- "$cc_d")
    done
    cc_dirs="$cc_dirs$cc_home
"
    cc_ifs="$IFS"
    IFS='
'
    for cc_c in $cc_dirs; do
        for cc_f in "$cc_c/config" "$cc_c/config.toml"; do
            [ -e "$cc_f" ] || continue
            { [ -f "$cc_f" ] && [ -r "$cc_f" ]; } || { IFS="$cc_ifs"; return 0; }
            cc_list="$cc_list$(sha256sum <"$cc_f" | cut -c1-64)
"
        done
    done
    IFS="$cc_ifs"
    printf '%s' "$cc_list" | sha256sum | cut -c1-16
}
# the scan of a tree starts at its parent; the register's at the lane root, the
# parent of the lane's `tree/`
if [ -n "$tree_dir" ]; then
    case "$tree_dir" in /*) cc_tree="$tree_dir" ;; *) cc_tree="$PWD/$tree_dir" ;; esac
    cc_start=$(dirname -- "$cc_tree")
else
    cc_start="$root"
fi

# The rustc the stations build with -- cargo's choice, in cargo's order: RUSTC, else
# `build.rustc` of the configuration (as CARGO_BUILD_RUSTC; its file form is
# measured as `cargo_config`), else the first rustc on PATH
# (re-check tv-bn M7w A-M2: the `rustc` field measured the PATH's while a RUSTC
# built; an update of the compiler behind the same RUSTC path was CACHED). One
# answer for the field, `target_prefix` and `--build-env`.
rustc_bin="${RUSTC:-${CARGO_BUILD_RUSTC:-rustc}}"
linker=$(linker_of)
rustc_v=$(first "$rustc_bin" --version)
cargo_v=$(first cargo --version)
not_installed "$rustc_v" "$rustc_bin" --version
not_installed "$cargo_v" cargo --version
cargo_config=$(cargo_config_of "$cc_start")
if [ -n "$only_build" ]; then
    # the build part of a fingerprint (`executor.build_fingerprint`): the gate lays it
    # over the register's before its lookup -- rustc and cargo among it, the
    # toolchain of the tree (N-K1), and the configuration outside it (N-K2)
    [ -n "$tree_dir" ] || { echo "lane_fingerprint.sh: --build-env needs the tree" >&2; exit 2; }
    printf '{"rustc": %s, "cargo": %s, "linker": %s, "build_env": %s, "cargo_config": %s}\n' \
        "$(js "$rustc_v")" "$(js "$cargo_v")" "$(js "$linker")" "$(build_env_json)" "$(js "$cargo_config")"
    exit 0
fi

nextest_v=$(first cargo nextest --version)
node_v=$(first node --version)
pw=""
pwdir=""
pw_where="workshop/tools/node_modules"
# <lane root>: the tools of the lane's core tree -- the wkenv.sh the gate sources before
# every browser driver, the playwright of the lane's cache or of the tree (lane.py THE LAYOUT)
core_tools() {
    bp_wkenv="$1/tree/workshop/tools/wkenv.sh"
    pw_dirs="$1/cache/workshop-tools/node_modules/playwright $1/tree/workshop/tools/node_modules/playwright"
}
core_tools "$root"
# a run's tree (`--browsers`, `--tree`): its own tools. A tree WITHOUT `workshop/tools`
# -- a satellite's, whose drivers import the core's -- takes the core tree of the lane
# it runs on when the caller names that lane (GH #1263, measured 18.10.: a satellite
# pass booked both engines and was RED "no playwright in workshop/tools/node_modules"
# beside a core tree with Playwright 1.63.0 whose engines started). A tree with its
# own directory keeps it, installed or not: its package.json pins the version. No
# lane named: no core guessed (a tree on this machine is no lane's).
own="$tree_dir"
[ -z "$only_browsers" ] || own="$root"
if [ -n "$own" ]; then
    if [ -d "$own/workshop/tools" ] || [ -z "$lane_core" ]; then
        bp_wkenv="$own/workshop/tools/wkenv.sh"
        pw_dirs="$own/workshop/tools/node_modules/playwright"
    else
        core_tools "$lane_core"
        pw_where="the lane's core tree ($lane_core/tree/workshop/tools/node_modules, $lane_core/cache/workshop-tools/node_modules)"
    fi
fi
for d in $pw_dirs; do
    if [ -f "$d/package.json" ]; then
        pw=$(sed -n 's/^ *"version": *"\([^"]*\)".*/\1/p' "$d/package.json" | head -n 1)
        pwdir="$d"
        break
    fi
done
# the kit's one Python (lane.PYTHON, re-check tv-bn M7p H2/M1): the version that
# enters the cache key is the one the stations run in, not the first python3 on PATH,
# in the one form `platform.python_version()` (M7q M1). The interpreter mirrors
# lane.PYTHON -- this script runs on the lane without the kit and cannot import it;
# test_lane_fingerprint holds both to one answer.
python_v=$(first /usr/bin/python3 -I -c 'import platform; print(platform.python_version())')
kernel=$(uname -r 2>/dev/null)
vcpu=$(nproc 2>/dev/null)
# the REAL cores (ruling 2026-10-10: lab slots count cores, not nproc): distinct
# sibling sets of the topology, 6 on a 6-core CPU with hyper-threading. A guest
# (`hypervisor` flag) that shows no siblings reports none: KVM shows every vCPU as
# its own core (build02: 12 x 1 on 6 real cores). A diagnostic only: the lane's
# colonies come from `cores=` in the host file, else half the vCPUs
# (`schedule.colony_capacity`), never from this count. MECLAW_FP_SYSROOT: the
# root the topology is read from (tests).
sysroot="${MECLAW_FP_SYSROOT:-}"
cpus=0
for f in "$sysroot"/sys/devices/system/cpu/cpu[0-9]*/topology/thread_siblings_list; do
    [ -f "$f" ] && cpus=$((cpus + 1))
done
cores=$(cat "$sysroot"/sys/devices/system/cpu/cpu[0-9]*/topology/thread_siblings_list 2>/dev/null | sort -u | wc -l)
[ "$cores" -gt 0 ] 2>/dev/null || cores=""
if [ -n "$cores" ] && [ "$cores" = "$cpus" ] \
        && grep -q '^flags.*[[:space:]]hypervisor\([[:space:]]\|$\)' "$sysroot"/proc/cpuinfo 2>/dev/null; then
    cores=""
fi
ram_g=$(awk '/^MemTotal:/ { printf "%d", $2 / 1048576 + 0.5 }' /proc/meminfo 2>/dev/null)
fs=$(bounded df -BG --output=fstype "$root" 2>/dev/null | tail -n 1 | tr -d ' ')
free_g=$(bounded df -BG --output=avail "$root" 2>/dev/null | tail -n 1 | tr -dc 0-9)
# dash and bash both know it; POSIX leaves it open
# shellcheck disable=SC3045
nofile=$(ulimit -n 2>/dev/null)
sccache_v=$(first sccache --version)
linger=""
if command -v loginctl >/dev/null 2>&1; then
    linger=$(bounded loginctl show-user "$(id -un)" -p Linger --value 2>/dev/null)
fi
pdf_v=$(bounded pdftotext -v 2>&1 | head -n 1)
command -v pdftotext >/dev/null 2>&1 || pdf_v=""
venv=""
[ -x "$root/pool/portal-venv/bin/python" ] && venv="$root/pool/portal-venv"

# browsers: per engine "ok" or why it cannot start -- the lane side of
# `executor.browser_probe` (GH #1161, OR-TB.S3.18/20), which runs this script with
# `--browsers`: the revision the PINNED playwright launches (its `browsers.json`),
# its bundle under PLAYWRIGHT_BROWSERS_PATH or ~/.cache/ms-playwright, `ldd` for
# every binary a headless launch starts under the library path the bundle's own
# wrapper sets, and then a REAL start (`bp_start`). Anything it cannot check is a
# reason, never "ok". The table is the executor's BROWSER_BUNDLES;
# `test_the_lane_and_the_executor_give_the_same_answer` holds both to one answer.
#
# Why a real start (build02, 10.10.): stale laboratory copies in the WebKit bundle's
# `sys/lib` shadowed the system Mesa; glvnd loads libEGL_mesa at run time, so no ldd
# chain showed it, the field said "ok", and every WebKit start died at newPage with
# "Could not create EGL display" (receipts/s3-webkit-build02/BEFUND.md). The probe
# launches, opens a context and a page, goes to a local page and evaluates in it,
# under the tree's wkenv.sh, MECLAW_REQUIRE_BROWSERS=1 and nice/ionice in the
# caller's environment (the executor's: user, HOME, PATH, PLAYWRIGHT_BROWSERS_PATH,
# the station TMPDIR) -- the way the gate drives every engine; for chromium a second
# pass with the binary the CDP drivers' findBrowser picks, then that binary started
# directly (`--headless --dump-dom about:blank`) with its own sandbox -- Playwright
# adds --no-sandbox, the gh766 units do not (tb-s10, GH #1189: build03 without the
# userns profile of `lane_browser_deps.sh` was admitted and died in `browser:display`
# with "No usable sandbox!"; a refused chromium names the install). Each engine is bounded
# by MECLAW_BROWSER_PROBE_SECS (default 30); a timeout, a signal or an exit other
# than 0 -- of ldd as of node -- is a reason. An "ok" is kept per engine under a
# key over the pinned playwright, the bundle (every file, size and time), the
# package state (dpkg status), the laboratory (wkenv.sh, MECLAW_WKDEPS), the
# kernel, node, the chromium findBrowser sees, for chromium the AppArmor userns
# profile and switch (MECLAW_APPARMOR_DIR, the profile's directory) and this probe; any change starts
# the engine again, and a failure is never kept.
bp_secs="${MECLAW_BROWSER_PROBE_SECS:-30}"
case "$bp_secs" in ''|*[!0-9]*) bp_secs=30 ;; esac
bp_cache="${MECLAW_BROWSER_PROBE_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/meclaw-browser-probe}"
bp_dpkg="${MECLAW_DPKG_STATUS:-/var/lib/dpkg/status}"
bp_wkdeps="${MECLAW_WKDEPS:-$HOME/.local/share/meclaw-wkdeps}"
bp_js=$(cat <<'PROBE'
const fs = require("fs");
const os = require("os");
const path = require("path");
const [pwdir, engine, secsArg] = process.argv.slice(1);
const secs = Math.max(1, Number(secsArg) || 30);
const logs = [];
let stage = "require";
const flat = (v) => String(v).replace(/\s+/g, " ").trim();
const errs = () => {
  const e = logs.filter((l) => /\[err\]/.test(l)).slice(-2).map(flat);
  return e.length ? " -- " + e.join(" | ") : "";
};
const say = (v, code) => process.stdout.write(flat(v).slice(0, 600) + "\n", () => process.exit(code));
const timer = setTimeout(() => say("no answer within " + secs + "s at " + stage + errs(), 1), secs * 1000);
// findBrowser: the chromium started outside the pinned playwright's choice -- the
// headless shell the gh766 units start without --no-sandbox. One root and one pick
// with `headless_shell` of lane_browser_deps.sh, the units' `playwright_chromium`
// and the AppArmor profile meclaw-playwright-chromium (tb-s10, re-check M-3):
// PLAYWRIGHT_BROWSERS_PATH, else ~/.cache/ms-playwright; of several
// chromium_headless_shell-*/chrome-headless-shell-linux64/chrome-headless-shell
// (the profile's glob) the bytewise last, never the first readdir hit.
const browsersDir = () => process.env.PLAYWRIGHT_BROWSERS_PATH || path.join(os.homedir(), ".cache", "ms-playwright");
const isFile = (p) => { try { return fs.statSync(p).isFile(); } catch (e) { return false; } };
const findBrowser = () => {
  if (process.env.MECLAW_CHROMIUM) {
    return fs.existsSync(process.env.MECLAW_CHROMIUM) ? process.env.MECLAW_CHROMIUM : null;
  }
  const root = browsersDir();
  let dirs = [];
  try { dirs = fs.readdirSync(root); } catch (e) { dirs = []; }
  const shells = dirs.filter((d) => d.startsWith("chromium_headless_shell-"))
    .map((d) => path.join(root, d, "chrome-headless-shell-linux64", "chrome-headless-shell"))
    .filter(isFile)
    .sort((a, b) => Buffer.compare(Buffer.from(a), Buffer.from(b)));
  if (shells.length) return shells[shells.length - 1];
  for (const dir of (process.env.PATH || "").split(":")) {
    for (const name of ["chromium", "chromium-browser", "google-chrome"]) {
      const exe = path.join(dir, name);
      if (fs.existsSync(exe)) return exe;
    }
  }
  return null;
};
// launch, newContext, newPage, goto a local page, evaluate, close.
const pass = async (pw, opts, tag) => {
  let browser = null;
  try {
    stage = "launch" + tag;
    browser = await pw[engine].launch(Object.assign({
      headless: true,
      timeout: secs * 1000,
      logger: { isEnabled: (name) => name === "browser", log: (name, sev, msg) => { logs.push(String(msg)); } },
    }, opts));
    stage = "newContext" + tag;
    const context = await browser.newContext();
    stage = "newPage" + tag;
    const page = await context.newPage();
    stage = "goto" + tag;
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "meclaw-browser-probe-"));
    const file = path.join(dir, "probe.html");
    fs.writeFileSync(file, "<!doctype html><p id=\"probe\">meclaw</p>");
    try {
      await page.goto("file://" + file, { timeout: secs * 1000 });
      stage = "evaluate" + tag;
      const got = await page.evaluate(() => document.getElementById("probe").textContent);
      if (got !== "meclaw") throw new Error("evaluate returned " + JSON.stringify(got));
    } finally {
      fs.rmSync(dir, { recursive: true, force: true });
    }
    stage = "close" + tag;
    await browser.close();
    browser = null;
  } finally {
    if (browser) browser.close().catch(() => {});
  }
};
(async () => {
  try {
    const pw = require(pwdir);
    await pass(pw, {}, "");
    if (engine === "chromium") {
      stage = "findBrowser";
      const exe = findBrowser();
      if (!exe) {
        throw new Error("the CDP drivers find no chromium (MECLAW_CHROMIUM, " + browsersDir() + "/chromium_headless_shell-*, PATH)");
      }
      await pass(pw, { executablePath: exe }, " of findBrowser " + exe);
      // Playwright launches chromium with its own --no-sandbox, so both passes say
      // nothing about the sandbox; the gh766 units start the shell without it (tb-s10,
      // GH #1189). Started that way once, as `lane_browser_deps.sh check` does.
      stage = "sandbox of findBrowser " + exe;
      const r = require("child_process").spawnSync(exe, ["--headless", "--dump-dom", "about:blank"], {
        stdio: ["ignore", "ignore", "pipe"], encoding: "utf8", timeout: secs * 1000, killSignal: "SIGKILL" });
      if (r.error || r.status !== 0) {
        const how = r.error ? String(r.error.message) : r.signal ? "signal " + r.signal : "exit " + r.status;
        const last = String(r.stderr || "").split("\n").map(flat).filter(Boolean).slice(-1)[0];
        throw new Error("the headless shell cannot start its own sandbox (" + how + (last ? ": " + last : "") +
                        "); provision the lane: scripts/strand.sh lanes browsers <lane> install chromium");
      }
    }
    clearTimeout(timer);
    say("ok", 0);
  } catch (e) {
    clearTimeout(timer);
    say("does not start (" + stage + "): " + String((e && e.message) || e).split("\n")[0] + errs(), 1);
  }
})();
PROBE
)
# <engine> <bundle>: "ok" when the engine starts, opens a page, takes content and
# evaluates in it; otherwise the probe's own words (stage, error, the browser's
# [err] lines).
bp_start() {
    if [ ! -f "$bp_wkenv" ]; then
        echo "no $bp_wkenv: the gate sources it before every browser driver"; return
    fi
    key=""
    if [ -r "$bp_dpkg" ]; then
        key=$( { printf '%s\n' v1 "$1" "$2" "$(id -u)" "${MECLAW_WKDEPS:-}"
                 uname -r; printf '%s\n' "${MECLAW_CHROMIUM:-}"; ls -ln "$bp_home" 2>&1
                 ls -ln "$HOME/.cache/ms-playwright" 2>&1
                 ls -ln "$bp_wkdeps" "$bp_wkdeps/root" 2>&1
                 ls -lnR "$2" 2>&1; cksum <"$bp_dpkg"; cksum <"$bp_wkenv"
                 cksum <"$pwdir/package.json"; bounded node --version 2>&1
                 if [ "$1" = chromium ]; then
                     cksum <"${MECLAW_APPARMOR_DIR:-/etc/apparmor.d}/meclaw-playwright-chromium" 2>&1
                     cat /proc/sys/kernel/apparmor_restrict_unprivileged_userns 2>&1
                 fi
                 printf '%s' "$bp_js" | cksum; } | cksum)
        if [ "$(cat "$bp_cache/$1" 2>/dev/null)" = "$key" ]; then echo "ok"; return; fi
    fi
    tmo=""
    command -v timeout >/dev/null 2>&1 && tmo="timeout -k 2 $((bp_secs + 2))"
    command -v nice >/dev/null 2>&1 && tmo="$tmo nice -n 19"
    command -v ionice >/dev/null 2>&1 && tmo="$tmo ionice -c3"
    # shellcheck disable=SC2016,SC2086
    said=$(MECLAW_REQUIRE_BROWSERS=1 $tmo sh -c '. "$1" >/dev/null 2>&1; shift; exec node -e "$@"' \
        bp "$bp_wkenv" "$bp_js" "$pwdir" "$1" "$bp_secs" </dev/null 2>/dev/null)
    rc=$?
    last=$(printf '%s\n' "$said" | sed '/^[[:space:]]*$/d' | tail -n 1)
    if [ "$rc" -eq 0 ] && [ "$last" = "ok" ]; then
        if [ -n "$key" ] && mkdir -p "$bp_cache" 2>/dev/null \
                && printf '%s\n' "$key" >"$bp_cache/$1.new" 2>/dev/null; then
            mv -f "$bp_cache/$1.new" "$bp_cache/$1"
        fi
        echo "ok"
    elif [ "$rc" -eq 124 ] || [ "$rc" -eq 137 ]; then
        echo "no answer within ${bp_secs}s from the start probe (launch, newContext, newPage, goto, evaluate)${last:+: $last}"
    elif [ "$rc" -gt 128 ]; then
        echo "the start probe died by signal $((rc - 128))${last:+: $last}"
    elif [ -n "$last" ] && [ "$last" != "ok" ]; then
        echo "$last"
    else
        echo "the start probe ended with exit $rc and no verdict"
    fi
}
bp_home="${PLAYWRIGHT_BROWSERS_PATH:-$HOME/.cache/ms-playwright}"
bp_pins=""
[ -n "$pwdir" ] && bp_pins="$pwdir/../playwright-core/browsers.json"
# The revision of <name> in the pins file, or nothing.
bp_rev() {
    awk -v want="$1" '
        /"name":/ { n = $0; sub(/.*"name": *"/, "", n); sub(/".*/, "", n) }
        /"revision":/ { r = $0; sub(/.*"revision": *"?/, "", r); sub(/[",].*/, "", r)
                        if (n == want) { print r; exit } }' "$bp_pins" 2>/dev/null
}
# <engine> <pins name> <bundle prefix> <library dirs, space separated> <binary>...
bp_probe() {
    engine="$1"; name="$2"; prefix="$3"; libs="$4"; shift 4
    if ! command -v node >/dev/null 2>&1; then echo "no node on the lane"; return; fi
    if [ -z "$bp_pins" ] || [ ! -r "$bp_pins" ]; then
        echo "no playwright in $pw_where (npm ci there)"; return
    fi
    rev=$(bp_rev "$name")
    if [ -z "$rev" ]; then echo "the pinned playwright names no $name revision"; return; fi
    bundle="$bp_home/$prefix-$rev"
    for b in "$@"; do
        if [ ! -f "$bundle/$b" ]; then
            echo "no bundle $bundle ($b missing): npx playwright install $engine"; return
        fi
    done
    if ! command -v ldd >/dev/null 2>&1; then
        echo "no ldd on the lane: the libraries of $bundle cannot be checked"; return
    fi
    llp=""
    for l in $libs; do llp="${llp:+$llp:}$bundle/$l"; done
    lost=""
    for b in "$@"; do
        # a wrapper script (build03/build04: chrome-headless-shell execs
        # chrome-headless-shell.real) is no ELF, ldd says "not a dynamic
        # executable" with exit 1 and knows nothing; the real start decides
        if [ "$(head -c 2 "$bundle/$b" 2>/dev/null)" = "#!" ]; then continue; fi
        if [ -n "$llp" ]; then
            out=$(bounded env LD_LIBRARY_PATH="$llp" ldd "$bundle/$b" 2>/dev/null)
        else
            out=$(bounded ldd "$bundle/$b" 2>/dev/null)
        fi
        # an ldd that fails says nothing about the libraries (Re-Check S3 m-1)
        lrc=$?
        if [ "$lrc" -ne 0 ]; then echo "ldd $b exited $lrc ($(basename "$bundle"))"; return; fi
        lost="$lost
$(printf '%s\n' "$out" | sed -n 's/^[[:space:]]*\(.*[^[:space:]]\)[[:space:]]*=> not found.*/\1/p')"
    done
    lost=$(printf '%s\n' "$lost" | sed '/^$/d' | LC_ALL=C sort -u | tr '\n' ' ' | sed 's/ $//')
    if [ -n "$lost" ]; then
        echo "libraries not found: $lost ($(basename "$bundle"), ldd with its own LD_LIBRARY_PATH)"
    else
        bp_start "$engine" "$bundle"
    fi
}
wpe="minibrowser-wpe/bin"
bp_chromium=""
bp_webkit=""
# `--tree`: the executor asks the browsers apart (`executor.browser_probe`, the
# `--browsers` run under the station environment), never twice per run
if [ -z "$tree_dir" ]; then
    bp_chromium=$(bp_probe chromium chromium-headless-shell chromium_headless_shell "" \
        chrome-headless-shell-linux64/chrome-headless-shell)
    bp_webkit=$(bp_probe webkit webkit webkit "minibrowser-wpe/lib minibrowser-wpe/sys/lib" \
        "$wpe/MiniBrowser" "$wpe/WPEWebProcess" "$wpe/WPENetworkProcess" "$wpe/WPEGPUProcess")
fi
if [ -n "$only_browsers" ]; then
    printf '{"chromium": %s, "webkit": %s}\n' "$(js "$bp_chromium")" "$(js "$bp_webkit")"
    exit 0
fi

printf '{"rustc": %s, "cargo": %s, "nextest": %s, "node": %s, "playwright": %s, "python": %s, ' \
    "$(js "$rustc_v")" "$(js "$cargo_v")" "$(js "$nextest_v")" "$(js "$node_v")" "$(js "$pw")" "$(js "$python_v")"
printf '"kernel": %s, "vcpu": %s, "cores": %s, "ram_g": %s, "target_fs": %s, "target_free_g": %s, "ulimit_n": %s, ' \
    "$(js "$kernel")" "$(jn "$vcpu")" "$(jn "$cores")" "$(jn "$ram_g")" "$(js "$fs")" "$(jn "$free_g")" "$(jn "$nofile")"
printf '"incremental_policy": %s, "linker": %s, "build_env": %s, "cargo_config": %s, "sccache": %s, "linger": %s, "pdftotext": %s, "portal_venv": %s, ' \
    "$(js "$incremental")" "$(js "$linker")" "$(build_env_json)" "$(js "$cargo_config")" "$(js "$sccache_v")" "$(js "$linger")" "$(js "$pdf_v")" "$(js "$venv")"
printf '"browsers": {"chromium": %s, "webkit": %s}}\n' "$(js "$bp_chromium")" "$(js "$bp_webkit")"
}; exit $?
