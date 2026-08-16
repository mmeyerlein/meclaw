<div align="center">

# meclaw

**Agent swarms that recursively evolve themselves.**

**Loops? I don't care. The swarm builds its own. Or it doesn't. Its call.**

[![ci](https://github.com/mmeyerlein/meclaw/actions/workflows/ci.yml/badge.svg)](https://github.com/mmeyerlein/meclaw/actions/workflows/ci.yml)
[![tests](https://img.shields.io/badge/tests-3800%2B%20passing-brightgreen)](#)
[![rust](https://img.shields.io/badge/rust-edition%202024-orange)](#)
[![license](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue)](#license)
[![stars](https://img.shields.io/github/stars/mmeyerlein/meclaw?style=social)](#)

</div>

---

Every agent framework ships you the same thing: a loop. Call the model, run a tool, feed the result back, call the model again, until some condition you wrote says stop. That loop is the harness. You hand-build it, you babysit it, you redeploy it when it's wrong.

meclaw doesn't ship you a loop.

An `llm` cell makes one provider call and emits one message. That's it. No inner loop, ever. The tool-loop, ReAct, plan-and-execute, every harness pattern you've ever wired by hand, becomes a shape in your filesystem instead. Tools are cells. The loop is an edge that routes back. The harness is topology.

And since the harness is just files on disk, the swarm can rewrite it. Add a cell. Rewire an edge. Decide a loop was the wrong shape entirely and build something else. You wrote the first version. From there, rewiring is a runtime move, yours or the swarm's.

Here's the whole thing in eight seconds. One task in, one answer out, and the
tool-loop you'd normally hand-write showing up as a path through the tree:

![meclaw: the loop is an edge](docs/demo.svg)

That's `docs/demo.sh` driving `examples/swarm`. Replay it yourself with
`asciinema play docs/demo.cast`.

<!-- asciinema: B4 -->

## Try it, no API key

`examples/hard-shell` is a colony with no `llm` cell in it, so there is nothing to
authenticate: no key, no model, no provider account. It only reaches outwards, and the whole
example is about where it refuses to. Everything below was run against a fresh clone; the
output is the real output.

From the 0.9.0 release on there is a prebuilt Linux x86_64 binary, and
[`scripts/install.sh`](scripts/install.sh) fetches it next to its published SHA-256, verifies
the sum, and writes one file into `~/.local/bin`. It downloads and checks data; it never pipes
code into a second shell:

```bash
curl -fsSL https://meclaw.ai/install.sh | sh
```

Building from source works on every release, needs a Rust toolchain, and is what the walkthrough
below was run against:

```bash
git clone https://github.com/mmeyerlein/meclaw
cd meclaw
cargo build --release          # the only slow step. minutes, once.
```

Boot the colony as a daemon. It comes up in well under a second:

```bash
./target/release/meclaw --root ./examples/hard-shell/seed \
                        --templates ./templates \
                        --daemon --api 127.0.0.1:7799
```

What is checked in is one cell. Not a framework's worth of scaffolding — one:

```console
$ curl -s http://127.0.0.1:7799/colony/registry | jq -c '.registry[] | {path, cell_type}'
{"path":"/probe","cell_type":"web_fetch"}
```

Now grow it while it runs. `grow.json` is a mutation: two nodes from the template library,
four edges, applied to a live colony without a restart.

```console
$ curl -s -X POST http://127.0.0.1:7799/colony/mutations \
       -H 'Content-Type: application/json' \
       -d @examples/hard-shell/grow.json | jq -c .
{"mutation":{"id":"01a00656-d847-72e3-b652-2fc23becf2e8","outcome":"committed"}}

$ curl -s http://127.0.0.1:7799/colony/registry | jq -c '.registry[] | {path, cell_type}'
{"path":"/surface","cell_type":"code"}
{"path":"/sink","cell_type":"code"}
{"path":"/probe","cell_type":"web_fetch"}
```

Three cells now, and nothing was redeployed. Point the colony at the address every
prompt-injected agent gets told to fetch — `169.254.169.254`, where the cloud hands out
instance credentials:

```console
$ curl -s -X POST http://127.0.0.1:7799/messages \
       -H 'Content-Type: application/json' \
       -d '{"target": "/surface",
            "body": {"messages": [{"origin": "assistant", "type": "tool_call", "id": "c1",
                                   "text": "{\"url\": \"http://169.254.169.254/latest/meta-data/iam/security-credentials/\"}"}]}}' | jq -c .
{"message_id":"01a00656-d885-7043-9d7b-550e08775200"}
```

Nothing in that seed configures a policy, an allow list or a security block. The refusal
below is the state the thing ships in. Then open <http://127.0.0.1:7799/ui/> and watch it,
or read the trace it just wrote — which is the next section.

## The organism at work

One message in, three hops, and the harness is the shape of the tree rather than a loop
someone wrote. This is the trace of the request above, straight out of the running colony:

```console
$ curl -s 'http://127.0.0.1:7799/colony/trace?limit=20' \
  | jq -r '.trace[] | "\(.from_path) -> \(.to_path)   ttl=\(.ttl)\n  hop:  \(.headers_json | fromjson | .hop | tostring)\n  body: \(.body_payload | fromjson | .messages[0] | .type + " | " + .text)\n"'

@external -> /surface   ttl=63
  hop:  {}
  body: tool_call | {"url": "http://169.254.169.254/latest/meta-data/iam/security-credentials/"}

/surface -> /probe   ttl=62
  hop:  {"chat_id":"default","duration_ms":17,"exit_code":0,"had_stderr":false,"route":"turn"}
  body: tool_call | {"url": "http://169.254.169.254/latest/meta-data/iam/security-credentials/"}

/probe -> /sink   ttl=61
  hop:  {"duration_ms":0,"error_code":"target_blocked","finish_reason":"error","operation":"web_fetch","route":"denied"}
  body: tool_result | web_fetch refuses 169.254.169.254: link-local 169.254.0.0/16 (cloud metadata)
```

Read down the `hop` column, because that is where the logic lives:

- **Hop 1** enters from `@external` — the HTTP ingress — with an empty hop. Nothing has
  decided anything yet.
- **Hop 2** carries `route: "turn"`. The `/surface` cell put the turn on a *named lane*; the
  edge to `/probe` fires on that name. The cell did not know `/probe` exists.
- **Hop 3** is the interesting one. `error_code: "target_blocked"` is a **typed** refusal, and
  the edge that routed it matched on the code, not on the prose — so the deny gets a lane of
  its own (`route: "denied"`) instead of dying quietly. There is **no `http_status`**, and that
  absence is the proof: the address was judged before the connect, so no packet left the
  machine.
- **`ttl` counts down** 63 → 62 → 61. A message that loops forever runs out of budget instead
  of running forever.

The dead-letter queue is empty, which is the point of the third cell — a refusal that
dead-letters is a refusal nobody sees:

```console
$ curl -s http://127.0.0.1:7799/colony/dead_letters | jq -c .
{"dead_letters":[]}
```

[`examples/hard-shell/WALKTHROUGH.md`](examples/hard-shell/WALKTHROUGH.md) takes this further,
command by command with the real output next to each one: `kill -9` the daemon mid tool run,
start a second one on the same directory, and watch the same absence of configuration hold.
Under two minutes, still no key.

## What meclaw is

A framework for building agentic harnesses, and swarms of them, as a directory tree. One Rust binary. Linux.

A harness is the scaffolding around an LLM: the tool-loop, the orchestration, the control flow, the part everyone hand-codes and nobody enjoys. In meclaw that's a shape in the tree. Compose them. Nest them (meta-harnesses, if you want the buzzword). Swarm them.

- **The filesystem is the topology.** The directory tree *is* your harness. Every node is a Cell (an actor). Folders marked `type: "hive"` are scopes: authority and mutation boundaries that hold the graph. No second config format to learn. The tree is the truth.
- **Cells are dumb. The edges do the thinking.** A cell has no idea who sent it a message or who comes next. It knows its contract, its params, and the one message in front of it. Routing, filtering, fan-out, loopback, all of it lives on the edges.
- **One LLM cell, a pile of tool cells.** `llm` thinks. Everything else (`bash`, `code`, `file`, `store`, `web_fetch`, `mcp`, and friends) is a tool cell. There is no built-in loop bolting them together. You draw it. That's the whole trick: the harness is topology, not control flow buried in someone's framework.
- **The swarm tunes itself.** Rewiring the graph is a runtime primitive. Any cell can emit a mutation and reshape the tree while the colony runs. The goal is agents that write those mutations themselves: read the topology, decide it needs another tool or a smarter loopback, ship the diff. Self-modification isn't a feature bolted on the side. It's the reason the thing exists.
- **It runs as a daemon.** A long-lived process you drive over HTTP, with a read-only web UI to watch the swarm do its thing. nginx energy. One binary, a few flags, a mode switch.
- **Durable and atomic.** Messages are atomic. State persists per cell in SQLite. Traces rebuild from a central log. Kill it, restart it, it picks up where it left off.

```bash
# fire up a swarm as a daemon, with an HTTP API and a UI to watch it:
meclaw --root ./examples/swarm --templates ./templates --daemon --api 127.0.0.1:7777
# then open http://127.0.0.1:7777/ui/
```

`examples/swarm` has an `llm` cell in it, so it wants a provider key in
`examples/swarm/.env` before it will boot. The quickstart above needs none, because
`hard-shell` has no `llm` cell at all. [`examples/README.md`](examples/README.md) says which is
which.

## Why it's different

| | Every other agent framework | meclaw |
|---|---|---|
| The harness | control flow hidden in the framework | a folder you can `ls` |
| The tool-loop | a `while` loop you babysit | an edge that routes back |
| Tuning the harness | edit code, redeploy, pray | a mutation the swarm writes itself |
| The LLM call | buried in hidden control flow | one cell, one call, one message |
| Defining an agent | code or YAML in a repo | a directory tree you `diff` and `git` |
| Language lock-in | a Python or JS SDK | none. it's an HTTP API |
| Substrate | "it scales" (citation needed) | Tokio actors, O(1) routing, supervised restarts |

meclaw is not BPMN. It is not Temporal. It does exactly one thing, LLM-shaped flows, and it makes them something you draw, inspect, and hand off to the agents to maintain.

## The shape of a colony

A colony is a folder. A colony that actually does something needs at least a hive and one cell. Only a hive carries a graph, so a lone cell with no edges just sits there routing to nobody:

```
my-colony/
└── main/                  # the root hive. holds the graph (the edges).
    ├── config.json        # type: "hive", params.graph wires its children
    └── responder/
        └── config.json    # type: "llm"
```

`config.json` says what a node is. The folder it sits in says where it is. That's the entire mental model. There is no step three.

## The whole vocabulary

That's the word. "Vocabulary." There isn't more to memorize.

- **Cell**: an actor. One Tokio task, one mailbox, single-threaded on the inside. Knows only its own contract.
- **Hive**: a folder marked `type: "hive"`. A scope, a boundary, and the thing that holds the graph. Not an actor itself.
- **Edge**: a routing rule between two paths, with a CEL condition and optional context tweaks. This is where the logic lives.
- **Colony**: the boss. Owns the registry, routing, templates, lifecycle, mutations. Routing is an O(1) lookup.
- **Template**: a cell or subtree under `templates/`. A class. An instance is a copy dropped into the tree.

## The cells you get out of the box

`llm` thinks. The rest are tool cells, the working muscle of any harness. Nobody wired a loop around them. You do that.

| cell | what it does |
|---|---|
| `llm` | one provider call, one message. any OpenAI-compatible API: OpenAI, OpenRouter, or local via Ollama / vLLM / LiteLLM |
| `bash` | run a shell command, hand back stdout / stderr / exit code |
| `code` | run a script (Python), emit one message or many |
| `file` / `edit` | read, write, and patch files |
| `store` | durable key/value and history, with full-text search, graph traversal, vector similarity, and canonical identity that folds drifted spellings onto one axis |
| `timer` | cron-style scheduling, fires messages |
| `proxy` | a long-running bridge to the outside world (Telegram, or Slack over Socket Mode) |
| `mcp` | speak the Model Context Protocol to external tools |
| `web_fetch` / `web_search` | pull the web in |
| `harness` | run an agent harness (Claude Code, say) as a supervised child process, one child per task |
| `subcolony` | a whole child colony, addressed as if it were a single cell |

A tool-loop is `llm → dispatcher → tools → collector → llm`, with the loopback condition sitting on one edge. You don't switch a loop on. You compose one. And once it exists as files, the swarm can rebuild it without asking you.

## The template library

Both halves of that tool loop already exist as templates, and so does most of an agent.
**14 of them ship in this repository**, every one pure DSL — directories, `config.json` files
and edges, not a line of Rust and no plugin API. Instantiating one *copies* the subtree into
your colony; from that moment the instance is yours and has no link back to the library.

| | |
|---|---|
| the tool loop | `dispatcher` fans a brain's tool calls out, `collector` decides what comes back into the context window |
| the conversation | `session-keeper` gives a conversation a beginning and an end, `summarizer` writes the handover |
| the front door | `door`, `firewall` (rules that are data, not code), `receptionist` (one agent per channel, built on demand) |
| memory | `memory-hive` — ten cells, an LLM-free write path and a nightly consolidation — plus `memory-drain` and `archive-bridge` |
| whole agents | `cogny` is the agent core as one node; `talky` is the full composite, four sub-units pre-wired |
| the small ones | `retry`, `terminal` |

The catalogue, with what each one is for and which ports to wire, is
[`templates/README.md`](templates/README.md). `examples/meclaw-os` boots a seed with **zero
cells in it** and grows a seventeen-cell agent out of that library with one declaration.

## It rewrites itself. That's the point.

Reshaping the graph is a first-class runtime move. A cell emits a mutation diff (`add_nodes`, `add_edges`, `swap_nodes`, and the rest), the colony validates it and applies it atomically while everything keeps running. This part is real and tested today.

The whole idea is agents that maintain their own harness. Read the current topology, notice it needs another tool cell or a smarter loopback, write the diff, ship it.

That part shipped. The **builder-hive** is an `llm` plus `code` topology that turns a plain-English request into a validated, deployed subtree — and it is itself pure DSL, not one line of new Rust. The rails are the interesting part, because they are measured substrate behaviour rather than a promise: a cell cannot even *address* the mutation lane without an edge, and no mutation whatsoever can create an edge onto the control plane — that one is bootstrap-only. Approval is classified by effect, not by name: a fresh unwired subtree is inert and auto-approved, while rerouting live traffic or touching the control plane escalates to a human.

## Where it's at

meclaw is **v0.10.1**. A proof of concept for the DSL and the self-modifying substrate, with a deliberately frozen on-disk schema — that is the `colony.db` `schema_version`, the persistence layout, not the DSL. The DSL keeps growing; the database you already have keeps opening.

Real and tested today: the full actor substrate, all 14 built-in cell types, hot and cold lifecycle, runtime mutations, the template system, long-running cells, the HTTP API and web UI, the builder-hive, agent harnesses as supervised child processes, and child colonies composed as single cells. **3800+ tests. 0 fail. And climbing.** The hot routing paths are byte-pinned against fixtures, so they can't quietly drift.

Not here yet: **composition, not federation.** A child colony is addressable as one cell, and that boundary is pinned by negative tests — a parent path into the child tree does not route, and a mutation scoped into the child creates nothing. Cross-colony routing is a deliberate non-goal, not a missing feature. One builder per scope. A few hardening items are tracked in the open. This is honest infrastructure, not a toy. It's also not something to run unsupervised in production yet. The `bash` cell has full shell access on purpose, so run untrusted topologies somewhere you don't mind a shell.

## Stability

**What is the contract, and what is not.** Four surfaces are the public contract of this project:

- the **HTTP API** — the `/colony/*` routes, `POST /messages`, their query parameters and status codes;
- the **template DSL** — the `template.json` and `config.json` schemas, including the mutation diff format;
- the **template ports** — the endpoints a template's README declares as its ingress and exit addresses;
- the **documented `error_code` strings** — the dead-letter codes and the cell-type error enums.

While meclaw is on `0.x`, changes to those four are **additive**: new fields, new codes, new
endpoints, new params. A change that breaks an existing topology is a **breaking change** and gets
its own **Breaking** section in [CHANGELOG.md](CHANGELOG.md), naming what breaks and what to do
about it. If it is not in that section, it was not meant to break you — file an issue.

One carve-out inside that: the `${KNOB}` environment variables the shipped templates read are an
**experimental** surface and are migrating onto the `params` block over the `0.x` line, so their
names are not covered by the promise above ([#138](https://github.com/mmeyerlein/meclaw/issues/138),
[`templates/README.md`](templates/README.md) § Env knobs).

**The Rust crates are internals.** Everything under `crates/` is `publish = false` and carries no
SemVer guarantee on any Rust item: types, function signatures, module paths and trait bounds move
between releases without notice, including on patch bumps. There is no `meclaw` library API. If you
depend on a crate over a git dependency, pin a commit and expect to do the work yourself on every
bump — that is a supported thing to do and an unsupported thing to be surprised by.

## What it costs to run

A colony spends money in exactly one place: the provider calls its `llm` cells make. On one
production colony, measured from its own database over a 24-hour window:

**0.32 EUR / 24 h** with a person talking to it, **0.024 EUR / 24 h** unattended.

The method matters more than the number, because the number is dominated by how much you talk
to the colony and which cells sit on which model tier — in that measurement the frontier model
was 97 % of the bill on 24 % of the calls. Every figure is re-derivable against your own
colony with [`scripts/cost_report.py`](scripts/cost_report.py), and
[`docs/costs.md`](docs/costs.md) states plainly which tiers have **not** been measured rather
than estimating them.

## Roadmap

**Now: meclaw-os.** The substrate had its waves; the first full agent is being built on top of
it as pure topology, no new Rust — context orchestration, a conversation lifecycle, a screened
front door, and memory the agent can ask rather than be handed. Every piece of it is a template
you can read, and they are all in [`templates/README.md`](templates/README.md). The epic is
[#26](https://github.com/mmeyerlein/meclaw/issues/26); every decision and open fork is tracked
there in the open.

Also in the queue: cutting the fixed cost of a `code` cell invocation. We measured it instead of guessing — the driver is ~16 ms of interpreter startup per call, not the store and not the payload, so the cost equation is (number of `code` calls on the serial path) × 16 ms. That one line is worth roughly 90 % of the available speedup. After that: more than one builder per scope, capability checks with teeth, durability hardening. All of it is in the open. Pick one, send a PR.

## Contributing

Issues, discussions, PRs, all open. Easy wins: example colonies, new template cells, docs. The spec in `docs/` is the source of truth, so read `docs/meclaw-overview.md` before anything big. What comes next and in which order lives in [ROADMAP.md](ROADMAP.md); the issue tracker carries the substance.

## License

Take your pick:

- MIT ([LICENSE-MIT](LICENSE-MIT))
- Apache 2.0 ([LICENSE-APACHE](LICENSE-APACHE))

Whichever you like.

---

<div align="center">

**No loops were used in the making of this framework.**

If that line made you twitch, you're exactly who this is for. Drop a ⭐.

</div>
