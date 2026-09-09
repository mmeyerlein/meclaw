#!/bin/sh
# meclaw start script: install, boot, grow, ask -- one line.
#
#   curl -fsSL https://github.com/mmeyerlein/meclaw/releases/latest/download/start.sh | sh
#
# What it does, in order, and it stops at the first step that fails and says
# which one:
#
#   1. runs install.sh from the same release (one static binary into
#      ~/.local/bin; the installer verifies the published SHA-256 sum)
#   2. fetches the templates and examples that match that binary's version
#   3. picks a free port and starts a colony daemon in the background
#   4. grows one of three example colonies and uses it:
#      hard-shell (no key needed) -- three cells, no model. It sends the
#      cloud-metadata fetch that a prompt-injected agent would send and prints
#      the refusal out of the colony's own trace.
#      meclaw-os (needs a key) -- seventeen cells, the shipped assistant. It
#      sends one turn with `meclaw ask` and prints the answer.
#      organism (needs a key) -- the meclaw-os shell, thirty-five cells, plus a
#      front door and a terminal. Nothing answers yet: an organisation, a member
#      and an agent are the three declarations docs/getting-started.md walks
#      through, and the run prints them.
#   5. prints what is running, the API URL, the UI URL, how to stop it, and
#      where the colony lives
#
# The key comes from the environment, or, when it is not there and a terminal
# is, from a question. The answer is not echoed while it is typed. It is never
# printed and it is written to exactly one file: the `.env` of the colony this
# run creates, mode 0600. With no key and no terminal to ask on -- a pipeline,
# a CI job, a cron line -- the run says how to set it and boots the keyless
# colony instead.
#
# POSIX sh, like install.sh: the one-liner runs under whatever /bin/sh the host
# has. Needs curl or wget, tar, and sha256sum or shasum; the two example
# colonies also need python3 on the host, because their door and their
# terminal are `code` cells.
#
# Knobs (all optional, all environment variables):
#   OPENROUTER_API_KEY   the provider key; with it the run grows the assistant
#                        and asks no question
#   MECLAW_EXAMPLE       which colony to grow: hard-shell, meclaw-os or
#                        organism (default meclaw-os with a key, hard-shell
#                        without)
#   MECLAW_MODEL         the model the assistant answers with
#                        (default openai/gpt-5.6-luna, any OpenRouter slug)
#   MECLAW_VERSION       install this version instead of the latest (e.g. 0.33.0)
#   MECLAW_INSTALL_DIR   binary goes here instead of ~/.local/bin
#   MECLAW_HOME          templates, examples and colonies go here
#                        (default ~/.local/share/meclaw)
#   MECLAW_PORT          first port to try (default 7777; the next free one
#                        above it is taken if it is busy)
#   MECLAW_REPO          owner/name of the GitHub repository
#   MECLAW_TEMPLATES_URL a tar.gz to take templates/ and examples/ from,
#                        instead of the release asset (mirrors, offline hosts)

set -eu

REPO="${MECLAW_REPO:-mmeyerlein/meclaw}"
INSTALL_DIR="${MECLAW_INSTALL_DIR:-$HOME/.local/bin}"
MECLAW_HOME="${MECLAW_HOME:-$HOME/.local/share/meclaw}"
PORT_START="${MECLAW_PORT:-7777}"
MODEL="${MECLAW_MODEL:-openai/gpt-5.6-luna}"
KEY="${OPENROUTER_API_KEY:-}"
EXAMPLE="${MECLAW_EXAMPLE:-}"

step=0
step_name=""

say() { printf '%s\n' "$*"; }
err() { printf 'error: %s\n' "$*" >&2; }

die() {
    if [ -n "$step_name" ]; then
        err "step ${step} (${step_name}) failed: $*"
    else
        err "$*"
    fi
    exit 1
}

begin() {
    step=$((step + 1))
    step_name="$1"
    say ""
    say "[${step}/5] $2"
}

need() {
    command -v "$1" > /dev/null 2>&1 || die "this script needs '$1', which is not on your PATH."
}

