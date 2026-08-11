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
  "condition": "hop.route == 'fire'",
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
