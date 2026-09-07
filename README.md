# meclaw

[![ci](https://github.com/mmeyerlein/meclaw/actions/workflows/ci.yml/badge.svg)](https://github.com/mmeyerlein/meclaw/actions/workflows/ci.yml) [![release](https://img.shields.io/github/v/release/mmeyerlein/meclaw)](https://github.com/mmeyerlein/meclaw/releases) [![license](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue)](#license)

[Docs](docs/README.md) · [Glossary](docs/glossary.md) · [Templates](templates/README.md) · [Examples](examples/README.md) · [Roadmap](ROADMAP.md) · [Contributing](CONTRIBUTING.md)

One Linux binary that runs a tree of agents. Every folder in the tree is an entity: one actor, one
`config.json`, one SQLite file, one kernel sandbox (Landlock, network namespace, cgroup v2,
seccomp). Edges between folders are the routes a message may take. The binary ships no agent loop; a
loop is an edge that routes back into an `llm` entity. To change a running system you POST a diff to
one endpoint: validated, applied without a restart, written to a ledger. Agents change the system
through the same endpoint. **meclaw-os** and an **assistant** are grown onto that tree at runtime
from JSON, not deployed.

You can rewire an assistant while it is still answering: the change is a diff of nodes and edges,
and nothing restarts. An agent that wants to change the tree goes through the same endpoint. The
shipped `builder` template drafts such a diff and holds no edge to that endpoint; `submit` is the
one node that does. 40 templates ship as JSON declarations and Python scripts, and `templates/`
contains no Rust. The docs call an entity a cell and the whole tree a colony.

## Start an assistant

```bash
# 1 — install meclaw: one static Linux binary (lands in ~/.local/bin)
curl -fsSL https://meclaw.ai/install.sh | sh
export PATH="$HOME/.local/bin:$PATH"
# the templates must match the binary: clone the tag the installer just gave you
git clone --depth 1 --branch "v$(meclaw --version | cut -d' ' -f2)" \
    https://github.com/mmeyerlein/meclaw && cd meclaw
# one key — replace sk-... with a real one (https://openrouter.ai/keys), or step 3 ends in code=auth
printf 'OPENROUTER_API_KEY=sk-...\nMODEL_BRAIN=openai/gpt-4o-mini\n' > examples/meclaw-os/seed/.env
# 7777 is an arbitrary free port: if it is taken, change it in every line below as well.
# The very first start reads a 25 MB binary from cold disk and can stay silent for ~40 s; every later start takes well under a second.
meclaw --root examples/meclaw-os/seed --templates ./templates --daemon --api 127.0.0.1:7777

# 2 — install the OS into the running colony: one POST, nothing restarts
curl -s -X POST 127.0.0.1:7777/colony/mutations \
     -H 'Content-Type: application/json' -d @examples/meclaw-os/grow.json

# 3 — talk to your assistant
curl -s -X POST 127.0.0.1:7777/messages -H 'Content-Type: application/json' \
     -d '{"target": "/door", "headers": {"channel": "chat-1"},
          "body": {"messages": [{"origin": "user", "type": "text",
                                 "text": "Say hello in one short sentence."}]}}'

# 4 — read the answer: nothing is hidden, the reply is a hop on the record (needs jq)
curl -s '127.0.0.1:7777/colony/trace?limit=200' | jq -r \
  '[.trace[] | select((.headers_json | fromjson | .hop.route) as $r | $r == "answer" or $r == "error")]
   | last | if . == null then "no answer yet — the colony is still working; watch it at http://127.0.0.1:7777/ui/"
            else .body_payload | fromjson | .messages[0].text end'

# 5 — watch the colony in the browser: http://127.0.0.1:7777/ui/
```

`--daemon` runs in the foreground and stops on Ctrl-C, so step 1 keeps its terminal. Run steps 2 to
5 in a second shell.

## Where it stands

Linux x86_64 only. The release is a static musl build, and the installer refuses any other platform.
A `code` cell runs `python3` and no other runner. The binary has no SDK and no plugin API. HTTP and
files are the interface. The daemon installs no authentication and no TLS. Put a reverse proxy in
front of it, like any Linux daemon. meclaw is under heavy development, and I would not leave it
unattended in production. The 0.32.0 release gate ran 6875 tests. One measured colony spent 0.32 EUR
on a day of conversation ([docs/costs.md](docs/costs.md)).

## Stability

Five surfaces are the public contract of this project: the HTTP API, the template DSL, the template
ports, the `web` cell's own origin, and the documented `error_code` strings. The API means the
`/colony/*` routes and `POST /messages`. The DSL means the `template.json` and `config.json`
schemas, including the mutation diff format. The ports are the ingress and exit endpoints a
template's README declares. The `web` origin is the `page.set` route grammar and its two reserved
names; the `error_code` strings are the documented dead-letter, cell-type and `/colony` read codes.
While meclaw is on `0.x`, changes to those five are additive. A change that breaks an existing
topology gets its own Breaking section in [CHANGELOG.md](CHANGELOG.md). Nothing under `crates/`
carries a SemVer guarantee.

## Docs

| If you want to | Read |
|---|---|
| read the whole system once | [system overview](docs/meclaw-overview.md) |
| know what a cell type does | [cell types](docs/cell-types.md) |
| write a `config.json` | [config format](docs/config.md) |
| change a colony while it runs | [rewiring](docs/rewiring.md) |
| pick a template | [template catalogue](templates/README.md) |
| read a measured transcript | [hard-shell walkthrough](examples/hard-shell/WALKTHROUGH.md) |
| know why it is built this way | [docs/why/](docs/why/) |

## License

MIT ([LICENSE-MIT](LICENSE-MIT)) or Apache 2.0 ([LICENSE-APACHE](LICENSE-APACHE)), whichever you
like.
