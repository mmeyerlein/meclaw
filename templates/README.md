# templates/

Reference topologies you can instantiate. Every one of them is pure DSL -- directories,
`config.json` files and edges. No new Rust, no plugin API, no runtime dependency on this
folder at all.

## What a template is

A template is a physical subtree on disk: a `template.json` at its root (name, version and a
four-slot description that the colony serves over `/colony/templates`), one `config.json` per
cell, and sub-cells as nested directories. That is the whole format. What you read here is
exactly what the substrate reads.

**Instantiation COPIES.** When a mutation names a template, the colony materializes the subtree
into your colony root -- substituting `${uuid7:*}` on disk, keeping `${VAR}` as a late-binding
token -- and from that moment your instance is yours. It has no link back to this library. This
folder is not on the runtime path of a booted colony; delete it and every colony that was built
from it keeps running.

A template is also self-contained: it has no edges leaving its own subtree. Wiring it into a
colony is the job of the mutation that instantiates it (see below).

## The library

| Template | Version | What it is |
|---|---|---|
| [`archive-bridge`](archive-bridge/) | 1.0.0 | Turns an llm's last answer into a store-native insert -- and swallows the store's reply echo, so an append-only archive costs one cell instead of a loop. |
| [`cogny`](cogny/) | 1.3.0 | The agent core as one node: a tool loop of `collector` + `dispatcher` around an llm brain slot, every internal edge pre-wired. A talky without a channel. |
| [`collector`](collector/) | 1.2.0 | Context assembly as a hive: a rolling conversation window, the memory bundle and the tool round fanned back in -- each leg capped by configuration, not by a model's judgement. |
| [`dispatcher`](dispatcher/) | 1.0.0 | The fan-out half of a tool loop: splits a brain's `tool_call` bundle into one routable message per call, announces the round to the fan-in, and passes a final answer through. |
| [`door`](door/) | 1.0.0 | The first cell of a colony, as one code cell: it takes what the HTTP ingress delivers -- request headers in `context`, an empty hop -- and puts the turn on a named lane, carrying the channel identity with it. |
| [`firewall`](firewall/) | 1.0.0 | Deterministic screening in front of an agent: size, sender, forbidden literal and rate, each verdict naming the rule that fired. Nothing here asks a model, and nothing here can. |
| [`memory-drain`](memory-drain/) | 1.0.0 | The adapter between a closed session and a central memory: decomposes one write batch into single-turn episodes, in order, idempotent across replays. |
| [`memory-hive`](memory-hive/) | 1.2.0 | Agent memory as a hive: an LLM-free write path, a token-budgeted tier-0 bundle, a four-leg retrieval fan fused without a model, and a nightly consolidation that supersedes instead of deleting. Ten cells, no Rust. |
| [`receptionist`](receptionist/) | 1.0.0 | One agent per channel, built on demand: the first turn of a channel nobody has met instantiates a fresh `talky` for exactly that channel and hands the turn straight into it. |
| [`retry`](retry/) | 1.0.0 | A bounded retry loop around one tool, as a single cell. At the cap the give-up lane hands the last error on with its `error_code` intact. |
| [`session-keeper`](session-keeper/) | 1.0.0 | A session as a channel generation, modelled on a phone call: minted at the surface, stamped onto every inbound turn, ended by arithmetic (a timer plus an idle threshold) rather than by judgement. |
| [`summarizer`](summarizer/) | 1.0.0 | The handover step: when a generation closes, it folds the day's write batch into one recency-weighted summary and emits it as a `system.handover` update. |
| [`talky`](talky/) | 1.2.0 | The full composite agent: `session-keeper`, `collector`, `dispatcher` and `summarizer` around an llm brain slot, with the loopback, the close path and the handover return already wired. |
| [`terminal`](terminal/) | 1.0.0 | The last cell of a lane, as one code cell: it accepts anything and emits nothing. Its whole job is to be an address, so that a lane without a destination yet still HAS one -- the message arrives, the trace records it, and the dead-letter queue stays empty. |

Read a template's `template.json` before wiring it -- the `description` slots (`purpose`,
`use_when`, `not_in_scope`, `examples`) document its named ports, which is what you need in
order to connect it. `cogny` and `talky` are composites: they carry byte-identical copies of the
smaller templates as sub-units, so instantiating one of them pulls in nothing that is not in
this table.