# ---------------------------------------------------------------------------
# HTTP. curl or wget, whichever the host has. Every call is one function so
# the rest of the script never spells the tool out.
# ---------------------------------------------------------------------------

if command -v curl > /dev/null 2>&1; then
    HTTP=curl
elif command -v wget > /dev/null 2>&1; then
    HTTP=wget
else
    die "this script needs 'curl' or 'wget', and found neither."
fi

fetch_to() {
    # fetch_to <url> <destination-path>
    if [ "$HTTP" = curl ]; then
        curl -fsSL "$1" -o "$2"
    else
        wget -qO "$2" "$1"
    fi
}

http_get() {
    # http_get <url>   -> body on stdout
    if [ "$HTTP" = curl ]; then
        curl -fsS "$1"
    else
        wget -qO- "$1"
    fi
}

http_post_file() {
    # http_post_file <url> <json-file>   -> body on stdout
    if [ "$HTTP" = curl ]; then
        curl -fsS -X POST -H 'Content-Type: application/json' --data-binary "@$2" "$1"
    else
        wget -qO- --header='Content-Type: application/json' --post-file="$2" "$1"
    fi
}

http_post_data() {
    # http_post_data <url> <json-string>   -> body on stdout
    if [ "$HTTP" = curl ]; then
        curl -fsS -X POST -H 'Content-Type: application/json' --data-binary "$2" "$1"
    else
        wget -qO- --header='Content-Type: application/json' --post-data="$2" "$1"
    fi
}

# A port is free when nothing answers a TCP connect on it. curl reports a
# refused connection as exit 7, wget as exit 4; anything else means somebody
# is listening there.
port_is_free() {
    if [ "$HTTP" = curl ]; then
        curl -s -o /dev/null --max-time 2 "http://127.0.0.1:$1/" > /dev/null 2>&1
        [ $? -eq 7 ]
    else
        wget -q -O /dev/null --timeout=2 --tries=1 "http://127.0.0.1:$1/" > /dev/null 2>&1
        [ $? -eq 4 ]
    fi
}

verify_sha256() {
    # verify_sha256 <directory> <checksum-file-name>
    if command -v sha256sum > /dev/null 2>&1; then
        (cd "$1" && sha256sum -c "$2" > /dev/null 2>&1)
    elif command -v shasum > /dev/null 2>&1; then
        (cd "$1" && shasum -a 256 -c "$2" > /dev/null 2>&1)
    else
        die "this script needs 'sha256sum' or 'shasum' to verify a download."
    fi
}

need tar
need python3

tmp="$(mktemp -d)"
STTY_SAVED=""

restore_tty() {
    # Echo goes back on however the run ends, including Ctrl-C at the prompt:
    # a terminal left mute after an interrupted script is a bug people fix by
    # closing the window.
    if [ -n "$STTY_SAVED" ]; then
        stty "$STTY_SAVED" < /dev/tty 2> /dev/null || true
        STTY_SAVED=""
    fi
}

trap 'restore_tty; rm -rf "$tmp"' EXIT INT TERM

# ---------------------------------------------------------------------------
# The key. Without it the run boots the keyless colony, so it is asked for
# before anything is installed rather than after.
#
# The question goes to /dev/tty and is read from /dev/tty, never from stdin:
# in the one-liner stdin is the pipe the script itself arrives on. Opening
# /dev/tty is also the test -- it fails with no controlling terminal, which is
# exactly the case that must not stop to ask. The open is tried in a SUBSHELL,
# because a failed redirection on a compound command ends a non-interactive
# POSIX shell outright (dash does, and dash is /bin/sh on Debian).
# ---------------------------------------------------------------------------

