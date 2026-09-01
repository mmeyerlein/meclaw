<div align="center">

# meclaw

**Where agents build agents.**

**An agentic build system for agentic systems. Ontology-grounded, auditable, one Rust binary.**

[![ci](https://github.com/mmeyerlein/meclaw/actions/workflows/ci.yml/badge.svg)](https://github.com/mmeyerlein/meclaw/actions/workflows/ci.yml)
[![tests](https://img.shields.io/badge/tests-6200%2B%20passing-brightgreen)](#)
[![rust](https://img.shields.io/badge/rust-edition%202024-orange)](#)
[![license](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue)](#license)

[Start an assistant](#start-an-assistant) · [Docs](docs/README.md) ·
[Templates](templates/README.md) · [Examples](examples/) · [Roadmap](ROADMAP.md)

</div>

meclaw is three things, and you only install the first one. **meclaw** is the substrate: a
directory tree that runs — every folder an actor, every edge a route, one Rust binary
underneath. **meclaw-os** is a small, experimental operating system for agents, *grown*
onto that substrate at runtime. An **assistant** is grown into the OS the same way — a
JSON file, not a deployment. Install once, grow everything else.

## Start an assistant

```bash
# 1 — install meclaw: one static Linux binary
curl -fsSL https://meclaw.ai/install.sh | sh
git clone https://github.com/mmeyerlein/meclaw && cd meclaw
printf 'OPENROUTER_API_KEY=sk-...\nMODEL_BRAIN=openai/gpt-4o-mini\n' > examples/meclaw-os/seed/.env
meclaw --root examples/meclaw-os/seed --templates ./templates --daemon --api 127.0.0.1:7777

# 2 — install the OS into the running colony: one POST, nothing restarts
curl -s -X POST 127.0.0.1:7777/colony/mutations \
     -H 'Content-Type: application/json' -d @examples/meclaw-os/grow.json

# 3 — talk to your assistant
curl -s -X POST 127.0.0.1:7777/messages -H 'Content-Type: application/json' \
     -d '{"target": "/door", "headers": {"channel": "chat-1"},
          "body": {"messages": [{"origin": "user", "type": "text",
                                 "text": "Say hello in one short sentence."}]}}'

# 4 — read the answer: nothing is hidden, the reply is a hop on the record
curl -s '127.0.0.1:7777/colony/trace?limit=200' | jq -r \
  '[.trace[] | select((.headers_json | fromjson | .hop.route) == "answer")]
   | last | .body_payload | fromjson | .messages[0].text'
```

One binary, one key, four commands — and the last one already shows the point:
the answer is not a return value, it is a message on the record.

## What just happened

**You installed meclaw.** A single Rust binary that turns a directory tree into a running
colony of actors: every folder is a cell, its `config.json` is its definition, and the
edges between folders are the routes a message can take. Nothing else got installed.

**You installed an operating system — without stopping anything.** `grow.json` is not
code and was not deployed. It is a *mutation*: nodes and edges, applied over HTTP to a
colony that was already running. It grew a door, a firewall and a conversation agent out
of the template library — and that is the only way anything is ever added to a colony,
which is why the same door is open to the agents themselves.

**You talked to an assistant nobody programmed.** No SDK, no agent class, no loop you
wrote. The assistant is a shape in the filesystem, grown from templates — and the full
version of that shape ([`examples/organism`](examples/organism/)) grows an organisation,
a person, their assistant and their channels from five such files.

meclaw is different enough that the same questions come up every time. The rest of this
page is those questions — each a few lines here, each a real page in
[`docs/why/`](docs/why/).

## meclaw doesn't ship you a loop

Every agent framework ships you the same thing: a loop — call the model, run a tool, feed
the result back, until some condition you wrote says stop. You hand-build that harness
and redeploy it when it's wrong. In meclaw an `llm` cell makes **one** provider call and
emits **one** message; tools are cells, the loop is an edge that routes back, the harness
is topology. Since topology is files, the swarm can rewrite its own harness while it runs.

## Everything is a file

Flexibility is not a feature here — it is the consequence of one decision. Because the
harness lives in the filesystem, `ls`, `grep`, `diff` and `git` are the tooling, every
change is diffable, and an agent rebuilds its own topology with the same closed vocabulary
a human uses. There is no SDK and no plugin API, and that is deliberate: the interface is
HTTP and files, and 38 shipped templates without a line of Rust are the proof.
*More: [docs/why/everything-is-a-file.md](docs/why/everything-is-a-file.md)*

## An operating system for agents

Every agentic product ends up rebuilding the same things: an organisation, its people,
their assistants, the channels they are reached on — plus secrets, screening, sessions
and a control loop across all of them. meclaw-os ships those as templates under one rule:
**a level owns what its siblings must share.** It is rudimentary and experimental, it
already has the concept of **apps**, and it exists so a new agent is a grow, not a project.
*More: [docs/why/an-os-for-agents.md](docs/why/an-os-for-agents.md)*

## One assistant, two brains

The shipped assistant runs two models on purpose: a conversation surface that answers
fast, and a reasoning core that thinks — one job, one brain, one tool menu each, and the
menu is asked for rather than typed into a prompt. One model doing both is either slow in
conversation or shallow in reasoning; the split is a harness decision, and the harness is
a file. *More: [docs/why/two-brains.md](docs/why/two-brains.md)*

## Memory that outlives the window

A conversation can run for weeks — not because something clever compacts the context, but
because **the window was never where the conversation was stored**. The memory hive
writes without an LLM, retrieves over five model-free legs, consolidates nightly by
superseding instead of deleting, and the window is assembled per turn out of the record,
under a budget. *More: [docs/why/memory.md](docs/why/memory.md)*

## Ontology, in the meclaw sense

Not philosophy: a typed catalogue. The builder designs against the template library and
its declarations and is validated by them, rather than emitting free-form JSON somebody
hopes parses. When the catalogue has no word for what you want, the manifest brings one —
`add_templates` registers a new class into a running colony. Apps are how the ontology
learns new words. *More: [docs/why/ontology.md](docs/why/ontology.md)*

## Prepared for recursive self-improvement

The primitives are here and tested: runtime mutation, a builder that turns a wish into a
manifest, keep-or-revert on a measured window, a receipt for every act. **The loop that
closes them is not** — deliberately. Nothing in this repository improves itself
unattended, and every goal the control loop could pursue ships disabled. No blind RSI.
*More: [docs/why/rsi.md](docs/why/rsi.md)*

## You talk, it shows

**This one is an idea, not a feature.** The vision for the assistant is the movie *Her*:
you **talk** to it, and it **shows** you — lists, plans, pictures, drawn onto a display
that belongs to you, not to any one agent. Nothing in this repository does voice today;
what exists is the window it would draw on.
*The idea, and what already stands under it: [docs/why/you-talk-it-shows.md](docs/why/you-talk-it-shows.md)*

## The strange names

argus, affinity, talky, cogny, hive — the names are roles, not branding, and each has a
one-line reason. *More: [docs/why/names.md](docs/why/names.md)*

## Why Rust, why Linux only

One static binary, one async task per cell — and the security model *is* the kernel:
Landlock, network namespaces, cgroup v2 and seccomp, fail-closed. Without those
primitives, "sandboxed" would be a promise instead of a property; that is why there is no
macOS build. Authentication is the reverse proxy's job, as for every Linux daemon.
*More: [docs/why/rust-and-linux.md](docs/why/rust-and-linux.md)*

## Limits

`code` cells run `python3`, nothing else. One screen, one app; voice is roadmap, not a
feature. Not for unsupervised production yet. Running costs, measured on one production
colony: 0.32 EUR per day in conversation, 0.024 EUR idle
([`docs/costs.md`](docs/costs.md)).

## Under heavy development

meclaw is not finished, and it is open source so it does not have to be finished alone.
Good first contributions: example colonies, template cells, docs drift-fixes — see
[CONTRIBUTING.md](CONTRIBUTING.md) and the `good first issue` label. **6200+ tests,
0 fail**; release truth lives in [CHANGELOG.md](CHANGELOG.md).

## Stability

**Five surfaces are the public contract of this project:**

- the **HTTP API** — the `/colony/*` routes, `POST /messages`, their query parameters and
  status codes;
- the **template DSL** — the `template.json` and `config.json` schemas, including the
  mutation diff format;
- the **template ports** — the endpoints a template's README declares as its ingress and
  exit addresses;
- the **`web` cell's own origin** — the route grammar a `page.set` accepts, the two
  reserved names (`/live/websocket` and `/@client/*`), and the closed component-template
  syntax ([`docs/cell-types.md`](docs/cell-types.md) § `web`); the removed `/surface/*`
  prefix and `cell.surface` key stay removed
  ([#383](https://github.com/mmeyerlein/meclaw/issues/383));
- the **documented `error_code` strings** — the dead-letter codes, the cell-type error
  enums, and the codes a `/colony` read reply carries
  ([#363](https://github.com/mmeyerlein/meclaw/issues/363)).

While meclaw is on `0.x`, changes to those five are **additive**. A change that breaks an
existing topology gets its own **Breaking** section in [CHANGELOG.md](CHANGELOG.md),
naming what breaks and what to do about it — if it is not in that section, it was not
meant to break you: file an issue. Two carve-outs: the `${KNOB}` environment variables
the shipped templates read are a declared **experimental** surface migrating onto
`params` ([#138](https://github.com/mmeyerlein/meclaw/issues/138)), and **the Rust crates
are internals** — nothing under `crates/` carries a SemVer guarantee, and there is no
`meclaw` library API.

## Docs

[`docs/README.md`](docs/README.md) is the index. First stops:
[glossary](docs/glossary.md) (the words you need first) ·
[system overview](docs/meclaw-overview.md) ·
[cell types](docs/cell-types.md) · [config format](docs/config.md) ·
[template catalogue](templates/README.md) · [examples](examples/).

## License

MIT ([LICENSE-MIT](LICENSE-MIT)) or Apache 2.0 ([LICENSE-APACHE](LICENSE-APACHE)) —
whichever you like.

---

<div align="center">

**No loops were used in the making of this framework.**

If that line made you twitch, you're exactly who this is for. Drop a ⭐.

</div>