## Instantiating one

Point a running colony at this directory and send an `add_nodes` mutation. The short form:

```bash
./target/release/meclaw --root ./mycolony --templates ./templates \
                        --daemon --api 127.0.0.1:7777

curl -s -X POST http://127.0.0.1:7777/colony/mutations \
  -H 'Content-Type: application/json' \
  -d '{"scope":"/","ctx":{},"diff":{
        "add_nodes":[{"name":"agent","template":"talky"}]
      }}'
```

Because a template has no outgoing edges, the node above lands connected to nothing -- and a
subtree that nothing crosses into derives inactive, so its long-running cells never spawn. Wire
the ports in the **same** mutation: one `add_edges` entry from an already active cell into the
template's entry port is enough to bring the whole subtree up on that one recompute.

```bash
curl -s -X POST http://127.0.0.1:7777/colony/mutations \
  -H 'Content-Type: application/json' \
  -d '{"scope":"/","ctx":{"model":"openai/gpt-4o-mini"},"diff":{
        "add_nodes":[{"name":"agent","template":"talky"}],
        "add_edges":[{"from":"./ingress","to":"./agent/keeper/stamp"}]
      }}'
```

The `ctx` block feeds the `${ctx.*}` placeholders a template declares -- `talky` wants a resolved
model literal. Which ports exist, and which of them are mandatory, is written in the template's
own `template.json`.

Working colonies built this way live in [`../examples/`](../examples/). Start with
[`hello`](../examples/hello/README.md) for the model itself, then
[`swarm`](../examples/swarm/README.md) for a tool loop. The mutation format is specified in
[`../docs/meclaw-overview.md`](../docs/meclaw-overview.md); the cell types a `config.json` may
declare are in [`../docs/cell-types.md`](../docs/cell-types.md).

## Versioning

A reference in a mutation is either `name` or `name@major.minor.patch`. With a version it is an
**exact** match (`talky@1.2.0`); without one, the highest version on disk wins. Semver ranges
(`^`, `~`) are not parsed today, so `talky@1` is not a resolvable reference -- when you see
`talky@1` in prose here or in an issue, it names the major line of the template, not a string you
put in a mutation.

Version numbers here move only forward, and a bump never reaches a colony that is already
running: instantiation copied the subtree, so a running instance is pinned to the bytes it was
built from. Upgrading is therefore always an explicit act -- instantiate the new version next to
the old one and move the edges -- never something that happens to you between restarts. That is
the reason the copy exists.

**Where pinning stops working today, and the intent.** A template lives in exactly one
directory, so a version bump **replaces** it: after `talky` goes to `1.2.0`, a mutation asking
for `talky@1.1.0` finds nothing and is rejected, even though that is precisely the request a
pin is supposed to survive. The pin protects a *running* instance (it already holds its copy);
it does not protect a *new* instantiation reproducing an old one. That asymmetry is a gap, not
a design. The intent: **starting with 0.9.0, superseded template versions remain available**,
so a pinned reference keeps resolving after the version it names has been superseded. Until
that lands, treat `name@exact-version` as a statement about what you built against, and vendor
the directory if you need to rebuild it later.

## Env knobs are an experimental surface

Most templates here take their tunables as `${KNOB}` substitutions out of `.env`. That is
convenient and it is temporary: the knobs are migrating onto the `params` block of the cells
that read them, template by template, over the `0.x` line. Until a template's migration lands,
**its knob names are not a compatibility promise** -- a name may change, split, or disappear in
any `0.x` release, and the template README is the only place that says what it is called today.

The migration is tracked in
[#138](https://github.com/mmeyerlein/meclaw/issues/138); `collector@1`
([#136](https://github.com/mmeyerlein/meclaw/issues/136)) is the reference pattern, and every
migration keeps its defaults bit-identical. What does **not** move: provider credentials and
endpoints stay in `.env`, because a secret in a `config.json` is a secret in the repository.

New templates are added by the same rule that governs everything else in this repository: a
subtree plus its gates, no substrate change. Contributions are welcome; see
[`../CONTRIBUTING.md`](../CONTRIBUTING.md).
