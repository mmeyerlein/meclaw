<div align="center">

# meclaw

**Build an agentic harness swarm as a directory tree. Then watch it refactor itself behind your back.**

**Loops? I don't care. The swarm builds its own. Or it doesn't. Its call.**

[![build](https://img.shields.io/badge/build-passing-brightgreen)](#)
[![tests](https://img.shields.io/badge/tests-closing%20in%20on%202000-brightgreen)](#)
[![rust](https://img.shields.io/badge/rust-edition%202024-orange)](#)
[![license](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue)](#license)
[![stars](https://img.shields.io/github/stars/mmeyerlein/meclaw?style=social)](#)

</div>

---

Every agent framework ships you the same thing: a loop. Call the model, run a tool, feed the result back, call the model again, until some condition you wrote says stop. That loop is the harness. You hand-build it, you babysit it, you redeploy it when it's wrong.

meclaw doesn't ship you a loop.

An `llm` cell makes one provider call and emits one message. That's it. No inner loop, ever. The tool-loop, ReAct, plan-and-execute, every harness pattern you've ever wired by hand, becomes a shape in your filesystem instead. Tools are cells. The loop is an edge that routes back. The harness is topology.

And since the harness is just files on disk, the swarm can rewrite it. Add a cell. Rewire an edge. Decide a loop was the wrong shape entirely and build something else. You wrote the first version. After that, it's the swarm's problem.

Here's the whole thing in eight seconds. One task in, one answer out, and the
tool-loop you'd normally hand-write showing up as a path through the tree:

![meclaw: the loop is an edge](docs/demo.svg)

That's `docs/demo.sh` driving `examples/swarm`. Replay it yourself with
`asciinema play docs/demo.cast`.

## What meclaw is

A framework for building agentic harnesses, and swarms of them, as a directory tree. One Rust binary. Linux.

A harness is the scaffolding around an LLM: the tool-loop, the orchestration, the control flow, the part everyone hand-codes and nobody enjoys. In meclaw that's a shape in the tree. Compose them. Nest them (meta-harnesses, if you want the buzzword). Swarm them.

- **The filesystem is the topology.** The directory tree *is* your harness. Every node is a Cell (an actor). Folders marked `type: "hive"` are scopes: authority and mutation boundaries that hold the graph. No second config format to learn. The tree is the truth.
- **Cells are dumb. The edges do the thinking.** A cell has no idea who sent it a message or who comes next. It knows its contract, its params, and the one message in front of it. Routing, filtering, fan-out, loopback, all of it lives on the edges.
- **One LLM cell, a pile of tool cells.** `llm` thinks. Everything else (`bash`, `code`, `file`, `store`, `web_fetch`, `mcp`, and friends) is a tool cell. There is no built-in loop bolting them together. You draw it. That's the whole trick: the harness is topology, not control flow buried in someone's framework.
- **The swarm tunes itself.** Rewiring the graph is a runtime primitive. Any cell can emit a mutation and reshape the tree while the colony runs. The point is that the agents write those mutations themselves: read the topology, decide it needs another tool or a smarter loopback, ship the diff. Self-modification isn't a feature bolted on the side. It's the reason the thing exists.
- **It runs as a daemon.** A long-lived process you drive over HTTP, with a read-only web UI to watch the swarm do its thing. nginx energy. One binary, a few flags, a mode switch.
- **Durable and atomic.** Messages are atomic. State persists per cell in SQLite. Traces rebuild from a central log. Kill it, restart it, it picks up where it left off.

```bash
# fire up a swarm as a daemon, with an HTTP API and a UI to watch it:
meclaw --root ./examples/swarm --daemon --api 127.0.0.1:7777
# then open http://127.0.0.1:7777/ui/
```

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

## Quickstart

```bash
# build it (Linux)
git clone https://github.com/mmeyerlein/meclaw
cd meclaw
cargo build --release
# binary: ./target/release/meclaw

# run a swarm as a daemon and watch it work
./target/release/meclaw --root ./examples/swarm --daemon --api 127.0.0.1:7777
# open http://127.0.0.1:7777/ui/
```

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
| `store` | durable key/value and history |
| `timer` | cron-style scheduling, fires messages |
| `proxy` | a long-running bridge to the outside world (Telegram) |
| `mcp` | speak the Model Context Protocol to external tools |
| `web_fetch` / `web_search` | pull the web in |

A tool-loop is `llm → dispatcher → tools → collector → llm`, with the loopback condition sitting on one edge. You don't switch a loop on. You compose one. And once it exists as files, the swarm can rebuild it without asking you.

## It rewrites itself. That's the point.

Reshaping the graph is a first-class runtime move. A cell emits a mutation diff (`add_nodes`, `add_edges`, `swap_nodes`, and the rest), the colony validates it and applies it atomically while everything keeps running. This part is real and tested today.

The whole idea is agents that maintain their own harness. Read the current topology, notice it needs another tool cell or a smarter loopback, write the diff, ship it. The mutation engine is here now. The builder-hive, an `llm` plus `code` topology that writes those diffs straight from a plain-English request, is the next thing on the bench. Not shipped yet. See the roadmap.

## Where it's at

meclaw is **v0.1.0**. A proof of concept for the DSL and the self-modifying substrate, with a schema that's deliberately frozen.

Real and tested today: the full actor substrate, every built-in cell type, hot and cold lifecycle, runtime mutations, the template system, long-running cells, the HTTP API and web UI. **Closing in on 2,000 tests. 0 fail. 0 ignored. And climbing.** The hot routing paths are byte-pinned against fixtures, so they can't quietly drift.

Not here yet: the builder-hive (plain English to topology) is next, not shipped. Single colony only, no federation. One builder per scope. A few hardening items are tracked in the open. This is honest infrastructure, not a toy. It's also not something to run unsupervised in production yet. The `bash` cell has full shell access on purpose, so run untrusted topologies somewhere you don't mind a shell.

## Roadmap

Next up: the builder-hive (requests in, graph mutations out). After that: federation across colonies, more than one builder per scope, capability checks with teeth, durability hardening. All of it is in the open. Pick one, send a PR.

## Contributing

Issues, discussions, PRs, all open. Easy wins: example colonies, new template cells, docs. The spec in `docs/` is the source of truth, so read `docs/meclaw-overview.md` before anything big.

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
