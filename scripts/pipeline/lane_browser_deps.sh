#!/bin/sh
# meclaw -- the system packages a browser engine needs on a lane, from the PINNED Playwright.
#
#     sh scripts/pipeline/lane_browser_deps.sh <lane root> <pinned version> check|install [<engine>...]
#
# Runs ON the lane, POSIX sh: `lane_sync.sh browsers` sends it over ssh on stdin
# (`sh -s -- ...`) with the pin of the tree, like `lane_fingerprint.sh`. Engines
# default to `chromium webkit`, the two `browser:display` books (tb-s10 deckel 17: a
# bare `check` asked webkit alone and admitted a lane whose chromium had no sandbox).
#
# Why (GH #1161): one lane's WebKit could not load libGL.so.1, and every WebKit proof
# there SKIPped green. The package list is never kept by hand: it is what
# `install-deps --dry-run <engine>` of the pinned Playwright reports missing (an
# `apt-get install -s` of its own list), and `install` runs that version's own
# `install-deps <engine>` as root -- `apt-get update` plus its list. Idempotent:
# nothing missing is "ok" without sudo. Afterwards the dry run must report nothing
# missing, or the run fails.
#
# The packages are not the whole answer (one lane, 10.10.): the WebKit bundle carries its
# own `minibrowser-wpe/sys/lib`, which Playwright's wrapper puts AHEAD of the system,
# and the laboratory recipe of `workshop/tools/wkenv.sh` copies libraries into it. A
# copied Mesa `libEGL_mesa.so.0` that links a libgallium the host does not have
# shadows the system Mesa: every package installed, and the web process still aborts
# with "Could not create EGL display: no supported platform available". So, for
# webkit, the bundle's own linker view is checked too -- `ldd` over the directories
# Playwright's host validation reads, with the wrapper's LD_LIBRARY_PATH -- and
# `install` sets a shadowing bundle aside (never deleted) and lets the pinned CLI
# install it again, as the user.
#
# Chromium has a third half (tb-s10, GH #1189): the packaged headless shell starts its
# own sandbox, and Ubuntu 24.04 (`kernel.apparmor_restrict_unprivileged_userns=1`)
# refuses it the user namespace -- "No usable sandbox!", exit 133, on build03. meclaw
# adds no `--no-sandbox` (R-G11, ADR-0044: the sandbox is the package's), so the lane
# gives the package what Ubuntu's own `chrome` profile gives Google Chrome: an
# unconfined AppArmor profile with `userns` for the headless shell of the lane's
# browser cache. `check` starts the shell once (`--dump-dom about:blank`); `install`
# writes the profile as root, loads it, and starts the shell again.
#
# Output: `browser-deps ok|missing|installed|shadowed|reinstalled|sandbox|sandboxed <engines> [<n>: <names>]` on stdout;
# every reason in plain words on stderr. Exit 0 ok, 1 packages missing (check) or
# still missing / install failed, 2 cannot tell (no node, no playwright, not the pin,
# a dry run without a package list), 3 no right to install (no root, no passwordless
# sudo). Every child reads /dev/null: on the lane this script IS stdin.

# One group, parsed whole before its first command runs: an in-place edit of this file under a running shell changes nothing that runs (GH #1223).
{
root="${1:-}"
pin="${2:-}"
mode="${3:-}"
[ $# -ge 3 ] && shift 3
engines="${*:-chromium webkit}"

PATH="$HOME/.local/bin:$HOME/.cargo/bin:$PATH"
export PATH

die() { code="$1"; shift; echo "lane_browser_deps: $*" >&2; exit "$code"; }

# The bound of every tool call that asks the lane (GH #1224, re-check v2-r3-shell
# M1): the `bounded` block of lane_fingerprint.sh, which `lane_sync.sh browsers`
# sends ahead of this script -- a call past it is said by name with its process
# tree and ends the run with exit 2 (cannot tell). Run by hand without the block,
# the calls run unbounded. The installs (apt, the bundle download) are no probe
# and keep their own time.
command -v bounded >/dev/null 2>&1 || bounded() { "$@" </dev/null; }

case "$mode" in check|install) ;; *) die 2 "usage: <lane root> <pinned version> check|install [<engine>...]" ;; esac
if [ -z "$root" ] || [ -z "$pin" ]; then die 2 "usage: <lane root> <pinned version> check|install [<engine>...]"; fi
# `$engines` goes unquoted to `install-deps`, as root on install: no glob expands
# in it, and only the three engines pass -- never another channel, never an option
# (re-check tb-s3 h2 m-3).
set -f
for e in $engines; do
    case "$e" in chromium|webkit|firefox) ;; *) die 2 "$e is not an engine (chromium, webkit, firefox)" ;; esac
