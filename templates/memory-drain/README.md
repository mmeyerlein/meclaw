# `memory-drain@2.0.0`

The adapter between a closed session and the central memory (GitHub #101).

A collector hands its day out as **one** write batch (`messages[]` = the whole day,
top-level slot `rounds`, `hop.session_id`/`turn_count`/`round_count` — see the C3 receipt).
The memory hive's writer takes **one** turn at a time and writes exactly one `episodes`
row per turn (hive spec § B.3/D.2 — the memory hive itself is not part of this
distribution). Nothing spoke both forms,
so a closed day never reached memory at all.

This hive is that translation, and it lives **outside** the memory hive on purpose
(ruling R-MD-1): it speaks only the documented `turn-write` port, so **not one line of the
memory hive changes**, the P15 invariance gate stays untouched, and nothing invents a
second write path.

```
<talky>/collector --route write--> ./drain --route episode--> <memory>/writer
                                     |  ^
                       route lstore  |  |  context.drain_origin == 'drain'
                                     v  |
                                 ./ledger
```

## Two cells

| Cell | Type | Role |
|---|---|---|
| `drain` | `code` | The phase machine: park the day, ask what was drained, fire the rest. |
| `ledger` | `store` | One table, two kinds of row: the parked day and the mark that says how far this session got. |

## Ports

Instantiate `add_nodes` and both port `add_edges` in **one** mutation — an island whose
edges are all internal derives inactive, so a two-step instantiation leaves the drain
dormant (the island-activation rule).

| Port | Direction | Endpoint | The edge must carry |
|---|---|---|---|
| in_batch | in | `./drain` | `condition: has(hop.route) && hop.route == 'write'`, `modifier: {set_hop: {route: "'in_batch'"}, set_context: {session_id: "hop.session_id"}}` |
| in_batch (per turn) | in | `./drain` | the same entry, a second edge, for a collector with `turn_write` set: `condition: … hop.route == 'turn_write'`, same modifier. See "Two cadences, one ledger". |
| episode | out | `./drain` → the memory hive's `turn-write` port | `condition: has(hop.route) && hop.route == 'episode'`, `modifier: {set_context: {session_id: "hop.session_id", turn_id: "hop.turn_id", happened_at: "hop.happened_at"}}` |

```json
{"scope": "/agent", "ctx": {}, "diff": {
  "add_nodes": [{"name": "drain", "template": "memory-drain"}],
  "add_edges": [
    {"from": "./talky", "to": "./drain",
     "condition": "has(hop.route) && hop.route == 'write'",
     "modifier": {"set_hop": {"route": "'in_batch'"},
                  "set_context": {"session_id": "hop.session_id"}}},
    {"from": "./drain", "to": "./memory/writer",
     "condition": "has(hop.route) && hop.route == 'episode'",
     "modifier": {"set_context": {"session_id": "hop.session_id",
                                  "turn_id": "hop.turn_id",
                                  "happened_at": "hop.happened_at"}}}
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

## Two cadences, one ledger

A close is a cadence, not a contract. The batch this adapter reads is "the session so
far", and the collector can hand that out at the close **or** after every stored turn
(`turn_write`, route `turn_write`). Wire both edges into this same entry and
the roles fall out by themselves:

```
collector --route turn_write--> ./drain    (per turn: the day so far)
collector --route write------> ./drain     (at the close: the same day, once more)
```

- **The per-turn lane is the ingest.** An episode then exists at the turn instead of at
  the night close, and that is the whole difference between a memory that can answer a
  question about the last exchange and one that cannot.
- **The close lane is the safety net.** Nothing about it changes; it simply usually finds
  nothing to do. The mark says the day is through, the probe skips, the chain ends after
  its `select`.
- **The count gate becomes a completeness proof.** It used to say "the batch arrived
  whole". Now it says "**the per-turn lane lost no turn**": `hop.turn_count` of the close
  batch equals the episodes of that session, and a close drain that writes **zero** is
  the success signal, not a broken one. What the per-turn lane did miss -- a restart, a
  lane switched on mid-session -- the close writes, and only that.

**Why through the drain and not straight into the hive.** The hive's writer inserts
unconditionally and `episodes.turn_id` carries no unique constraint: this ledger is the
*only* thing that recognises a turn that is already home. A second, direct per-turn edge
into `writer` would mint its own ids beside these, and the close drain -- which knows
nothing of it -- would write the whole day a second time. One minter, one ledger. That is
also why the `turn_id` formula stays where it is: it is not repeated anywhere, it is
called twice.

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
- **The parked day is a copy.** Each drain run parks the day it was handed; a second close
  of the same session parks a second copy. The ledger is a transport buffer, not a record —
  the durable record is the `episodes` table of the memory hive and the window store of the
  talky before it. Pruning the ledger is an operator lane, not built here.
- **The mark is a high-water number, and it assumes an append-only prefix.** That is what
  the collector's close batch is (`select turns where session_id order by id asc`). If a
  session were pruned *between* two drains, the second batch would be a suffix, and both
  the mark and the index-based id would shift with it. Drain before prune.
- **The batch is unbounded**, exactly like the batch it consumes: a very long day is a very
  large message and a very large parked row. Per-turn cadence multiplies that: each turn
  parks the day it was handed, so a day of *n* turns leaves *n* parked copies. Bounded by
  the same operator lane the ledger already needs, and by nothing else.
- **Select-before-insert is read-modify-write across two hops.** Two batches of the SAME
  session in flight at once can both probe before either marks, and then both write. A
  close cadence never produced that (one close per session); a per-turn cadence can, if
  two turns of one session arrive inside the ledger's own round trip. What it costs is a
  duplicate episode under an id that already exists -- so a tree that expects bursts on
  one session wants the guarded-mark variant of this chain, which is a change to this
  template and not to its wiring.