if [ -z "$KEY" ] && ( : < /dev/tty ) 2> /dev/null; then
    printf '\n' > /dev/tty
    printf 'meclaw grows its assistant with an OpenRouter key (https://openrouter.ai/keys).\n' > /dev/tty
    printf 'Paste one, or press Enter for the keyless colony.\n' > /dev/tty
    printf 'OPENROUTER_API_KEY: ' > /dev/tty

    if command -v stty > /dev/null 2>&1; then
        STTY_SAVED="$(stty -g < /dev/tty 2> /dev/null || true)"
        if [ -n "$STTY_SAVED" ]; then
            stty -echo < /dev/tty 2> /dev/null || STTY_SAVED=""
        fi
    fi

    KEY=""
    IFS= read -r KEY < /dev/tty || KEY=""

    restore_tty
    printf '\n' > /dev/tty
elif [ -z "$KEY" ]; then
    say "OPENROUTER_API_KEY is not set and there is no terminal to ask on. Set it in front"
    say "of the pipe's sh:  curl -fsSL .../start.sh | OPENROUTER_API_KEY=sk-... sh"
fi

# ---------------------------------------------------------------------------
# 1. Install. The installer is the same install.sh that the release ships; it
#    is run, not re-implemented. A checkout that carries scripts/install.sh
#    next to this file uses that copy; the one-liner fetches it from the same
#    release it takes everything else from.
# ---------------------------------------------------------------------------

begin install "Installing meclaw"

if [ -n "${MECLAW_VERSION:-}" ]; then
    release_url="https://github.com/${REPO}/releases/download/v${MECLAW_VERSION#v}"
else
    release_url="https://github.com/${REPO}/releases/latest/download"
fi

installer=""
case "$0" in
    */*)
        if [ -f "$(dirname "$0")/install.sh" ]; then
            installer="$(dirname "$0")/install.sh"
        fi
        ;;
esac
if [ -z "$installer" ]; then
    fetch_to "${release_url}/install.sh" "${tmp}/install.sh" \
        || die "could not download ${release_url}/install.sh"
    installer="${tmp}/install.sh"
fi
sh "$installer" || die "the installer did not finish (its message is above)."

MECLAW="${INSTALL_DIR}/meclaw"
[ -x "$MECLAW" ] || die "expected the binary at ${MECLAW} after the install, and it is not there."
version="$("$MECLAW" --version | sed 's/.* //')"
[ -n "$version" ] || die "${MECLAW} --version printed nothing usable."

# ---------------------------------------------------------------------------
# 2. Templates and examples, matching the binary. A colony is grown from
#    templates, and a template library from another version is the classic
#    "it worked on the README" failure -- so the version comes from the binary
#    that was just installed, never from a guess.
#
#    Source, in order: the release asset meclaw-<version>-templates.tar.gz with
#    its SHA-256 sum (2 MB, only the two directories, verified); if the release
#    has no such asset (releases that predate it), the tag tarball GitHub serves
#    for every tag (8 MB, the whole tree, no published sum -- the transport is
#    TLS to github.com). No git needed for either.
# ---------------------------------------------------------------------------

begin templates "Fetching templates and examples for ${version}"

lib="${MECLAW_HOME}/${version}"
TEMPLATES="${lib}/templates"
EXAMPLES="${lib}/examples"

if [ -d "$TEMPLATES" ] && [ -d "$EXAMPLES" ]; then
    say "already in ${lib}"
else
    mkdir -p "${tmp}/lib"
    got=""
    if [ -n "${MECLAW_TEMPLATES_URL:-}" ]; then
        say "from ${MECLAW_TEMPLATES_URL}"
        fetch_to "$MECLAW_TEMPLATES_URL" "${tmp}/lib/templates.tar.gz" \
            || die "could not download ${MECLAW_TEMPLATES_URL}"
        got=custom
    else
        asset="meclaw-${version}-templates.tar.gz"
        asset_url="https://github.com/${REPO}/releases/download/v${version}/${asset}"
        if fetch_to "$asset_url" "${tmp}/lib/templates.tar.gz" 2> /dev/null; then
            fetch_to "${asset_url}.sha256" "${tmp}/lib/${asset}.sha256" \
                || die "could not download the checksum for ${asset}. Refusing to unpack unverified."
            mv "${tmp}/lib/templates.tar.gz" "${tmp}/lib/${asset}"
            verify_sha256 "${tmp}/lib" "${asset}.sha256" \
                || die "checksum mismatch for ${asset}. Refusing to unpack."
            mv "${tmp}/lib/${asset}" "${tmp}/lib/templates.tar.gz"
            got=asset
            say "from the release asset ${asset} (checksum verified)"
        else
            tag_url="https://github.com/${REPO}/archive/refs/tags/v${version}.tar.gz"
            fetch_to "$tag_url" "${tmp}/lib/templates.tar.gz" \
                || die "neither ${asset_url} nor ${tag_url} could be downloaded."
            got=tag
            say "from the tag tarball v${version} (no templates asset on that release)"
        fi
    fi

    tar -xzf "${tmp}/lib/templates.tar.gz" -C "${tmp}/lib" \
        || die "could not unpack the ${got} tarball."
    # Both shapes carry the two directories one level down:
    # meclaw-<version>-templates/{templates,examples} or
    # meclaw-<version>/{templates,examples}.
    found=""
    for d in "${tmp}/lib"/*/; do
        if [ -d "${d}templates" ] && [ -d "${d}examples" ]; then
            found="$d"
            break
        fi
    done
    [ -n "$found" ] || die "the tarball did not contain templates/ and examples/ side by side."
    mkdir -p "$lib"
    rm -rf "${lib}/templates.new" "${lib}/examples.new"
    mv "${found}templates" "${lib}/templates.new"
    mv "${found}examples" "${lib}/examples.new"
    rm -rf "$TEMPLATES" "$EXAMPLES"
    mv "${lib}/templates.new" "$TEMPLATES"
    mv "${lib}/examples.new" "$EXAMPLES"
    say "into ${lib}"