done
# Globbing comes back for the bundle walk below: after this check every word of
# `$engines` is one of the three names, so no later expansion of it can glob.
set +f

node_bin=$(command -v node 2>/dev/null)
[ -n "$node_bin" ] || die 2 "no node on the lane $(hostname 2>/dev/null): the pinned playwright cannot be asked"

# The same places `lane_fingerprint.sh` reads the playwright of the lane from.
pw=""
for d in "$root"/cache/workshop-tools/node_modules/playwright "$root"/tree/workshop/tools/node_modules/playwright; do
    if [ -f "$d/package.json" ]; then pw="$d"; break; fi
done
[ -n "$pw" ] || die 2 "no playwright under $root (cache/workshop-tools or tree/workshop/tools): npm ci in workshop/tools first"
have=$(sed -n 's/^ *"version": *"\([^"]*\)".*/\1/p' "$pw/package.json" | head -n 1)
[ "$have" = "$pin" ] || die 2 "playwright $have at $pw is not the pinned $pin of the tree: npm ci there first"
cli="$pw/cli.js"
[ -f "$cli" ] || die 2 "no $cli: the playwright at $pw is incomplete"

# The missing packages, one line `<n> <pkg>...`, or a reason (exit 2).
missing() {
    # shellcheck disable=SC2086
    out=$(bounded "$node_bin" "$cli" install-deps --dry-run $engines 2>&1)
    if printf '%s\n' "$out" | grep -q '^All system dependencies are installed\.$'; then
        echo 0
        return 0
    fi
    n=$(printf '%s\n' "$out" | sed -n 's/^Missing system dependencies (\([0-9][0-9]*\)):$/\1/p' | head -n 1)
    pkgs=$(printf '%s\n' "$out" | sed -n 's/^  \([a-z0-9][a-z0-9+.-]*\)$/\1/p' | tr '\n' ' ' | sed 's/ $//')
    got=$(printf '%s' "$pkgs" | wc -w | tr -d ' ')
    if [ -z "$n" ] || [ "$n" != "$got" ] || [ "$n" = 0 ]; then
        echo "the pinned playwright $pin gave no package list for $engines: $(printf '%s' "$out" | head -n 3 | tr '\n' ' ')" >&2
        return 2
    fi
    echo "$n $pkgs"
}

# Where the pinned CLI keeps (or would keep) the webkit bundle; nothing for other engines.
bundle_dir() {
    case " $engines " in *" webkit "*) ;; *) return 0 ;; esac
    bounded "$node_bin" "$cli" install --dry-run webkit 2>/dev/null \
        | sed -n 's/^ *Install location: *//p' | head -n 1
}

# The sonames the bundle cannot resolve in its own linker view, sorted, one line.
shadowed() {
    b="$1/minibrowser-wpe"
    lp="$b:$b/bin:$b/lib:$b/sys/lib"
    for d in "$b" "$b/bin" "$b/lib" "$b/sys/lib"; do
        [ -d "$d" ] || continue
        for f in "$d"/*; do
            [ -f "$f" ] || continue
            case "${f##*/}" in *.so|*.so.*) ;; *) [ -x "$f" ] || continue ;; esac
            # past its bound (124) the walk ends: the next ldd would wait the same
            bounded env LD_LIBRARY_PATH="$lp" ldd "$f" 2>/dev/null || [ $? -ne 124 ] || exit 124
        done
    done | sed -n 's/^[[:space:]]*\([^[:space:]]*\) => not found$/\1/p' | sort -u \
        | tr '\n' ' ' | sed 's/ $//'
}

# Returns when the bundle resolves; check: a shadowed bundle is exit 1;
# install: set it aside, install it again from the pin, and it must resolve then.
whole_bundle() {
    dir=$(bundle_dir)
    [ -n "$dir" ] || return 0
    if [ ! -d "$dir" ]; then
        echo "lane_browser_deps: no webkit bundle at $dir yet, nothing of it to check" >&2
        return 0
    fi
    command -v ldd >/dev/null 2>&1 || die 2 "no ldd on the lane: the bundle at $dir cannot be checked"
    libs=$(shadowed "$dir")
    [ -n "$libs" ] || return 0
    k=$(printf '%s' "$libs" | wc -w | tr -d ' ')
    if [ "$mode" = check ]; then
        echo "browser-deps shadowed webkit $k: $libs"
        exit 1
    fi
    aside="$HOME/.cache/meclaw-shadowed-bundles/${dir##*/}-$(date +%Y%m%d%H%M%S)"
    if ! mkdir -p "${aside%/*}" || ! mv "$dir" "$aside"; then
        die 1 "cannot set $dir aside to $aside"
    fi
    "$node_bin" "$cli" install webkit </dev/null >&2 \
        || die 1 "the pinned playwright $pin could not install webkit again (the old bundle is at $aside)"
    echo "browser-deps reinstalled webkit $k: $libs"
    still=$(shadowed "$dir")
    [ -z "$still" ] || die 1 "webkit still shadowed after a fresh install of playwright $pin: $still"
}

