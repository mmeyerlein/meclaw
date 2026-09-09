# Store-backed tool-loop protocol

A worked pattern: one `llm` cell, several tools running in parallel, and a store that decides
when a round is complete. It is topology, and no cell type does any of it.

Written for anyone building a multi-tool agent on meclaw.
[`examples/telegram-research`](../examples/telegram-research/) is the tree described here. Read
§ One round with two tools first; compaction and TTL matter once turns get long. One round
traced against a live provider is in
[`never-forgets/WALKTHROUGH.md`](../examples/never-forgets/WALKTHROUGH.md) § Step 7.

The primitives this composes are on [`meclaw.md`](meclaw.md).

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

Every condition that reads an optional `hop` key needs that `has()`, because `hop` is
single-hop and most messages passing a fan-out carry no `tool_name` at all. The reason a bare
comparison still routes correctly, and why it costs a log line per non-matching lane, is
CEL error semantics plus the GH #80 log level: `meclaw-overview.md` § *Edge expression
language*. Keys under `context.*` are carried along and need no guard.

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

The comparison is over IDs; row counts are never used. Arrival order cannot change the result,
and a duplicate ID cannot make a missing result appear complete.

A tool result is its `messages[]`, every `tool_result` turn of it, each under the `id` of the
call it answers, and nothing else. One result may therefore answer several calls at once,
which is what a batching tool does when it receives the whole call bundle in one message;
every id in it enters `received`. A `system` slot or a top-level body slot on that lane is
dropped on the way in. What leaves the collector's seam in `system.*` is upserted into the
brain cell's own `cell.db` and stands in the prompt until something overwrites the same slot
path, so it is durable state of the agent and no evidence of one round. Only something
re-sent under a fixed path on every turn, the memory bundle, belongs there. A tool with
structure to hand back serialises it into the text of its result.

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

The `fired` column is a compare-and-set guard, not a completion flag: it grants one collector
path permission to cross the loopback edge.

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

