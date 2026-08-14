# `receptionist@1.0.0`

One agent per channel, built the moment a channel first speaks. Two cells under
one hive: `greet` (a `code` cell) and `ledger` (a `store`). No new cell type, no
Rust.

**A shared communicator flattens every surface it serves** (GH #29). Chat
etiquette, formatting and pacing differ per channel, and one `talky` in front of
many chats mixes their windows into one. The receptionist is the front door that
keeps them apart: the first turn of a channel nobody has met makes it
instantiate a fresh [`talky@1`](../talky/) for exactly that channel -- **one**
mutation carrying the `add_nodes` **and** all four crossing port edges -- and
the turn that triggered it follows the ingress edge that mutation just drew.

## What it delivers

- **A ledger, not a guess.** One row per channel, naming the instance that owns
  it. A row means known and the turn is handed straight through; no row means
  new. Nothing polls the registry, nothing probes the graph.
- **One mutation per channel, ports included.** `add_nodes` plus the ingress,
  reply, write and error edges travel in ONE diff, so the subtree is
  edge-connected from apply and derives ACTIVE -- an island without a crossing
  edge never wakes its timer (the one-mutation-per-connection discipline of the builder docs).
- **The triggering turn is not lost.** It is emitted *after* the mutation, in the
  same burst. The colony's outputs arm dispatches a `/colony/mutations` emission
  INLINE, before it takes the next emission off the mailbox, so the ingress edge
  is in the table when the turn behind it is routed. Emitting the turn first
  loses it -- pinned as a probe in the receipt, and the reason the order in
  `greet` is not cosmetic.
- **Self-locating.** The cell reads its own path off the envelope (`target`), so
  the mutation's `scope` is the hive's PARENT and the `from` of the ingress edge
  is the cell itself. Nothing is configured twice, and moving the reception to
  another scope changes nothing.
- **A burst on a cold channel does not fork it.** Two turns can both find the
  ledger empty; the colony serialises the mutations, exactly one commits, and
  the loser needs no repair because every turn rides behind its own mutation.

## Cells

| path | type | role |
|---|---|---|
| `greet` | `code` | the pass: ask the ledger, hand through or instantiate |
| `ledger` | `store` | `channels(channel, talky_path, created_at)` |

Two internal edges: `greet -> ledger` on `hop.route == 'rstore'` (promoting the
step, the channel and the parked turn to context), and `ledger -> greet` back on
`context.rec_origin == 'greet'`.

## Ports

Two edges, and **the parent draws both**.

| port | endpoint | direction | note |
|---|---|---|---|
| ingress | `./greet` | in | sets `hop.route='in_turn'` and promotes `context.channel` (mandatory) |
| mutation lane | `./greet -> /colony/mutations` | out | **bootstrap only** -- see below |

```json
{"from": "<surface>", "to": "./reception/greet",
 "condition": "has(hop.route) && hop.route == 'turn'",
 "modifier": {"set_hop": {"route": "'in_turn'"},
              "set_context": {"channel": "hop.chat_id"}}},
{"from": "./reception/greet", "to": "/colony/mutations",
 "condition": "has(hop.msg_type) && hop.msg_type == 'mutation'"}
```

**The mutation lane is bootstrap-only, and that is the point.** No mutation can
create an edge to a `/colony` endpoint at any scope -- `validate_scope_containment`
answers `scope_out_of_bounds` for every one of them, and a template's own
`params.graph` is containment-checked against its subtree root, so the edge
cannot travel inside this template either. It exists exactly if an operator
wrote it into a `config.json` before boot, and no topology can grant itself the
lane afterwards (cookbook
[`colony-endpoint-roundtrip.md`](../../cookbook/colony-endpoint-roundtrip.md),
rule 4; same shape as [`builder-hive`](../builder-hive/)).

Consequence for the gate runbook: a receptionist instantiated by `add_nodes`
registers and activates, but its mutation lane is **inert** until an operator
adds the edge and reboots. A receptionist belongs in the bootstrap tree.

**The channel promotion is not optional.** Without `set_context: {"channel": ...}`
every chat of the colony collapses onto the channel `default` -- which is one
talky for everybody, exactly the thing this template exists to prevent. Whatever
the surface calls "the same conversation partner" goes in there: a
Telegram/Slack `chat_id`, a room, a phone number.

**Numbers on the hop need `int()`.** A proxy delivers JSON integers, CEL
deserialises them as `uint`, and a bare `hop.chat_id == 12345` is silently
**false** -- no error, no log line. Any numeric condition on YOUR ingress edge
carries the cast. The edges the reception draws itself do not need it: `greet`
owns `hop.chan` and always writes it as a string.

### What the reception draws per channel

One mutation, `scope` = the hive's parent, `ctx.model` = `RECEPTIONIST_MODEL`:

```json
{"scope": "/", "ctx": {"model": "openai/gpt-4o-mini"}, "diff": {
  "add_nodes": [{"name": "talky-<key>", "template": "talky"}],
  "add_edges": [
    {"from": "./reception/greet", "to": "./talky-<key>/keeper/stamp",
     "condition": "has(hop.route) && hop.route == 'turn' && has(hop.chan) && hop.chan == '<key>'",
     "modifier": {"set_hop": {"route": "'in_turn'"},
                  "set_context": {"channel": "hop.chan_raw"}}},
    {"from": "./talky-<key>/collector/assemble", "to": "<RECEPTIONIST_REPLY_TO>",
     "condition": "has(hop.route) && hop.route == 'answer' && !has(hop.round_capped)"},
    {"from": "./talky-<key>/collector/assemble", "to": "<RECEPTIONIST_WRITE_TO>",
     "condition": "has(hop.route) && hop.route == 'write'",
     "modifier": {"set_hop": {"route": "'in_batch'"}}},
    {"from": "./talky-<key>/errors", "to": "<RECEPTIONIST_ERROR_TO>",
     "condition": "has(hop.route) && hop.route == 'error'"}]}}
```

Those are talky's four ports, in talky's own form. A target left empty in `.env`
means the edge is **left out** rather than invented -- and an unwired answer
lane dead-letters, loudly.

**Two keys per channel, on purpose.** `hop.chan` is the sanitised key: it names
the node and it is what the edge condition compares, so no channel identity ever
has to be escaped into a CEL string literal. `hop.chan_raw` is the identity as
the surface knows it, and the ingress modifier promotes THAT to
`context.channel`, so the talky's keeper mints session ids the surface
recognises. A channel that had to be sanitised (`tg:42` -> `tg_42`) keeps an
8-hex digest of the original, so two different channels can never collapse onto
one agent.

## Knobs

All `${VAR:-default}`, environment class, bound late at every read.

| env var | default | meaning |
|---|---|---|
| `RECEPTIONIST_MODEL` | `openai/gpt-4o-mini` | the `ctx.model` handed to every instance. Convention K-H2 (Lane B): put the RESOLVED literal in `.env` |
| `RECEPTIONIST_TEMPLATE` | `talky` | the composite to instantiate; also the instance name prefix |
| `RECEPTIONIST_INGRESS` | `keeper/stamp` | entry port inside the composite |
| `RECEPTIONIST_REPLY_FROM` | `collector/assemble` | the cell emitting answers and write batches |
| `RECEPTIONIST_ERROR_FROM` | `errors` | the composite's error drain |
| `RECEPTIONIST_REPLY_TO` | (empty) | scope-relative answer target; empty = no edge |
| `RECEPTIONIST_WRITE_TO` | (empty) | scope-relative batch target; empty = no edge |
| `RECEPTIONIST_ERROR_TO` | (empty) | scope-relative drain target; empty = no edge |

The five talky-shaped defaults are how another composite plugs in: point them at
its ports and the reception instantiates that instead.

**Every knob the INSTANCES take is theirs, not the reception's.**
`KEEPER_IDLE_MS`, `COLLECTOR_*`, `SUMMARIZER_*` and the rest live in the same
`.env` and reach every instance verbatim -- which also means every channel gets
the same ones. Per-channel tuning is not a knob; it is a different template.

## Two things it deliberately does not do

**Tools.** A freshly instantiated talky has **no tools** -- no tool cells, no
`system.tools` schemas, no `hop.tool_name` lanes. The tool set is the per-agent
choice ([`talky/README.md`](../talky/README.md) § per-instance lanes), and the
reception has no way to know which one a new channel should get. A parent tree
that wants tools on the fleet adds them per instance, after the fact, in its own
mutation. Until then a tool name nobody answers to dead-letters and stalls that
round until `COLLECTOR_ROUND_IDLE_MS` closes it.

**Identity.** No seed, no persona, no `system` write. A seed only takes on a
FRESH birth anyway, and what an agent IS is not topology.

## The verdict is terminal

The colony answers the mutation with `{"mutation": {"outcome": …}}` -- not a UBF
body, on a fresh trace, with an empty context compartment. `greet` therefore
declares `"consumes": {"body": {}}` (any declared key would make the reply
bounce) and treats the verdict as **terminal**: it emits nothing. Recognising a
reply as "some other request" is the reply-fallback loop that spins until the
TTL kills it.

The only reject this lane produces is a lost race, and it needs no repair: the
winner's edge answers the same key.

## What it is not

- **Not a surface.** No proxy, no HTTP ingress, no allowlist. Who may talk to
  the fleet is a condition on the ingress edge, in the parent scope.
- **Not a router.** It draws per-channel edges once; the colony routes.
- **Not a lifecycle.** Nothing is ever removed (No-Delete): a channel that fell
  silent keeps its row and its agent, cold, at the cost of a registry entry and
  a `cell.db`. There is no eviction, no quota and no fairness.
- **Not a migration.** Instances already created keep the topology and the
  `ctx.model` they were born with; changing `RECEPTIONIST_*` moves only the
  channels that arrive AFTER it.

## Pins

`crates/meclaw-cells/tests/receptionist.rs` -- nine script pins on the shipped
`script_inline` (the ledger question, the hand-through, the one mutation and its
four edges, the channel key, the terminal verdict, the unwired port) plus two
colony pins against the mock OpenAI wire: two channels get two talkys from one
template while the second turn of the first channel builds nothing (proved
positively through the mutation audit, not through the registry, because a
second `add_nodes` would be REJECTED and leave the registry looking identical),
and a burst on a cold channel never forks it.
