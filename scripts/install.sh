#!/bin/sh
# meclaw installer.
#
#   curl -fsSL https://meclaw.ai/install.sh | sh
#
# What it does: picks the release asset for this machine, downloads it next to
# its published SHA-256 sum, verifies the sum, and moves the binary into a
# directory on your PATH. Nothing else. It writes exactly one file, it never
# edits your shell profile, and running it twice is the same as running it once
# (the second run overwrites the binary in place).
#
# Deliberately POSIX sh, not bash: the one-liner above runs under whatever
# /bin/sh the host has, and on Debian/Ubuntu that is dash. `set -euo pipefail`
# is a bashism -- `set -eu` plus explicit status checks is the portable
# equivalent and is what this script uses.
#
# Deliberately no second pipe into a shell: this script downloads data and
# verifies it, it never fetches code and executes it.
#
# Knobs (all optional, all environment variables):
#   MECLAW_VERSION      install this version instead of the latest (e.g. 0.9.0)
#   MECLAW_INSTALL_DIR  install here instead of ~/.local/bin
#   MECLAW_REPO         owner/name of the GitHub repository

set -eu

REPO="${MECLAW_REPO:-mmeyerlein/meclaw}"
INSTALL_DIR="${MECLAW_INSTALL_DIR:-$HOME/.local/bin}"
BIN_NAME="meclaw"

say() { printf '%s\n' "$*"; }
err() { printf 'error: %s\n' "$*" >&2; }

die() {
    err "$*"
    exit 1
}

need() {
    command -v "$1" > /dev/null 2>&1 || die "this installer needs '$1', which is not on your PATH."
}

# ---------------------------------------------------------------------------
# Platform. Version 1 ships one target. Everything else gets a real answer
# instead of a broken download: build from source, it is one command.
#
# This deliberately does NOT run inside a command substitution. A `$(...)`
# would capture the friendly multi-line explanation below into a variable
# instead of showing it, and the reader would be left with the one-line error
# and no way forward -- which is the exact case this branch exists for.
# ---------------------------------------------------------------------------

detect_target() {
    os="$(uname -s)"
    arch="$(uname -m)"

    case "$os/$arch" in
        Linux/x86_64 | Linux/amd64)
            target="x86_64-unknown-linux-musl"
            ;;
        *)
            err "no prebuilt binary for $os/$arch yet (today: Linux x86_64 only)."
            say ""
            say "Build it from source instead -- you need a Rust toolchain (https://rustup.rs):"
            say ""
            say "    cargo install --git https://github.com/${REPO} meclaw-cli"
            say ""
            say "If you want a prebuilt binary for $os/$arch, say so in an issue:"
            say "    https://github.com/${REPO}/issues"
            exit 1
            ;;
    esac
}

# ---------------------------------------------------------------------------
# Downloading. curl or wget, whichever the host has.
# ---------------------------------------------------------------------------

fetch_to() {
    # fetch_to <url> <destination-path>
    if command -v curl > /dev/null 2>&1; then
        curl -fsSL "$1" -o "$2"
    elif command -v wget > /dev/null 2>&1; then
        wget -qO "$2" "$1"
    else
        die "this installer needs 'curl' or 'wget', and found neither."
    fi
}

fetch_stdout() {
    if command -v curl > /dev/null 2>&1; then
        curl -fsSL "$1"
    elif command -v wget > /dev/null 2>&1; then
        wget -qO- "$1"
    else
        die "this installer needs 'curl' or 'wget', and found neither."
    fi
}

# Two ways to ask "what is the latest release", tried in this order.
#
# First: the /releases/latest page redirects to /releases/tag/<tag>, so the tag
# is readable straight off the redirect target. This costs no API quota, and
# quota is the reason it goes first -- api.github.com allows 60 unauthenticated
# requests per hour PER IP, which is a shared budget behind any NAT, any CI
# runner and any office. An installer that fails with "rate limit exceeded"
# through no fault of the person running it is a bad first impression.
#
# Second, if that yields nothing: the API, as before.
#
# Both strip an optional leading `v`, because the tag is `v0.9.0` while the
# asset is named `meclaw-0.9.0-<target>.tar.gz`.
latest_version_via_redirect() {
    if command -v curl > /dev/null 2>&1; then
        curl -fsSL -o /dev/null -w '%{url_effective}\n' \
            "https://github.com/${REPO}/releases/latest" 2> /dev/null \
            | sed -n 's|.*/releases/tag/v\{0,1\}\([^/]*\)$|\1|p' \
            | head -n 1
    elif command -v wget > /dev/null 2>&1; then
        wget -q -S --spider "https://github.com/${REPO}/releases/latest" 2>&1 \
            | sed -n \
                's|.*[Ll]ocation:[[:space:]]*.*/releases/tag/v\{0,1\}\([^[:space:]]*\).*|\1|p' \
            | head -n 1
    fi
}

# The GitHub API answer is JSON, and jq is not a fair thing to require from an
# install one-liner. The tag_name field is the only thing read out of it, and
# it is read with sed rather than parsed -- if that ever breaks, the version
# comes out empty and the script stops right below instead of guessing.
latest_version_via_api() {
    fetch_stdout "https://api.github.com/repos/${REPO}/releases/latest" 2> /dev/null \
        | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"v\{0,1\}\([^"]*\)".*/\1/p' \
        | head -n 1
}

