# Installation, step by step

What the one-line start does at each step, every environment variable it reads, and
the same steps typed by hand.

Written for anyone who wants to install without the one-liner, mirror the download,
or find out which step failed. [Getting started](getting-started.md) is the guided
path; this page is the mechanics under it.

`install.sh` and `start.sh` live in [`scripts/`](../scripts/) and ship as assets with
every release. `https://meclaw.ai/install.sh` and `https://meclaw.ai/start.sh`
redirect to `releases/latest/download/<name>`.

## What you need

- Linux on x86_64. The binary is static (musl); the installer refuses every
  other platform and prints the `cargo install` line instead.
- `curl` or `wget`, `tar`, `sha256sum` or `shasum`. Nothing is unpacked before
  its published sum matches.
- `python3`, because the example colonies have `code` cells, and a `code` cell
  runs `python3` and nothing else.
- For the assistant: an OpenRouter key from <https://openrouter.ai/keys>, which
  the script asks for if it has a terminal. The keyless colony needs no key, no
  model and no account.

## Step 1: the binary

`start.sh` runs `install.sh` from the same release; there is one installer.

```bash
curl -fsSL https://github.com/mmeyerlein/meclaw/releases/latest/download/install.sh | sh
```

It follows the `releases/latest` redirect (no API quota), downloads the archive
and its `.sha256`, verifies, and moves the binary to `~/.local/bin/meclaw` in
one rename. It writes that one file and never touches a shell profile. If
`~/.local/bin` is not on your `PATH`, it prints the `export` line; `start.sh`
does not need it, `meclaw ask` afterwards does.

## Step 2: templates that match the binary

A colony is grown from templates, and they have to be the ones the binary was
released with. `start.sh` reads the version off the binary and fetches the
release asset `meclaw-<version>-templates.tar.gz` (`templates/` and
`examples/`, about 2 MB, sum verified). Releases without that asset fall back
to the tag tarball GitHub serves (about 8 MB, no sum). Neither needs `git`.
They land in `~/.local/share/meclaw/<version>/`, and a second run finds them
there.

By hand, the same asset:

```bash
v="$(meclaw --version | cut -d' ' -f2)"
base="https://github.com/mmeyerlein/meclaw/releases/download/v$v"
curl -fsSLO "$base/meclaw-$v-templates.tar.gz"
curl -fsSLO "$base/meclaw-$v-templates.tar.gz.sha256"
sha256sum -c "meclaw-$v-templates.tar.gz.sha256"
tar -xzf "meclaw-$v-templates.tar.gz"
```

That unpacks `meclaw-<version>-templates/templates/` and `.../examples/`. To
build from source instead, clone the repository:
[CONTRIBUTING.md](../CONTRIBUTING.md).

## Step 3: the key, or no key

`start.sh` takes the key from the environment. If it is not there and there is
a terminal to ask on, the run asks, before it installs anything; the answer is
read from `/dev/tty` and not echoed while it is typed. No key is not an error:
an empty answer, or no terminal to ask on, selects the keyless colony, and the
run says how to set the key for the other one.

```bash
curl -fsSL https://github.com/mmeyerlein/meclaw/releases/latest/download/start.sh \
    | OPENROUTER_API_KEY=sk-... sh
```

With a key, the script writes one file, the colony's `.env`, mode `0600`
before the first byte lands:

```
OPENROUTER_API_KEY=sk-...
MODEL_BRAIN=openai/gpt-5.6-luna
MODEL_CORE=openai/gpt-5.6-luna
...
```

The key is never printed. Which model token a declaration reads is the
declaration's business, so the file carries every one the shipped declarations
ask for and gives them all the same slug: `MODEL_BRAIN` for
`examples/meclaw-os`, and `MODEL_CORE`, `MODEL_CORE_FAST`, `MODEL_SURFACE`,
`MODEL_CLOSER`, `MODEL_DIALECTIC` and `MODEL_DREAMER` for the four levels of
`examples/organism`. Another model is one line and a restart, and any
OpenAI-compatible endpoint works, OpenRouter is only the default `base_url`. A
wrong key is not caught here: the colony grows, and the first turn ends in
`code=auth`, which the script reports as step 4.

## Step 4: the daemon