The thread is rebuilt cumulatively (step 5), so a tool result enters the model's context again
on every round of the same turn. One 172 KB fetch in a two-round turn was measured at roughly
70k prompt tokens (CHANGELOG 0.4.0, GH #83); in a five-round turn the same fetch is carried
five times.

Two places bound that, and they are different decisions.

At the tool, `web_fetch` takes `params.max_bytes` (default 256 KiB, GH #83) and marks a trim
in the payload (`… [truncated, N bytes total]`, `header.truncated: true`, `header.bytes` = the
full size). `bash` shares the same knob and default for runaway stdout, and `web_search` trims
its result list at `params.max_results` (default 10, visible inside the JSON) with the same byte
backstop. Inside a loop those values belong much lower:
[`examples/telegram-research`](../examples/telegram-research/) sets 32 KiB on its `reader`. A cap
bounds the worst case; it does not express a policy.

At the collector, what leaves the assembled context again is its decision, and that decision
is the one that turns a large result from a per-round cost back into a one-time cost. The shape
is deterministic policy and no model judgement: whole turns leave on a turn cap and a byte cap,
never halves, and the turn being answered is never the one evicted. An eviction rule over the
tool rows of the round slate is that same shape one level down, and that rule is not built.

For a genuinely large document the honest pattern is no cap at all: fetch it to a file with a
`file` cell and hand the model the path, so the payload never becomes a thread row.

## Compacting the thread when the window fills

A cap bounds one item; an eviction drops a whole turn. Neither turns twelve rounds into a
paragraph, so a long turn's window keeps growing until the provider refuses it. The missing
capability is condensation: fold the old rounds into one summary row and rebuild from
`summary + tail`.

This is topology and needs no new cell type. The reference tree is
[`tests/fixtures/gh120-compaction-lane`](../tests/fixtures/gh120-compaction-lane/), the loop
above plus one hive and one edge split, pinned in
`crates/meclaw-cells/tests/compaction_lane.rs`.

### The accumulator is an edge, not a cell

`tokens_prompt` is an `llm` hop key, and a `hop` expires on the next cell emission. The one place
it is still readable is the edge that leaves the brain, so that edge keeps the running total:

```json
{ "from": "./llm", "to": "./tool-loop",
  "condition": "hop.finish_reason == 'tool_calls'",
  "modifier": { "set_context": {
    "tokens_seen": "int(context.tokens_seen) + int(hop.tokens_prompt)" } } }
```

No cell counts anything, and the number rides in `context` next to `iter`, in the same
compartment, owned by the same layer.

### The threshold splits the re-entry edge in two

The loopback edge of step 5 becomes two edges that partition the `fire` lane. The second
condition is the exact negation of the first, because a lane that is not exhaustive parks the
turn: the collector has already spent its fire guard and nothing else will emit.

```json
{ "from": "./collector", "to": "/compact/prep",
  "condition": "hop.route == 'fire' && int(context.tokens_seen) >= 40 && int(context.iter) >= 1",
  "modifier": { "set_context": { "tokens_seen": "0", "compacting": "'1'" } } },
{ "from": "./collector", "to": "/llm",
  "condition": "hop.route == 'fire' && (int(context.tokens_seen) < 40 || int(context.iter) < 1)",
  "modifier": { "set_context": { "iter": "int(context.iter) + 1", "firing": "''",
                                 "compacting": "''" } } }
```

Three things are decided here and nowhere else.

The threshold `40` is a fixture-scale number, chosen so four rounds of a deterministic mock
cross it; a real one is a fraction of the model's context window, in the tens of thousands.

Compaction sets `tokens_seen` back to `0`, and that reset is what makes the lane fire again
later instead of once. The accumulator *is* the lane's state.

The iteration bound `int(context.iter) >= 1` is the number of rounds kept verbatim
(`KEEP_ROUNDS`, one) expressed on the edge: a fold needs something to fold. Compaction does
not consume an iteration; only the brain edge counts.

### The lane, policy in `code` and prose in `llm`

`/compact` is three ordinary cells:

| cell | type | what it decides |
|---|---|---|
| `prep` | `code` | groups the rebuilt thread into rounds, picks the cut, writes the prompt |
| `condense` | `llm` | the prose, and only the prose |
| `emit` | `code` | one `insert` of the summary row, or the degradation route |

Three rules are worth taking from the compaction lane of
[prime-agent](https://github.com/PrimeIntellect-ai/prime-agent), and each lands in a different
place here:

1. Summarize iteratively. The previous summary is already the second message of the rebuilt
   thread, so `prep` finds it without asking the store: it enters the prompt as the running
   summary and the next fold refines it instead of starting from a blank page.
2. Force fixed sections. The instruction shipped by `prep` names four headings: goal,
   progress, key decisions, next steps. Fixed shape is what makes two folds of the same turn
   comparable; a summary whose shape drifts can only be rewritten, never refined.
3. Never cut at a `tool_result`. `prep` groups the thread into rounds, a round beginning at the
   first assistant `tool_call` of a run of them, and folds whole groups. A cut between groups
   cannot land between a `tool_call` and its answer, so every folded prefix stays a valid
   provider thread. The rule is structural; nothing checks it afterwards.

### The rebuild, `user + summary + tail`

`emit` writes one row whose `iter` column carries the boundary:

```text
turn_id  iter  role       fired  turn
t-7      2     summary    null   {"origin":"assistant","type":"text","text":"GOAL: …"}
```

The collector's rebuild (step 5) reads that column. Without a summary row it is the plain
cumulative thread; with one it is the user turn, the newest summary, and every `assistant`/`tool`
row behind the boundary. After a fold at boundary `2`, a four-round turn re-enters the brain
with four messages instead of nine.

The chain re-enters through the collector instead of around it: the summary insert comes back
as an ordinary store reply, and because it carries `context.compacting`, the collector re-selects
with `firing = 1` instead of racing for a guard the chain already holds.

### Compaction folds the view and keeps the record

The folded rows stay in `thread`, byte for byte. Nothing is updated, nothing is deleted; the
summary row is an addition. That is the same discipline the collector template applies to its
own window: a cap is a read-time cut of something the environment still holds, and rows fall only
where deletion is the declared job. A later fold, an audit, or a session batch still sees the
full turn.

### The lane can always leave

A fold that produced nothing, an empty answer or a provider error, must not fold the rounds into
nothing, and it must not park the turn either. `emit` then leaves on `refire`: a plain select that
sends the uncompacted window on. That costs the tokens the fold would have saved and finishes
the turn; parking finishes nothing. `prep` carries the same exit for a thread with nothing to
fold, which the iteration bound above already makes unreachable.

### What it costs

A fold replaces the one hop of a plain re-entry with eight: collector, `prep`, `condense`,
`emit`, the store, the reply back, the re-select, its reply, and then the brain. It therefore
costs seven extra hops on the round it interrupts. The next section's advice applies unchanged,
and more so: put `restore_ttl` on the re-entry edge and the loop pays for one round at a time,
fold included.

## The TTL budget of one round

`ttl` is the routing-loop guard: colony decrements it on every routing decision and a message
that reaches `0` is dead-lettered (`meclaw-overview.md` § *Message model*). One user-visible
tool round in this shape costs about a dozen routing hops, and not one, because the
collector's read-modify-write conversation with the store is itself routing:

| Leg | Hops |
|---|---|
| planner to dispatcher | 1 |
| dispatcher to tool | 1 |
| tool to collector | 1 |
| collector to store insert, and the reply back | 2 |
| collector to store select, and the reply back | 2 |
| collector to store guarded update, and the reply back | 2 |
| collector to store firing select, and the reply back | 2 |
| collector to planner (the loopback edge) | 1 |
| **one round** | **~12** |

Parallel tool calls do not multiply this: `ttl` lives on each message envelope, so the branches
burn their own copies and the number above is the cost along the chain that re-enters the
planner.

Measured on the checked-in fixture (`tests/fixtures/14b-tool-loop-store`, pinned in
`crates/meclaw-cells/tests/tool_loop_ttl_budget.rs`): six tool rounds end to end cost 76
routing hops. `message_default_ttl` defaults to 64, so the default budget holds five rounds and
the sixth runs out. Five rounds is not generous for an assistant. "Write the file, read it
back, fix it, verify, then summarise" is five.

### The recommended form, letting the loopback edge restore the budget

Raising the colony-wide budget pays for every round of every turn up front, and it makes the
number of rounds an agent may take a property of `colony.json` instead of a property of the
loop. The loop can pay per round. An edge may declare that it restores the routing budget of
the message it takes (GH #82, ruling 2026-08-13):

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
loop then only ever has to fit one round into the budget instead of all of them, so
[`examples/telegram-research`](../examples/telegram-research/) needs no `colony.json` at all:
six rounds and more run on the substrate default of 64. Pinned in
`crates/meclaw-cells/tests/tool_loop_ttl_budget.rs`
(`six_tool_rounds_complete_on_the_default_budget_when_the_loopback_edge_restores_ttl`).

The restore resets the budget and never accumulates it, it never lowers a message ingested with
a larger budget, and it moves the runaway guard from TTL to the iteration bound in the edge's
own `condition`, which is why a restoring edge without a condition is refused at config load
and at `add_edges` validation. The three properties in full, with their rationale:
`meclaw-overview.md` § *Message model*. The ceiling is pinned in
`a_restoring_loopback_edge_never_lifts_ttl_above_the_initial_budget`.

### Sizing the budget instead, for shapes without the modifier

A shape whose re-entry edge does not restore still has to fit its whole run into one budget.
Size it on purpose, in `colony.json`:

```json
{ "schema_version": 1, "message_default_ttl": 160 }
```

Rule of thumb: `message_default_ttl >= 4 + rounds * 12` for a store-backed loop. The `160` above
buys twelve rounds. Per initial message the HTTP ingress accepts a `ttl` field that overrides it.

### TTL exhaustion is a silent stall, so bound the loop yourself

TTL expiry is terminal: the message goes directly to the dead-letter queue and skips the
`reply_to` cascade (`meclaw-overview.md` § *Message model*). Inside a fan-in that is invisible
from the agent surface. The collector's fan-in never completes, so it parks, and nothing is
emitted toward the origin: no answer, no error, nothing a topology can route on. The colony
logs the death loudly (an `ERROR` line naming the message id, its target, and the trace id) and
writes a `ttl_expired` dead-letter row; those are the operator's signals, and they are the only
ones.

TTL is a substrate guard and never the loop's bound. Bound the loop where the loop lives, on
the recommended edge above, with the iteration counter it already owns, and give its condition
the `has()` guard on the optional key.

With `restore_ttl` that bound is mandatory. It is the only thing left that stops the loop, which
is exactly why the substrate refuses an unconditional restoring edge.

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

Three things carry over to another store-backed loop. Give every inbound request a stable
correlation ID, and test completeness by ID membership scoped to that ID and the current
iteration. Put the one-shot guard in the store operation and never in code-cell memory, and
rebuild the thread only after winning it. Count the rounds on the edge that re-enters the
`llm` cell, where the counter is already declared.

Validate the worked example without external credentials:

```bash
cargo run --bin meclaw -- --root ./examples/telegram-research --validate
```

Running the live example additionally requires the Telegram, LLM, and search credentials
documented in its README.
