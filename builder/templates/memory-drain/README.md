# `memory-drain@1.0.0`

The adapter between a closed session and the central memory (GitHub #101).

A collector hands its day out as **one** write batch (`messages[]` = the whole day,
top-level slot `rounds`, `hop.session_id`/`turn_count`/`round_count` — see the C3 receipt).
The memory hive's writer takes **one** turn at a time and writes exactly one `episodes`
row per turn ([hive spec](../memory-hive/README.md) § B.3/D.2). Nothing spoke both forms,
so a closed day never reached memory at all.

This hive is that translation, and it lives **outside** the memory hive on purpose
(ruling R-MD-1): it speaks only the documented `turn-write` port, so **not one line of the
memory hive changes**, the P15 invariance gate stays untouched, and nothing invents a
second write path.

```
<talky>/collector/assemble --route write--> ./drain --route episode--> <memory>/writer
                                              |  ^
                                     route lstore|  |context.drain_origin == 'drain'
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
edges are all internal derives inactive
([cookbook/island-activation.md](../../cookbook/island-activation.md)).

| Port | Direction | Endpoint | The edge must carry |
|---|---|---|---|
| in_batch | in | `./drain` | `condition: has(hop.route) && hop.route == 'write'`, `modifier: {set_hop: {route: "'in_batch'"}, set_context: {session_id: "hop.session_id"}}` |
| episode | out | `./drain` → the memory hive's `turn-write` port | `condition: has(hop.route) && hop.route == 'episode'`, `modifier: {set_context: {session_id: "hop.session_id", turn_id: "hop.turn_id", happened_at: "hop.happened_at"}}` |

```json
{"scope": "/agent", "ctx": {}, "diff": {
  "add_nodes": [{"name": "drain", "template": "memory-drain"}],
  "add_edges": [
    {"from": "./talky/collector/assemble", "to": "./drain/drain",
     "condition": "has(hop.route) && hop.route == 'write'",
     "modifier": {"set_hop": {"route": "'in_batch'"},
                  "set_context": {"session_id": "hop.session_id"}}},
    {"from": "./drain/drain", "to": "./memory/writer",
     "condition": "has(hop.route) && hop.route == 'episode'",
     "modifier": {"set_context": {"session_id": "hop.session_id",
                                  "turn_id": "hop.turn_id",
                                  "happened_at": "hop.happened_at"}}}
  ]}}
```

The `write` route usually already has a consumer (a summarizer, an archive). This is a
**second parent edge on the same route** — supported, and the form the S2 receipt named as
the way to fan a batch out (§ 6, limit 2). The drain adds a lane, it does not take one.

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

## Known limits

- **`happened_at` is usually empty.** The collector's close batch carries no per-turn
  timestamp, so the writer stamps its own clock and event time equals ingest time for a
  drained day. A batch that *does* know its event times (a replay out of a day archive)
  puts `happened_at` (or `recorded_at`) on the turn, and the drain hands it on — the
  bi-temporal split is preserved wherever the information exists at all.
- **The parked day is a copy.** Each drain run parks the day it was handed; a second close
  of the same session parks a second copy. The ledger is a transport buffer, not a record —
  the durable record is the `episodes` table of the memory hive and the window store of the
  talky before it. Pruning the ledger is an operator lane, not built here.
- **The mark is a high-water number, and it assumes an append-only prefix.** That is what
  the collector's close batch is (`select turns where session_id order by id asc`). If a
  session were pruned *between* two drains, the second batch would be a suffix, and both
  the mark and the index-based id would shift with it. Drain before prune.
- **The batch is unbounded**, exactly like the batch it consumes: a very long day is a very
  large message and a very large parked row.