# `as_root` for an install, or exit 3 with the reason.
root_cmd() {
    if [ "$(id -u)" = 0 ]; then
        as_root=""
    elif sudo -n true </dev/null >/dev/null 2>&1; then
        as_root="sudo -n"
    else
        die 3 "$1 and no passwordless sudo for $(id -un) on $(hostname 2>/dev/null): as root run $2"
    fi
}

# The chromium headless shell of the lane's browser cache the gh766 units start
# (`playwright_chromium`: the bytewise last), or nothing; `lane_fingerprint.sh`
# findBrowser picks the same one (tb-s10, re-check M-3).
browsers_dir() { echo "${PLAYWRIGHT_BROWSERS_PATH:-$HOME/.cache/ms-playwright}"; }
headless_shell() {
    for b in "$(browsers_dir)"/chromium_headless_shell-*/chrome-headless-shell-linux64/chrome-headless-shell; do
        [ -x "$b" ] && echo "$b"
    done | LC_ALL=C sort | tail -n 1
}
# 0 when the shell starts its own sandbox and answers.
sandbox_runs() {
    timeout 30 "$1" --headless --dump-dom about:blank </dev/null >/dev/null 2>&1
}

# Returns when chromium is not asked for, has no shell yet, or its sandbox runs;
# check: a shell without a sandbox is exit 1; install: the AppArmor profile, then
# the shell must start.
chromium_sandbox() {
    case " $engines " in *" chromium "*) ;; *) return 0 ;; esac
    bin=$(headless_shell)
    if [ -z "$bin" ]; then
        echo "lane_browser_deps: no chromium headless shell under $(browsers_dir) yet, nothing of it to check" >&2
        return 0
    fi
    sandbox_runs "$bin" && return 0
    if [ "$mode" = check ]; then
        echo "browser-deps sandbox chromium: $bin cannot start its own sandbox (no user namespace under AppArmor) -- scripts/strand.sh lanes browsers <lane> install chromium"
        exit 1
    fi
    dir="${MECLAW_APPARMOR_DIR:-/etc/apparmor.d}"
    prof="$dir/meclaw-playwright-chromium"
    glob="$(browsers_dir)/chromium_headless_shell-*/chrome-headless-shell-linux64/chrome-headless-shell"
    root_cmd "chromium has no usable sandbox" "an AppArmor profile with userns for $glob"
    printf '%s\n' "# meclaw lane_browser_deps.sh (GH #1189): the playwright headless shell" \
        "# starts its own sandbox; like Ubuntu's chrome profile, unconfined with userns." \
        "abi <abi/4.0>," "include <tunables/global>" \
        "profile meclaw-playwright-chromium $glob flags=(unconfined) {" "  userns," "}" \
        | $as_root tee "$prof" >/dev/null || die 1 "cannot write $prof"
    $as_root apparmor_parser -r "$prof" </dev/null >&2 || die 1 "apparmor_parser could not load $prof"
    sandbox_runs "$bin" || die 1 "chromium still has no usable sandbox after loading $prof"
    echo "browser-deps sandboxed chromium: AppArmor profile $prof (userns) for $glob"
}

list=$(missing) || die 2 "no package list from the pinned playwright (see above)"
n=${list%% *}
pkgs=${list#* }
if [ "$n" = 0 ]; then
    whole_bundle
    chromium_sandbox
    echo "browser-deps ok $engines: every system package of playwright $pin is installed"
    exit 0
fi
if [ "$mode" = check ]; then
    echo "browser-deps missing $engines $n: $pkgs"
    exit 1
fi

root_cmd "$n packages missing for $engines ($pkgs)" "$node_bin $cli install-deps $engines"
# shellcheck disable=SC2086
$as_root env DEBIAN_FRONTEND=noninteractive "$node_bin" "$cli" install-deps $engines </dev/null >&2 \
    || die 1 "install-deps $engines failed (playwright $pin, the apt output is above)"
echo "browser-deps installed $engines $n: $pkgs"
after=$(missing) || die 2 "no package list after install-deps (see above)"
if [ "${after%% *}" != 0 ]; then
    die 1 "still missing after install-deps $engines: ${after#* }"
fi
whole_bundle
chromium_sandbox
echo "browser-deps ok $engines: every system package of playwright $pin is installed"
true
}; exit $?
