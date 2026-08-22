# `receptionist@2.0.3`

One agent per channel, built the moment a channel first speaks. Two cells under
one hive: `greet` (a `code` cell) and `ledger` (a `store`). No new cell type, no
Rust.

**A shared communicator flattens every surface it serves** (GH #29). Chat
etiquette, formatting and pacing differ per channel, and one `talky` in front of
many chats mixes their windows into one. The receptionist is the front door that
keeps them apart: the first turn of a channel nobody has met makes it
instantiate a fresh [`talky`](../talky/) for exactly that channel -- **one**
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
step, the channel, the round and the parked turn to context), and
`ledger -> greet` back on `context.rec_origin == 'greet'`.

## Lanes

`params.ports` is empty (GH #228): the address is the hive, the lane is `hop.route`.
Two edges, and **the parent draws both**.

| lane | direction | note |
|---|---|---|
| `in_turn` | in | the first turn of a channel. The edge promotes `context.channel` and `context.audience_set`, both mandatory |
| `mutate` | out | the tree this reception decided to grow, on to `/colony/mutations` -- **bootstrap only**, see below |
| `turn` | out | the turn itself, on the lane its own agent answers to |
| `reject` | out | the channel ledger did not answer a step of this reception (`hop.reject_reason` `store_refused`, the ledger's own code in `hop.store_error`, the refused op in `hop.store_operation` -- [#343](https://github.com/mmeyerlein/meclaw/issues/343)). The body names the **step**: at `look` nothing is built and no row is written -- an unanswered lookup has the same shape as a channel nobody has met, and read as the second it grew a **second** agent for a channel that already had one. At `open` the agent was already built and the turn already left in the same emission, so only the ledger row is missing, and every later turn on that channel repeats the instantiation -- refused each time as a name collision |

```json
{"from": "<surface>", "to": "./reception",
 "condition": "has(hop.route) && hop.route == 'turn'",
 "modifier": {"set_hop": {"route": "'in_turn'"},
              "set_context": {"channel": "hop.chat_id",
                              "audience_set": "'[\"member:alex\",\"agent:scribe\"]'"}}},
{"from": "./reception", "to": "/colony/mutations",
 "condition": "has(hop.route) && hop.route == 'mutate'"}
```

**The mutation lane is bootstrap-only, and that is the point.** No mutation can
create an edge to `/colony/mutations` at any scope -- `validate_scope_containment`
answers `scope_out_of_bounds`, and a template's own `params.graph` is
containment-checked against its subtree root, so the edge cannot travel inside
this template either. (`/colony` is not blanket-forbidden any more: since
[#163](https://github.com/mmeyerlein/meclaw/issues/163) exactly one endpoint,
the read-only `/colony/graph`, *is* drawable by a mutation --
`MUTATION_DRAWABLE_VIRTUAL_ENDPOINTS` in
`crates/meclaw-colony/src/mutation/mod.rs`, deliberately a single name and not a
prefix. `/colony/mutations` is authority transfer and stays out.) It exists exactly if an operator
wrote it into a `config.json` before boot, and no topology can grant itself the
lane afterwards (the colony-endpoint roundtrip rule 4; the `builder-hive`
topology has the same shape).

Consequence for the gate runbook: a receptionist instantiated by `add_nodes`
registers and activates, but its mutation lane is **inert** until an operator
adds the edge and reboots. A receptionist belongs in the bootstrap tree.

**The channel promotion is not optional.** Without `set_context: {"channel": ...}`
every chat of the colony collapses onto the channel `default` -- which is one
talky for everybody, exactly the thing this template exists to prevent. Whatever
the surface calls "the same conversation partner" goes in there: a
Telegram/Slack `chat_id`, a room, a phone number.

**The round is the caller's too, and for a reason this template cannot get
around.** The reception knows a channel, and a channel is a room -- `tg:42`, a
phone number, a Slack conversation. A participant is a different thing
(`member:alex`, `agent:scribe`), and turning the first into the second would be
inventing a person nobody named. So the round travels the same way the channel
does: the caller's ingress edge declares `context.audience_set` as a JSON list in
affinity vocabulary, and the reception hands it to the edge it draws into the
composite. It derives nothing, defaults nothing, and never writes `["*"]`.

Leave it out and every generation this reception builds closes **untagged**: the
keeper writes an empty round onto the generation row at the open (ADR-0002 E8 --
the round is a constant of the generation, and E12 -- provenance is never
rewritten), the night sweep carries that empty round out on the `write` port, and
a [`memory-drain`](../memory-drain/) on that port refuses the day with
`missing_audience` rather than storing a row that claims everyone was present.
Nothing is lost and nothing is guessed; the day simply does not land. Wiring the
key afterwards takes effect on the NEXT generation of a channel, never on one
that is already open.

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
    {"from": "./reception", "to": "./talky-<key>",
     "condition": "has(hop.route) && hop.route == 'turn' && has(hop.chan) && hop.chan == '<key>'",
     "modifier": {"set_hop": {"route": "'in_turn'"},
                  "set_context": {"channel": "hop.chan_raw",
                                  "audience_set": "hop.aud"}}},
    {"from": "./talky-<key>", "to": "<RECEPTIONIST_REPLY_TO>",
     "condition": "has(hop.route) && hop.route == 'answer' && !has(hop.round_capped) && !has(hop.degraded)"},
    {"from": "./talky-<key>", "to": "<RECEPTIONIST_WRITE_TO>",
     "condition": "has(hop.route) && hop.route == 'write'",
     "modifier": {"set_hop": {"route": "'in_batch'"}}},
    {"from": "./talky-<key>", "to": "<RECEPTIONIST_ERROR_TO>",
     "condition": "has(hop.route) && hop.route == 'error'"}]}}