Every run copies the example's `seed/` to a fresh
`~/.local/share/meclaw/colonies/<flavour>-<date>-<time>`, because one root
takes one daemon at a time. The port is the first free one from `7777` up.

```bash
meclaw --root <colony> --templates <templates> --daemon --api 127.0.0.1:<port>
```

The pid goes to `<colony>/daemon.pid` and the tracing stream to two places:
`daemon.out`, which holds what the colony writes to stderr, and `log.jsonl`
beside it in JSON. The script polls `GET /health` until it answers.
**The first start can stay silent for about 40 seconds** while the 30 MB
binary comes off cold disk; every later start answers in under a second.
By hand, `--daemon` runs in the foreground and stops on Ctrl-C, so the next
steps go in a second shell.

## Step 5: grow it

One JSON file, posted once, nothing restarts:

```bash
curl -s -X POST 127.0.0.1:7777/colony/mutations \
     -H 'Content-Type: application/json' -d @examples/meclaw-os/grow.json
```

The answer is `{"mutation":{"id":"...","outcome":"committed"}}`; a rejection
names a code and changes nothing on disk. Without a key the file is
`examples/hard-shell/grow.json` (three cells); with a key it is
`examples/meclaw-os/grow.json` (seventeen cells, the shipped assistant). With
`MECLAW_EXAMPLE=organism` the root is `examples/organism/seed-ref`, whose
`cell.type: "ref"` marker grows the shell on the first boot, so the file posted
here is `examples/organism/grow-door.json` and the three levels below the shell
are [Getting started](getting-started.md). The READMEs under
[`examples/`](../examples/README.md) say what each colony is.

## Step 6: use it

With a key, one turn:

```bash
meclaw ask --api 127.0.0.1:7777 --target /door "Say hello in one short sentence."
```

`ask` posts to `/door` and reads the answer out of `GET /colony/trace`. It
exits `0` on an answer, `1` on an error route, `2` when `--timeout` (120 s)
runs out.

Without a key, meaning a run with no terminal to ask in, the script sends the
fetch a prompt-injected agent is told to send first, the cloud-metadata address,
and reads the verdict out of the trace: `route: denied`,
`error_code: target_blocked`, `duration_ms: 0`, no `http_status`, a refusal that
never opened a socket. The seed configures no security at all.
[`examples/hard-shell/WALKTHROUGH.md`](../examples/hard-shell/WALKTHROUGH.md)
takes it apart.

## Knobs

| Variable | Meaning | Default |
|---|---|---|
| `OPENROUTER_API_KEY` | set, the run grows the assistant and asks nothing | unset, the run asks on a terminal and otherwise boots the keyless colony |
| `MECLAW_EXAMPLE` | which colony to grow: `hard-shell`, `meclaw-os` or `organism` | `meclaw-os` with a key, `hard-shell` without |
| `MECLAW_MODEL` | the slug written to every `MODEL_*` token in the colony's `.env` | `openai/gpt-5.6-luna` |
| `MECLAW_VERSION` | install this version; `v1.2.3` and `1.2.3` mean the same | latest |
| `MECLAW_INSTALL_DIR` | where the binary goes | `~/.local/bin` |
| `MECLAW_HOME` | where templates, examples and colonies go | `~/.local/share/meclaw` |
| `MECLAW_TEMPLATES_URL` | take `templates/` and `examples/` from this `.tar.gz` (a mirror, an offline host) | the release asset |
| `MECLAW_PORT` | the first port to try | `7777` |
| `MECLAW_REPO` | owner/name on GitHub | `mmeyerlein/meclaw` |

## When it stops

Every failure names its step, `error: step 2 (templates) failed: ...`. The
install step is the installer's own message (platform, directory, checksum,
version). The templates step is the download or its sum. The boot step is no
free port, a seed that could not be copied, or a daemon that did not answer in
120 seconds; the last lines of `daemon.out` are printed. The grow step is a
declaration that was not committed (the receipt is printed), a verdict that did
not appear in the trace, or `meclaw ask` reporting an error, which is where a
wrong key ends up.
The daemon is left running so `/ui/` can be read.

At the end the script prints the colony directory, the pid, the API and UI
URLs and the `kill` line. Deleting the colony directory is the whole uninstall
of that colony; the binary and the templates are the two other places it
wrote to. A second run is a second colony on the next free port.