fi

# ---------------------------------------------------------------------------
# 3. Port and daemon. Every run gets a fresh colony directory, because a
#    colony that was booted once carries its database, and one root takes one
#    daemon at a time (the daemon refuses a second, by design).
# ---------------------------------------------------------------------------

# Which colony. The key decides it unless MECLAW_EXAMPLE says otherwise, and
# two of the three have a model in them, so they need one.
if [ -n "$EXAMPLE" ]; then
    flavour="$EXAMPLE"
elif [ -n "$KEY" ]; then
    flavour=meclaw-os
else
    flavour=hard-shell
fi

case "$flavour" in
    hard-shell) seed=seed ;;
    meclaw-os)  seed=seed ;;
    # The organism starts from the seed that declares the shell: its root tree
    # carries a `cell.type: "ref"` marker naming meclaw-os, and the first boot
    # grows it. The three declarations of docs/getting-started.md are then the
    # levels below it, which is the order that page teaches.
    organism)   seed="seed-ref" ;;
    *) die "MECLAW_EXAMPLE=${flavour} is not one of hard-shell, meclaw-os, organism." ;;
esac

if [ -z "$KEY" ] && [ "$flavour" != hard-shell ]; then
    die "examples/${flavour} runs on a model, so it needs a provider key. Set OPENROUTER_API_KEY, or leave MECLAW_EXAMPLE unset for the keyless colony."
fi

begin boot "Starting a colony (${flavour})"

port="$PORT_START"
tries=0
while ! port_is_free "$port"; do
    tries=$((tries + 1))
    [ "$tries" -lt 50 ] || die "no free port between ${PORT_START} and $((PORT_START + 49)). Set MECLAW_PORT."
    port=$((port + 1))
done
[ "$port" = "$PORT_START" ] || say "port ${PORT_START} is busy, using ${port}"

stamp="$(date +%Y%m%d-%H%M%S)"
root="${MECLAW_HOME}/colonies/${flavour}-${stamp}"
mkdir -p "${MECLAW_HOME}/colonies"
cp -R "${EXAMPLES}/${flavour}/${seed}" "$root" || die "could not copy the ${flavour} ${seed} to ${root}."