```

Those are talky's four essential lanes, at talky's own path -- since `talky@3` there is no cell inside it a mutation could name. A target left empty in `.env`
means the edge is **left out** rather than invented -- and an unwired answer
lane dead-letters, loudly.

**The reply guard names two keys, not one.** Three sorts travel talky's `answer`
lane and only one of them is an answer: a real one, a round that hit `max_iter`
(`hop.round_capped`), and -- since `collector@2.1.1` -- a turn the store would
not let be assembled (`hop.degraded`, [#343](https://github.com/mmeyerlein/meclaw/issues/343)).
The third carries **no** `round_capped`, so a guard against `round_capped` alone
lets it through and a store refusal is read out to a person as a real reply.
Neither of the two goes to the reply sink; both dead-letter, which is where this
reception already leaves the capped sort. A parent that wants to render them
differently draws its own edge on `has(hop.degraded)`.

**Three keys travel, and the third is the round.** `hop.aud` is the round the
caller declared, carried through the ledger round trip as `context.rec_aud` and
put back on the hop -- so the ingress edge the reception draws **declares** it
rather than hoping the context survives the hops. That distinction is the whole
of GH #274: a value that arrives because nothing deleted it is not a promise, and
the next version of any cell on the path may stop carrying it. `hop.aud` is
always present and empty when the door declared nothing, because a missing hop
key makes a CEL modifier fail and a failed modifier skips the edge -- a turn that
cannot name its round would vanish instead of being refused downstream.

**Two keys per channel, on purpose.** `hop.chan` is the sanitised key: it names
the node and it is what the edge condition compares, so no channel identity ever
has to be escaped into a CEL string literal. `hop.chan_raw` is the identity as
the surface knows it, and the ingress modifier promotes THAT to
`context.channel`, so the talky's keeper mints session ids the surface
recognises. A channel that had to be sanitised (`tg:42` -> `tg_42`) keeps an
8-hex digest of the original, so two different channels can never collapse onto
one agent.

## Knobs

**Env knobs are an experimental surface.** Until this template's knobs move onto the `params`
block of the cells that read them, their names carry no compatibility promise and may change in
any `0.x` release; provider credentials keep living in `.env` either way. The migration is
tracked in [#138](https://github.com/mmeyerlein/meclaw/issues/138), with the
`collector@1.2.0` migration ([#136](https://github.com/mmeyerlein/meclaw/issues/136)) as the
reference pattern.

All `${VAR:-default}`, environment class, bound late at every read.

| env var | default | meaning |
|---|---|---|
| `RECEPTIONIST_MODEL` | `openai/gpt-4o-mini` | the `ctx.model` handed to every instance. Convention K-H2 (Lane B): put the RESOLVED literal in `.env` |
| `RECEPTIONIST_TEMPLATE` | `talky` | the composite to instantiate; also the instance name prefix |
| `RECEPTIONIST_INGRESS` | *(empty)* | where the turn enters the composite, relative to the instance root. Empty is the instance path itself -- what a sealed composite wants, since the lane and not the path selects the cell |
| `RECEPTIONIST_REPLY_FROM` | *(empty)* | where answers and write batches leave the composite. Empty is the instance path itself |
| `RECEPTIONIST_ERROR_FROM` | *(empty)* | where the error lane leaves the composite. Empty is the instance path itself |
| `RECEPTIONIST_REPLY_TO` | (empty) | scope-relative answer target; empty = no edge |
| `RECEPTIONIST_WRITE_TO` | (empty) | scope-relative batch target; empty = no edge |
| `RECEPTIONIST_ERROR_TO` | (empty) | scope-relative drain target; empty = no edge |

The five talky-shaped defaults are how another composite plugs in: point them at
its ports and the reception instantiates that instead.

**Every knob the INSTANCES take is theirs, not the reception's.** The env-class ones --
`KEEPER_IDLE_MS`, `SUMMARIZER_*`, `DISPATCHER_*` -- live in the same `.env` and reach every
instance verbatim, which also means every channel gets the same ones. The collector's knobs
are params since `collector@1.2.0` and could in principle differ per channel, but this
template writes the same mutation for every new channel and therefore ships the same
defaults to all of them. Per-channel tuning is not a knob of the reception; it is a
follow-up mutation on the instance, or a different template.

## Two things it deliberately does not do

**Tools.** A freshly instantiated talky has **no tools** -- no tool cells, no
`system.tools` schemas, no `hop.tool_name` lanes. The tool set is the per-agent
choice ([`talky/README.md`](../talky/README.md) § per-instance lanes), and the
reception has no way to know which one a new channel should get. A parent tree
that wants tools on the fleet adds them per instance, after the fact, in its own
mutation. Until then a tool name nobody answers to dead-letters and stalls that
round until `round_idle_ms` closes it.

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

`crates/meclaw-cells/tests/gh274_a_receptionist_built_talky_closes_tagged.rs` --
the whole path in one run: the reception builds a talky, the keeper's own night
timer (wound down to a second) sweeps the day shut, and the episode rows land in
a memory hive carrying the round the conversation was spoken in. The edge the
mutation drew is read back out of the live colony in the same test, so the pin
covers the promise as well as the landing.
