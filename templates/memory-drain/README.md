# `memory-drain@2.0.5`

The adapter between a write batch and the central memory (GitHub #101).

> **What this adapter is for: bulk import of foreign history — a benchmark
> haystack, an exported transcript from another agent, a month of chat out of an
> archive. Nothing on a live path, and nothing shipped wires it**
> (**ADR 0012**; [GH #298](https://github.com/mmeyerlein/meclaw/issues/298),
> ruling Q11).
> A collector hands out **one message per turn** on its `turn_write` route, with
> `hop.turn_id`, `hop.turn_index` and `hop.happened_at` on it — which is exactly
> what the memory hive's `in_episode` lane reads, so a live turn has nothing left
> for a decomposer to do, and that route is wired straight at the hive. Both
> shipped examples used to put this hive in the middle of it; neither does any
> more. The template keeps every line of its code and its version: what it loses
> is its wires, not its mechanism. The one thing it still owns that nothing else
> in the distribution owns is the **ledger** — a re-delivered transcript is a
> skip, which is the property an interrupted import needs and which neither
> shipped importer has. ADR 0012 decided to keep it on that ground and named the
> trigger that would retract it (`docs/roadmap.md` § W5-Nachtrag): a transfer
> lane that learns raw-transcript import, or a second bulk importer written
> without anybody reaching for this one.

**Where it comes from.** A collector hands its day out as **one** write batch
(`messages[]` = the whole day, top-level slot `rounds`,
`hop.session_id`/`turn_count`/`round_count` — see the C3 receipt). The memory hive's writer
takes **one** turn at a time and writes exactly one `episodes` row per turn (hive spec
§ B.3/D.2 — the memory hive itself is not part of this distribution). Nothing spoke both
forms, so a closed day never reached memory at all. That was the gap of GH #101; the live
half of it closed the other way, by making the conversation itself speak one turn per
message (GH #298, ruling Q11). What is left of the gap is the one place a batch is still
genuinely a batch: **history that arrives from somewhere else**.

This hive is that translation, and it lives **outside** the memory hive on purpose
(ruling R-MD-1): it speaks only the memory hive's documented `in_episode` lane, at the hive
path, so **not one line of the memory hive changes**, the P15 invariance gate stays untouched, and nothing invents a
second write path.

```
<batch producer> --route write--> ./drain --route episode--> <memory> (lane in_episode)
                                     |  ^          \--route reject--> <wherever refusals are read>
                       route lstore  |  |  context.drain_origin == 'drain'
                                     v  |
                                 ./ledger
```

## Two cells

| Cell | Type | Role |
|---|---|---|
| `drain` | `code` | The phase machine: park the day, ask what was drained, fire the rest. |
| `ledger` | `store` | One table, two kinds of row: the parked day and the mark that says how far this session got. |

## Not in scope

**Neither the fact path nor the episode path of a live conversation passes through here any
more** (ADR 0012, GH #298 ruling Q11). Both are written **per turn**: the collector emits one
finished turn per message on `turn_write` straight at the memory hive's `in_episode` lane,
and extraction runs off the episode the hive itself wrote. There is no batch adapter on a
conversational path, and this hive is not one waiting to be plugged back in.

**Nor is it the transfer lane** ([#243](https://github.com/mmeyerlein/meclaw/issues/243)).
That lane moves a hive's **own records** between hives — episodes, facts, judgements — and
re-derives nothing. This adapter turns **foreign raw conversation** into episodes, which is
a derivation with a different guarantee; `templates/memory-hive/porter/config.json`
§ not_in_scope is explicit that the transfer lane does not take that job.

And, unchanged: no extraction, no summarising, no judgement about what is worth remembering;
no LLM call and no provider key (the write path is synchronous and model-free by spec); no
second write path into memory; no session lifecycle of its own; no recall lane — this adapter
only writes; and no provenance of its own — the participant set and the room are carried
through untouched, never minted and never defaulted to `["*"]`.

## Ports

**No shipped topology draws these edges** (GH #298, ruling Q11; ADR 0012). What follows is
the shape a wiring would have to take, not a wiring that exists. The one case it is meant
for is an **out-of-band import**: a tree that receives foreign history as a batch and wants
the ledger's skip on a re-delivery. Never on a conversational path, where `turn_write`
already speaks the `in_episode` lane one turn at a time.

Instantiate `add_nodes` and both port `add_edges` in **one** mutation — an island whose
edges are all internal derives inactive, so a two-step instantiation leaves the drain
dormant (the island-activation rule).

| Port | Direction | Endpoint | The edge must carry |
|---|---|---|---|
| in_batch | in | `./drain` | `condition: has(hop.route) && hop.route == 'write'`, `modifier: {set_hop: {route: "'in_batch'"}, set_context: {session_id: "hop.session_id"}}` — plus `audience_set` and `channel` in `context`, see "The provenance the batch has to carry" |
| episode | out | `./drain` → the memory hive's path, lane `in_episode` | `condition: has(hop.route) && hop.route == 'episode'`, `modifier: {set_hop: {route: "'in_episode'"}, set_context: {session_id: "hop.session_id", turn_id: "hop.turn_id", happened_at: "hop.happened_at"}}` |
| reject | out | `./drain` → wherever refusals are read | `condition: has(hop.route) && hop.route == 'reject'`. **Required**: the template declares the pairing, so a mutation that wires `in_batch` without this edge is refused. |

```json
{"scope": "/main", "ctx": {}, "diff": {
  "add_nodes": [{"name": "drain", "template": "memory-drain"}],
  "add_edges": [
    {"from": "./talky", "to": "./drain",
     "condition": "has(hop.route) && hop.route == 'write'",
     "modifier": {"set_hop": {"route": "'in_batch'"},
                  "set_context": {"session_id": "hop.session_id"}}},
    {"from": "./drain", "to": "./memory",
     "condition": "has(hop.route) && hop.route == 'episode'",
     "modifier": {"set_hop": {"route": "'in_episode'"},
                  "set_context": {"session_id": "hop.session_id",
                                  "turn_id": "hop.turn_id",
                                  "happened_at": "hop.happened_at"}}},
    {"from": "./drain", "to": "./sink",
     "condition": "has(hop.route) && hop.route == 'reject'"}
  ]}}
```

The `write` route usually already has a consumer (a summarizer, an archive). This is a
**second parent edge on the same route** — supported, and the form the S2 receipt named as
the way to fan a batch out (§ 6, limit 2). The drain adds a lane, it does not take one.

**`./drain` is the port contract, and it is the HIVE.** Both the entry and the exit sit on
the hive path itself: the template declares `. -> ./drain` on an in_ lane and `./drain -> .`
on `episode`, and `params.ports` is `[]` — the hive path is the only address (overview,
§ Die Hive-Grenze). Earlier versions of this file wired `./drain/drain`, the cell inside;
that worked, and it also wrote this template's internals into every parent's topology. What
is behind the door may change in a version bump; the address may not — moving it is a breaking change
to every parent that wired it, and it gets a CHANGELOG Breaking entry and a new major
version, not a patch.

**Every hop key is always present**, empty string where it has no value. A `set_context`
whose CEL expression reads an *absent* hop key fails to evaluate, and a failed modifier
makes the colony skip the **whole** edge — the same trap the memory hive documents for its
recall window. `happened_at` in particular is usually empty (see below) and must still be
promoted.

## The provenance the batch has to carry

Since the audience gate (`memory-hive@2.1.0`, GH #244, ADR-0002 E2) a turn is written with
**who was present** or it is not written at all. Two keys carry that, and they are keys of
the `context`, not of the body — a turn cannot assert its own audience:

| Key | Meaning |
|---|---|
| `context.audience_set` | a JSON list of participants in affinity vocabulary (`member:alex`, `agent:scribe`); `["*"]` alone means universal |
| `context.channel` | the room the batch was spoken in |

**This adapter neither mints them nor reads them for their value.** Both ride in `context`
from wherever they were declared, through the ledger round trip, out on every `episode` and
into the hive's writer untouched — context is carried hop by hop, so nothing here has to
copy them and nothing here may rewrite them (E12). What the adapter does is check that they
are **there** before it consumes a batch it could not deliver:

```
in_batch with an audience  -> parked, probed, drained
in_batch without one       -> route reject, hop.reject_reason 'missing_audience'
                              ZERO ledger rows, zero episodes
park/probe refused         -> route reject, hop.reject_reason 'store_refused'
                              (+ hop.store_error, hop.store_operation)
                              nothing left, nothing marked -> redeliver
mark refused               -> route reject, same keys, but the episodes
                              ALREADY left: only the high-water mark is
                              missing, and a redelivery repeats them
```

**The last step is the one to read carefully.** The `mark` insert rides in the SAME
emission as the episodes it covers -- that is what makes "what left" and "what says it
left" one decision instead of two -- so a refusal of the mark arrives *after* the turns
are gone. The reject body names the step for exactly this reason: at `park` and `probe`
nothing has been consumed and a redelivery drains whole, at `mark` the delivery happened
and only the adapter's own idempotence gate is blind. The turn_ids are deterministic, so
the hive sees the repeat; this adapter does not, until a mark lands
([#343](https://github.com/mmeyerlein/meclaw/issues/343)).

**Why the refusal is here and not only in the hive.** The hive refuses correctly and
loudly — one `reject` per turn, with a reason. But it refuses one hop too late: by then
this adapter has already written the `mark` that says the day is through, and the ledger
has no way back. The turns would then be refused **and** recorded as drained, and no later
delivery — not the close batch, not a replay, not a fixed edge — would ever offer them
again. Refusing at the door keeps the batch **undrained**: correct the wiring, deliver the
same day again, and all of it lands.

**`["*"]` is not a default and never will be.** It means readable by everyone in every
later round, which is the one value that cannot be taken back once a row carries it — so
an unknown audience is an error, not an occasion to guess. Same for the room: if it is not
known, it is not known.

**Where they usually come from.** In a colony whose channels sit in the `channels` level of
`assistant@1.0.1` both are already in `context` before the turn ever reaches the talky: the
ingress door of the generation declares `audience_set` (the participant set is a constant
of a generation's lifetime, ADR-0002 E8) and the connector promotes the room to
`context.channel`. Such a tree wires this adapter exactly as it always did and adds
nothing. A tree that holds its channels some other way declares both on the `in_batch` edge
itself.

## The chain

The script keeps no state between hops — so the day has to survive the ledger read it
triggers, which is the same reason the collector's own close lane parks its turns.

```
in_batch  -> insert drain_log(kind 'batch', payload = the day)     phase park
park      -> select drain_log where session_id, order by id asc    phase probe
probe     -> N x ROUTE episode  +  insert drain_log(kind 'mark')   <- ONE multi-send
mark      -> nothing (the chain ends)
```

## The two gates this template exists for (R-MD-2)

1. **Lossless (count gate).** `hop.turn_count` of the batch = the number of new `episodes`
   rows of that session. Every user/assistant text turn becomes exactly one episode, in
   the order of the day; nothing is judged, merged, capped or dropped. The caps of the
   collector bound a *context window*; a batch is not one, and neither is a memory.
2. **Idempotent.** The id an episode travels under is minted **deterministically** from
   `session_id` + turn index: `turn_id = "<session_id>#<index>"`. It is read out of the
   ledger **before** anything is fired (select-before-insert), and an already drained turn
   is **skipped**, not written a second time. The same batch delivered twice leaves the row
   count where it was.

Note what the adapter can and cannot mint: the `episodes.id` uuid is minted **inside** the
writer and is the hive's business. The identity this adapter controls — and the one it
therefore dedups on — is the `turn_id` that travels as an ingress context key and lands in
the `episodes` row. Deterministic there is deterministic where it counts.

## Retracted: "two cadences, one ledger"

Earlier versions of this file described a **second** in_batch edge, on the collector's
`turn_write` route, and called the two "cadences of one document": the per-turn lane as
the ingest, the close lane as the safety net. That is withdrawn
([GH #298](https://github.com/mmeyerlein/meclaw/issues/298), ruling Q11), and it is
withdrawn rather than reworded:

```
collector --route turn_write--> ./drain     <- GONE. Do not wire this.
```

`turn_write` no longer carries "the day so far". It carries **one finished turn per
message**, with a deterministic `hop.turn_id` (`<session_id>#<index>`), `hop.turn_index`
and `hop.happened_at` beside it, and it is addressed at the memory hive's `in_episode`
lane directly. Idempotence for that lane lives in the collector's own `turns` table
(`episode_written`), not here. Putting this adapter in front of it therefore does not add
a gate — it adds a **second minter** over turns the lane already wrote, which is the exact
failure the retracted paragraph warned about, pointing the other way.

**The argument that used to stand here is inverted, not weakened.** It read: the hive's
writer inserts unconditionally and `episodes.turn_id` carries no unique constraint, so
this ledger is the only thing that recognises a turn already home, and a direct per-turn
edge into the writer would mint ids beside it. Both halves are still true about the
*mechanism*; what changed is where the one minter lives. Since GH #298 the collector is
the minter and its `turns` table is the gate, so the drain beside it is the second one.
One minter, one gate — that principle is unchanged, and it is now what keeps this hive
off the live path.

## Known limits

- **`happened_at` is usually empty.** The collector's close batch carries no per-turn
  timestamp, so the writer stamps its own clock and event time equals ingest time for a
  drained day. A batch that *does* know its event times (a replay out of a day archive)
  puts `happened_at` on the turn, and the drain hands it on — the bi-temporal split is
  preserved wherever the information exists at all. `happened_at` is a sanctioned UBF turn
  slot since GH #135; before that the branch described a route the body schema forbade, and
  a replay had to speak to the episode port with the time in the header instead. The
  script's `recorded_at` fallback is a leftover of that period: `TurnObject` does not carry
  that key, so nothing can reach it.
- **A close that nobody was present for is refused, not written.** The participant set is
  a key of the round, and a batch that arrives without one is refused — visibly, on the
  reject lane, with nothing consumed. It stays deliverable: wire the set onto the edge and
  send the same day again.

  This used to catch every session a timer swept closed, because a sweep carries the
  *sweep's* context and not the conversation's. Since `session-keeper@2.0.1` it does not:
  the keeper records the round on the generation row when the conversation OPENS it and
  reads it back off that row at the seal, so `talky@4.2.1`'s close edge has a room and a
  round to promote (GH #273). What remains refused is a generation whose ingress door
  never declared one — including every generation that was already open when that edge was
  wired, because provenance is written once and never rewritten (ADR-0002 E12).
- **The parked day is a copy.** Each drain run parks the day it was handed; a second close
  of the same session parks a second copy. The ledger is a transport buffer, not a record —
  the durable record is the `episodes` table of the memory hive and the window store of the
  talky before it. Pruning the ledger is an operator lane, not built here.
- **The mark is a high-water number, and it assumes an append-only prefix.** That is what
  the collector's close batch is (`select turns where session_id order by id asc`). If a
  session were pruned *between* two drains, the second batch would be a suffix, and both
  the mark and the index-based id would shift with it. Drain before prune.
- **The batch is unbounded**, exactly like the batch it consumes: a very long transcript is
  a very large message and a very large parked row. Bounded by the same operator lane the
  ledger already needs, and by nothing else.
- **Select-before-insert is read-modify-write across two hops.** Two batches of the SAME
  session in flight at once can both probe before either marks, and then both write. One
  batch per session never produces that; a producer that fires the same session twice
  inside the ledger's own round trip does. What it costs is a duplicate episode under an
  id that already exists -- so a tree that expects bursts on one session wants the
  guarded-mark variant of this chain, which is a change to this template and not to its
  wiring.
