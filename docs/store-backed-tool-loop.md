# Store-backed tool-loop protocol

An `llm` cell makes one provider call. It does not remember the conversation and it does not
wait for tools. A multi-tool loop therefore needs application topology that can:

1. fan tool calls out in parallel,
2. remember which results belong to the current inference round,
3. wait until every expected result has arrived,
4. rebuild the conversation, and
5. route that conversation into a fresh `llm` call exactly once.

[`examples/telegram-research`](../examples/telegram-research/) is the worked example in this
guide. Its `prep`, `dispatch`, and `collector` nodes are ordinary `code` cells. The durable
thread lives in the `memory` store. The loop itself is the `collector -> planner` edge.

## The four roles

| Role | Example node | Responsibility |
|---|---|---|
| Ingress | `/prep` | Mint a `turn_id`, persist the user turn, and send the initial thread to the planner. |
| Dispatcher | `/dispatch` | Preserve the assistant tool-call turn for fan-in, then emit one message per tool call. |
| Store | `/memory` | Persist user, assistant, and tool rows. Execute the collector's insert, select, and guarded update operations. |
| Collector | `/collector` | Derive expected and received tool-call IDs, claim the completed round, rebuild the thread, and emit it on the `fire` lane. |

The `planner`, `searcher`, and `reader` do not participate in the fan-in protocol. They only
produce or consume ordinary UBF turns. This keeps correlation out of the cell implementations.

## State carried by the topology

The loop uses the `thread` table declared in
[`main/memory/config.json`](../examples/telegram-research/main/memory/config.json):

| Column | Meaning |
|---|---|
| `turn_id` | Correlates every row for one inbound user question. |
| `iter` | Identifies one planner/tool round within that question. |
| `role` | `user`, `assistant`, or `tool`. |
| `turn` | JSON-serialized UBF turn, or an array of assistant tool-call turns. |
| `fired` | `0` on a pending assistant row and `1` after one collector claims the completed round. |

Four context keys connect the store replies to the right collector state:

- `turn_id` persists for the complete question.
- `iter` starts at `0` and changes only on the loopback edge.
- `store_origin` distinguishes collector operations from unrelated store traffic.
- `firing` marks the select that is allowed to rebuild and fire the thread.

The store reports the completed operation in hop data such as `operation` and
`rows_affected`. Those values describe one store reply, so they remain hop-local.

## One round with two tools

Assume the user asks a question and the planner requests two tools with IDs `call-search` and
`call-read`. The scheduler may deliver either result first.

### 1. Persist the user turn

`prep` creates a new `turn_id`, writes the user turn at iteration `0`, and independently sends
the same turn plus persona and tool schemas to `planner`:

```text
turn_id  iter  role  fired  turn
t-7      0     user  null   {"origin":"user","type":"text",...}
```

The insert reply can return to `prep` through the store's reply path. `prep` accepts only a
user text turn, so that reply produces an empty multi-send and stops.

### 2. Fan out calls and record the expectation set

When `planner` finishes with `tool_calls`, the edge routes its output to `dispatch`.
`dispatch` emits:

- one `c_asst` message containing both original tool-call turns, and
- one message for each tool, selected by `hop.tool_name`.

The lane edges guard the key they discriminate on:

```json
{ "from": "./dispatch", "to": "./collector",
  "condition": "has(hop.route) && hop.route == 'c_asst'" },
{ "from": "./dispatch", "to": "./searcher",
  "condition": "has(hop.tool_name) && hop.tool_name == 'web_search'" },
{ "from": "./dispatch", "to": "./reader",
  "condition": "has(hop.tool_name) && hop.tool_name == 'web_fetch'" }
```

The `has()` is not decoration. `hop` is single-hop, so most messages passing a fan-out carry no
`tool_name` at all: the `c_asst` emission, every store reply, every collector emission. A bare
`hop.tool_name == 'web_search'` does not evaluate to `false` on those, it **errors** — CEL
standard semantics — and the substrate skips the edge. Routing is right either way; the
difference is one log line per non-matching lane per message, which at eight lanes is most of the
log. Since GH #80 the substrate logs that class at `debug` instead of `warn`, and the guarded
form above produces no line at all. Apply it to every condition that reads an optional `hop` key;
`context.*` keys, which are carried along, do not need it.

The collector stores the complete assistant turn in one row:

```text
t-7      0     assistant  0  [{"id":"call-search",...},{"id":"call-read",...}]
```

Keeping the assistant calls together matters. When the thread is rebuilt, the provider sees
one assistant message that requested both tools, followed by their results.

### 3. Persist results in arrival order

`searcher` and `reader` run independently. Each result reaches the collector on the `c_res`
lane and becomes a tool row with the same `turn_id` and `iter`:

```text
t-7      0     tool  null  {"id":"call-read","type":"tool_result",...}
t-7      0     tool  null  {"id":"call-search","type":"tool_result",...}
```

After every insert reply, the collector selects all rows for `t-7`. It examines only rows from
the current iteration and derives two sets:

```text
expected = {call-search, call-read}
received = {call-read, call-search}
complete = expected is non-empty and expected is a subset of received
```

The comparison uses IDs rather than row counts. Arrival order cannot change the result, and a
duplicate ID cannot make a missing result appear complete.

### 4. Claim the completed round once

Several insert/select chains can observe a complete round at nearly the same time. A plain
"complete, then fire" check would send the same thread back to the planner more than once.

Instead, each contender asks the store for the same guarded update:

```json
{
  "operation": "update",
  "table": "thread",
  "set": { "fired": 1 },
  "where": {
    "turn_id": "t-7",
    "iter": 0,
    "role": "assistant",
    "fired": 0
  }
}
```

The store serializes its own operations. Exactly one update changes the assistant row and
reports `rows_affected == 1`. Every loser reports `0` and parks by emitting an empty
multi-send. The winner issues one final select with `context.firing == "1"`.

The `fired` column is therefore not a completion flag. It is a compare-and-set guard that
grants one collector path permission to cross the loopback edge.

### 5. Rebuild and re-enter

On the firing select, the collector sorts rows by iteration and role, parses each `turn`, and
emits the cumulative `messages[]` on its `fire` lane. For iteration `0`, the order is:

```text
user -> assistant tool calls -> tool results
```

The edge from `collector` to `planner` performs the state transition:

```json
{
  "condition": "has(hop.route) && hop.route == 'fire'",
  "modifier": {
    "set_context": {
      "iter": "int(context.iter) + 1",
      "firing": "''"
    }
  }
}
```

No cell increments the counter and no cell calls the planner directly. The graph owns both
actions.

If the next planner call emits more tool calls, their rows use iteration `1` and the same
protocol repeats. If it finishes with `stop`, separate edges send the answer to the Telegram
proxy and to `archive`; the tool loop is done.

## A tool result is re-sent on every subsequent round

The thread is rebuilt cumulatively (step 5), so a tool result does not enter the model's context
once. It enters again on every round of the same turn. One 172 KB fetch in a two-round turn was
measured at roughly 70k prompt tokens; in a five-round turn the same fetch is carried five times.

Two places bound that, and they are different decisions:

- **At the tool.** `web_fetch` takes `params.max_bytes` (default 256 KiB, GH #83) and marks a trim
  in the payload (`… [truncated, N bytes total]`, `header.truncated: true`, `header.bytes` = the
  full size). `bash` shares the same knob and default for runaway stdout, and `web_search` trims
  its result list at `params.max_results` (default 10, visible inside the JSON) with the same byte
  backstop. Inside a loop those values belong much lower —
  [`examples/telegram-research`](../examples/telegram-research/) sets 32 KiB on its `reader`. A cap
  is a bound on the worst case, not a policy.
- **At the collector.** What leaves the assembled context again is the collector's decision, and it
  is the one that turns a large result from a per-round cost back into a one-time cost. The shape
  is deterministic policy rather than a model judgement: whole turns leave on a turn cap and a byte
  cap, never halves, and the turn being answered is never the one evicted. An eviction rule over
  the tool rows of the round slate is that same shape one level down, and it is tracked on GH #83.

For a genuinely large document the honest pattern is not a cap at all: fetch it to a file with a
`file` cell and hand the model the path, so the payload never becomes a thread row.

## The TTL budget of one round

`ttl` is the routing-loop guard: colony decrements it on every routing decision and a message
that reaches `0` is dead-lettered. One user-visible tool round in this shape is **not** one hop.
It is about a dozen, because the collector's read-modify-write conversation with the store is
itself routing:

| Leg | Hops |
|---|---|
| planner -> dispatcher | 1 |
| dispatcher -> tool | 1 |
| tool -> collector | 1 |
| collector -> store insert, and the reply back | 2 |
| collector -> store select, and the reply back | 2 |
| collector -> store guarded update, and the reply back | 2 |
| collector -> store firing select, and the reply back | 2 |
| collector -> planner (the loopback edge) | 1 |
| **one round** | **~12** |

Parallel tool calls do not multiply this: `ttl` lives on each message envelope, so the branches
burn their own copies and the number above is the cost along the chain that re-enters the
planner.

Measured on the checked-in fixture (`tests/fixtures/14b-tool-loop-store`, pinned in
`crates/meclaw-cells/tests/tool_loop_ttl_budget.rs`): six tool rounds end to end cost **76**
routing hops. `message_default_ttl` defaults to **64**, so the default budget holds **five**
rounds and the sixth runs out. Five rounds is not generous for an assistant — "write the file,
read it back, fix it, verify, then summarise" is five.

### The recommended form: let the loopback edge restore the budget

Raising the colony-wide budget pays for every round of every turn up front, and it makes the
number of rounds an agent may take a property of `colony.json` rather than of the loop. The
loop can instead pay per round. An edge may declare that it restores the routing budget of the
message it takes (GH #82, ruling 2026-08-13):

```json
{
  "from": "./collector",
  "to": "./planner",
  "condition": "hop.route == 'fire' && int(context.iter) < 12",
  "modifier": {
    "set_context": { "iter": "int(context.iter) + 1", "firing": "''" },
    "restore_ttl": true
  }
}
```

That is one edge: the re-entry edge, carrying the iteration counter and the restore together.
When it takes a message, colony lifts the follow-up's `ttl` back to `message_default_ttl`. The
loop then only ever has to fit **one** round into the budget instead of all of them, so
[`examples/telegram-research`](../examples/telegram-research/) needs no `colony.json` at all:
six rounds and more run on the substrate default of 64. Pinned in
`crates/meclaw-cells/tests/tool_loop_ttl_budget.rs`
(`six_tool_rounds_complete_on_the_default_budget_when_the_loopback_edge_restores_ttl`).

What the restore is, precisely:

- **A reset, not a grant.** `ttl` becomes the budget, never `ttl + budget`. Six restores and
  one restore leave the same ceiling, so a restoring cycle can never inflate its own budget
  (pinned: `a_restoring_loopback_edge_never_lifts_ttl_above_the_initial_budget`).
- **Never a demotion.** A message ingested with a larger budget (the `ttl` field of
  `POST /messages`) keeps what it has.
- **Not a hole in the guard, a move of it.** A restoring edge declares its loop legitimate, so
  the runaway guard for that loop is the iteration bound in its `condition`, not TTL. That is
  why a restoring edge **without** a condition is refused at config load and at `add_edges`
  validation instead of booting a colony that can spin forever. TTL keeps guarding everything
  that did not opt in, and the substrate default stays 64.

### Sizing the budget instead, for shapes without the modifier

A shape whose re-entry edge does not restore still has to fit its whole run into one budget.
Size it on purpose, in `colony.json`:

```json
{ "schema_version": 1, "message_default_ttl": 160 }
```

Rule of thumb: `message_default_ttl >= 4 + rounds * 12` for a store-backed loop. The `160` above
buys twelve rounds. Per initial message the HTTP ingress accepts a `ttl` field that overrides it.

### TTL exhaustion is a silent stall, so bound the loop yourself

TTL expiry is **terminal**: the message goes directly to the dead-letter queue and deliberately
does **not** take the `reply_to` cascade (`meclaw-overview.md` § TTL semantics). Inside a fan-in
that is invisible from the agent surface: the collector's fan-in never completes, so it parks by
design, and **nothing is emitted toward the origin** — no answer, no error, nothing a topology
can route on. The colony logs the death loudly (an `ERROR` line naming the message id, its
target, and the trace id) and writes a `ttl_expired` dead-letter row; those are the operator's
signals, and they are the only ones.

So TTL is a substrate guard, not the loop's bound. Bound the loop where the loop lives — on the
loopback edge, with the iteration counter the edge already owns:

```json
{
  "from": "./collector",
  "to": "./planner",
  "condition": "has(hop.route) && hop.route == 'fire' && int(context.iter) < 12",
  "modifier": {
    "set_context": { "iter": "int(context.iter) + 1", "firing": "''" },
    "restore_ttl": true
  }
}
```

With `restore_ttl` that bound is not optional but mandatory: it is the only thing left
that stops the loop, which is exactly why the substrate refuses an unconditional restoring edge.

A second edge with the inverse condition gives the runaway round a destination that answers
(an apology turn, an error lane, a notifier) instead of a silence.

## Reading the collector script

The branches in
[`main/collector/config.json`](../examples/telegram-research/main/collector/config.json) map
directly to the protocol:

| Input | Collector action |
|---|---|
| `hop.route == c_asst` | Insert the pending assistant row with `fired: 0`. |
| `hop.route == c_res` | Insert one tool-result row. |
| store reply `operation == insert` | Select the thread rows for this `turn_id`. |
| store reply `operation == select`, not firing | Compare expected and received IDs; guarded-update if complete. |
| store reply `operation == update` | Final-select only when `rows_affected == 1`; otherwise park. |
| store reply `operation == select`, firing | Rebuild `messages[]` and emit `fire`. |

This is a read-modify-write protocol built from messages. The code cell has no hidden state;
every fact needed after a reply is either in the store or in message context.

## Adapting the pattern

To build another store-backed loop:

1. Give each inbound request a stable correlation ID.
2. Persist the user turn before the first inference.
3. Preserve the complete assistant tool-call turn before dispatching individual calls.
4. Return every tool result with its original tool-call ID.
5. Test completeness by ID membership, scoped to the current correlation ID and iteration.
6. Put the one-shot guard in the store operation, not in code-cell memory.
7. Rebuild the provider thread only after winning that guard.
8. Increment the iteration and route to the LLM on an edge.
9. Make store replies and losing races terminate explicitly with an empty multi-send.

Validate the worked example without external credentials:

```bash
cargo run --bin meclaw -- --root ./examples/telegram-research --validate
```

Running the live example additionally requires the Telegram, LLM, and search credentials
documented in its README.