if [ -n "$KEY" ]; then
    # The one file the key goes into. 0600 before the first byte is written.
    #
    # Which model names a declaration reads is the declaration's business, so
    # the file carries every token the shipped ones ask for and gives them all
    # the same model. MODEL_BRAIN is what examples/meclaw-os reads; the six
    # MODEL_* below are what the four levels of examples/organism read
    # (assistant@2.6.0 takes the first three as ctx, memory-hive@3.3.0 the
    # last three), and a token with no value is refused as `env_var_missing`
    # rather than committing a half-wired cell.
    (
        umask 077
        printf 'OPENROUTER_API_KEY=%s\n' "$KEY" > "${root}/.env"
        for token in MODEL_BRAIN MODEL_CORE MODEL_CORE_FAST MODEL_SURFACE \
                     MODEL_CLOSER MODEL_DIALECTIC MODEL_DREAMER; do
            printf '%s=%s\n' "$token" "$MODEL" >> "${root}/.env"
        done
    ) || die "could not write ${root}/.env"
fi

api="127.0.0.1:${port}"
"$MECLAW" --root "$root" --templates "$TEMPLATES" --daemon --api "$api" \
    > "${root}/daemon.out" 2>&1 &
pid=$!
printf '%s\n' "$pid" > "${root}/daemon.pid"

# The very first start reads a 30 MB binary from cold disk and can stay silent
# for ~40 s; every later start answers well under a second. 120 s is the
# budget, and a daemon that dies inside it is reported with its last lines.
say "waiting for http://${api}/health (the first start can take ~40 s)"
waited=0
until http_get "http://${api}/health" > /dev/null 2>&1; do
    if ! kill -0 "$pid" 2> /dev/null; then
        err "the daemon exited before it answered. Last lines of ${root}/daemon.out:"
        tail -n 20 "${root}/daemon.out" >&2
        die "daemon did not come up."
    fi
    [ "$waited" -lt 120 ] || die "no answer on http://${api}/health after 120 s. The daemon (pid ${pid}) is still running; its log is ${root}/daemon.out."
    sleep 1
    waited=$((waited + 1))
done
say "up after ${waited} s, pid ${pid}"

# ---------------------------------------------------------------------------
# 4. Grow, then use it.
# ---------------------------------------------------------------------------

grow() {
    # grow <declaration.json>
    receipt="$(http_post_file "http://${api}/colony/mutations" "$1")" \
        || die "POST /colony/mutations did not answer."
    case "$receipt" in
        *'"outcome":"committed"'*) ;;
        *) die "the colony did not commit the declaration: ${receipt}" ;;
    esac
}