latest_version() {
    found="$(latest_version_via_redirect)"
    if [ -z "$found" ]; then
        found="$(latest_version_via_api)"
    fi
    printf '%s\n' "$found"
}

# ---------------------------------------------------------------------------
# Checksum verification. Not optional: the tarball is only unpacked after its
# published sum matches.
# ---------------------------------------------------------------------------

verify_sha256() {
    # verify_sha256 <directory> <checksum-file-name>
    if command -v sha256sum > /dev/null 2>&1; then
        (cd "$1" && sha256sum -c "$2" > /dev/null 2>&1)
    elif command -v shasum > /dev/null 2>&1; then
        (cd "$1" && shasum -a 256 -c "$2" > /dev/null 2>&1)
    else
        die "this installer needs 'sha256sum' or 'shasum' to verify the download."
    fi
}

# ---------------------------------------------------------------------------
# Main.
# ---------------------------------------------------------------------------

need tar
need uname

detect_target

# The install directory is checked BEFORE anything is downloaded. A 25 MB
# download followed by "that directory is not writable" is a worse answer than
# the same sentence four seconds earlier, and it is the same sentence.
if ! mkdir -p "$INSTALL_DIR" 2> /dev/null; then
    die "cannot create ${INSTALL_DIR}.
Either pick a writable directory:  MECLAW_INSTALL_DIR=\"\$HOME/bin\" sh install.sh
or install system-wide with sudo:  sudo MECLAW_INSTALL_DIR=/usr/local/bin sh install.sh"
fi
if [ ! -w "$INSTALL_DIR" ]; then
    die "${INSTALL_DIR} is not writable by $(id -un).
Either pick a writable directory:  MECLAW_INSTALL_DIR=\"\$HOME/.local/bin\" sh install.sh
or install system-wide with sudo:  sudo MECLAW_INSTALL_DIR=${INSTALL_DIR} sh install.sh"
fi

version="${MECLAW_VERSION:-}"
if [ -z "$version" ]; then
    say "Looking up the latest meclaw release..."
    version="$(latest_version)"
fi
# The release is TAGGED `v0.9.0` and the asset inside it is named
# `meclaw-0.9.0-<target>.tar.gz`. People copy the tag, because the tag is what
# the releases page shows -- so `MECLAW_VERSION=v0.9.0` has to mean the same
# thing as `MECLAW_VERSION=0.9.0`, instead of building `meclaw-v0.9.0-...` and
# dying on a 404 that names a file nobody ever published. One leading `v` is
# stripped here, exactly as the two lookup paths already strip it.
version="${version#v}"
[ -n "$version" ] || die "could not determine the latest version. Set MECLAW_VERSION=x.y.z and retry."

asset="${BIN_NAME}-${version}-${target}.tar.gz"
base_url="https://github.com/${REPO}/releases/download/v${version}"

tmp="$(mktemp -d)"
# The trap is set before the first download, so an interrupted run leaves no
# half-downloaded tarball behind.
trap 'rm -rf "$tmp"' EXIT INT TERM

say "Downloading meclaw ${version} (${target})..."
fetch_to "${base_url}/${asset}" "${tmp}/${asset}" \
    || die "download failed: ${base_url}/${asset}
Check that version ${version} exists: https://github.com/${REPO}/releases"
fetch_to "${base_url}/${asset}.sha256" "${tmp}/${asset}.sha256" \
    || die "could not download the checksum for ${asset}. Refusing to install unverified."

say "Verifying checksum..."
verify_sha256 "$tmp" "${asset}.sha256" || die "checksum mismatch for ${asset}. Refusing to install."

tar -xzf "${tmp}/${asset}" -C "$tmp"
unpacked="${tmp}/${BIN_NAME}-${version}-${target}/${BIN_NAME}"
[ -f "$unpacked" ] || die "the archive did not contain ${BIN_NAME} where it was expected."
chmod +x "$unpacked"

# mv within one filesystem is atomic, so a running meclaw is never handed a
# half-written file. The copy into $INSTALL_DIR first (rather than mv from
# $tmp, which is usually a different filesystem) is what keeps it atomic.
cp "$unpacked" "${INSTALL_DIR}/.${BIN_NAME}.new"
chmod 755 "${INSTALL_DIR}/.${BIN_NAME}.new"
mv -f "${INSTALL_DIR}/.${BIN_NAME}.new" "${INSTALL_DIR}/${BIN_NAME}"

say "Installed ${INSTALL_DIR}/${BIN_NAME}"

# PATH. Checked with a case match on the padded PATH, so /usr/local/binx never
# counts as a match for /usr/local/bin.
case ":${PATH}:" in
    *":${INSTALL_DIR}:"*)
        say ""
        "${INSTALL_DIR}/${BIN_NAME}" --version
        say ""
        say "Next: https://github.com/${REPO}#quickstart"
        ;;
    *)
        say ""
        "${INSTALL_DIR}/${BIN_NAME}" --version
        say ""
        say "${INSTALL_DIR} is not on your PATH. Add it:"
        say ""
        say "    export PATH=\"${INSTALL_DIR}:\$PATH\""
        say ""
        say "Put that line in ~/.bashrc or ~/.zshrc to make it stick, then:"
        say ""
        say "    meclaw --version"
        ;;
esac