if [ "$flavour" = hard-shell ]; then
    begin grow "Growing examples/hard-shell and sending it the cloud-metadata fetch"
    say "This colony is keyless: three cells, no model."
    if [ -z "$KEY" ]; then
        say "To grow the assistant instead:  OPENROUTER_API_KEY=sk-... sh start.sh"
    fi
    say ""
    grow "${EXAMPLES}/hard-shell/grow.json"
    say "grown: /door, /probe, /sink"

    # The first address a prompt-injected agent is told to fetch: where AWS,
    # GCP and Azure hand out instance credentials over plain HTTP.
    probe='{"target": "/door", "body": {"messages": [{"origin": "assistant", "type": "tool_call", "id": "c1", "text": "{\"url\": \"http://169.254.169.254/latest/meta-data/iam/security-credentials/\"}"}]}}'
    posted="$(http_post_data "http://${api}/messages" "$probe")" \
        || die "POST /messages did not answer."
    trace_id="$(printf '%s' "$posted" | sed -n 's/.*"message_id":"\([^"]*\)".*/\1/p')"
    [ -n "$trace_id" ] || die "POST /messages answered without a message_id: ${posted}"

    # The refusal is a hop on the colony's own record, so it is read from the
    # trace of that message, not from the POST's answer.
    trace=""
    waited=0
    while :; do
        trace="$(http_get "http://${api}/colony/trace?trace_id=${trace_id}" 2> /dev/null || true)"
        case "$trace" in
            *error_code*) break ;;
        esac
        [ "$waited" -lt 30 ] || die "the trace of ${trace_id} shows no verdict after 30 s: ${trace}"
        sleep 1
        waited=$((waited + 1))
    done
    code="$(printf '%s' "$trace" | sed -n 's/.*\\"error_code\\":\\"\([^\\]*\)\\".*/\1/p' | head -n 1)"
    route="$(printf '%s' "$trace" | sed -n 's/.*\\"error_code\\".*\\"route\\":\\"\([^\\]*\)\\".*/\1/p' | head -n 1)"
    text="$(printf '%s' "$trace" | sed -n 's/.*\\"origin\\":\\"tool\\",\\"text\\":\\"\([^\\]*\)\\".*/\1/p' | head -n 1)"
    say ""
    say "sent:    GET http://169.254.169.254/latest/meta-data/iam/security-credentials/  (through /door)"
    say "verdict: route=${route:-?}  error_code=${code:-?}"
    say "message: ${text:-$trace}"
    say "trace:   http://${api}/colony/trace?trace_id=${trace_id}"
    say ""
    say "The fetch never left the machine: no http_status on that hop, duration 0 ms, and"
    say "nothing in seed/main/probe/config.json says so. Details: examples/hard-shell/WALKTHROUGH.md"
elif [ "$flavour" = organism ]; then
    begin grow "Opening the front door of the shell"
    # The boot grew the shell out of the seed's ref marker. What it has no way
    # to grow is an EDGE, and an inbound turn needs two: a `door` cell to put
    # the turn on a named lane, and one edge that carries it into the shell.
    # The same declaration ends the lanes that leave the shell in a `terminal`,
    # so an answer stops in the trace instead of the dead-letter queue.
    grow "${EXAMPLES}/organism/grow-door.json"
    say "grown: /door, /sink (the shell itself came up with the colony)"
    say ""
    say "Nothing answers yet -- an organisation, a member and an agent are three"
    say "declarations, and docs/getting-started.md walks through them:"
    say ""
    # Not `step`: that name belongs to begin(), which counts with it.
    for decl in org member assistant; do
        say "  curl -s -X POST http://${api}/colony/mutations \\"
        say "       -H 'Content-Type: application/json' \\"
        say "       -d @${EXAMPLES}/organism/grow-${decl}.json"
    done
else
    begin grow "Growing examples/meclaw-os and asking it one question"
    grow "${EXAMPLES}/meclaw-os/grow.json"
    say "grown: /door, /firewall, /talky, /sink (model ${MODEL})"

    # The commit spawned every cell; give the registry a moment to list the
    # door awake before the first turn goes in.
    waited=0
    until http_get "http://${api}/colony/registry" 2> /dev/null | grep -q '"path":"/door"'; do
        [ "$waited" -lt 30 ] || die "/door did not appear in /colony/registry within 30 s."
        sleep 1
        waited=$((waited + 1))
    done

    question="Say hello in one short sentence."
    say ""
    say "> ${question}"
    if ! answer="$("$MECLAW" ask --api "$api" --target /door "$question")"; then
        say ""
        die "meclaw ask did not get an answer (its message is above). A wrong key ends in code=auth; the daemon is still running on http://${api}, pid ${pid}."
    fi
    say "${answer}"
fi

# ---------------------------------------------------------------------------
# 5. Where things are.
# ---------------------------------------------------------------------------

begin summary "Running"
say "colony:   ${root}"
say "daemon:   pid ${pid}, log ${root}/daemon.out"
say "API:      http://${api}/"
say "UI:       http://${api}/ui/"
say "stop it:  kill ${pid}"
if [ "$flavour" = hard-shell ]; then
    say "with key: OPENROUTER_API_KEY=sk-... sh start.sh   (grows the assistant; keys: https://openrouter.ai/keys)"
else
    say "talk to it: ${MECLAW} ask --api ${api} --target /door \"...\""
fi
say "by hand:  https://github.com/${REPO}/blob/main/docs/installation.md"
