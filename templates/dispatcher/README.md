# `dispatcher@1.1.0`

The fan-**out** half of a tool loop, as one `code` cell -- no new cell type, no Rust.
Its counterpart is the fan-**in**: [`collector`](../collector/), which assembles the
round and re-enters the brain. Splitting the two is the point (`R-OS-2`): routing a call
to a tool and assembling a context window are different jobs, and a cell that did both
would be the monolith the DSL exists to avoid.

A brain answers a turn in one of two ways. It says something final, or it asks for tools
-- possibly several in one answer, in **one bundle**. This cell turns that bundle into
messages a graph can route.

## What it delivers

- **One message per call, addressed by NAME.** A call leaves carrying `hop.tool_name`
  and `hop.tool_call_id` and nothing else. Which cell answers to `web_search` is a
  question for an edge condition, never for this cell: no tool list, no map of the tree,
  nothing to update when a tool is added.
- **The expectation set first.** The assistant turn goes to the fan-in *before* any tool
  message leaves (PLAIN order). A tool that answers in a millisecond can otherwise report
  a result for a round the collector has not been told about yet.
- **A budget that answers instead of dropping.** A bundle above `DISPATCHER_MAX_CALLS`
  runs no tool at all -- and every expected `tool_call_id` still gets a reply, a synthetic
  error `tool_result`. The round stays fan-in-complete, so the brain sees the refusal and
  has to respond to it. A silently dropped call would stall the fan-in until the TTL runs
  out, and TTL expiry emits **nothing** towards the surface.
- **A final answer, passed through.** `finish_reason == 'stop'` leaves on its own lane,
  unchanged.
- **A sentence next to the bundle, delivered at once.** One brain response may carry
  `content` **and** `tool_calls`. The text leaves on the `answer` lane with
  `hop.interim = "1"` while the calls keep running: the turn ends with "one moment, I am
  asking" instead of with silence, and no second inference is spent on saying so.
- **An async tool class that opens no expectation.** Names listed in
  `DISPATCHER_ASYNC_TOOLS` are classified here -- this cell is the only one that ever sees
  the whole bundle -- and their `tool_call_id`s ride out on `hop.async_calls`. The fan-in
  reads that and waits for the *other* calls only, so a tool that thinks for minutes never
  races the round's idle window.
- **A handoff class on top of it, which also ends the turn.** Names listed in
  `DISPATCHER_HANDOFF_TOOLS` are async *and* say that the answer comes from a **later
  turn** -- an advisor's event, an escalation re-entering the seam. Their ids ride out on
  `hop.handoff_calls` beside `hop.async_calls`, and the fan-in files the round they leave
  behind as over even when no sentence stood beside the bundle. An async call that is *not*
  a handoff is fire-and-forget: the model still owes this turn an answer
  ([#372](https://github.com/mmeyerlein/meclaw/issues/372)).

## The cell

| cell | type | what it holds |
|---|---|---|
| the template root | `code` | the whole split: lanes, the budget, the OpenAI-form unwrap. No state, no `cell.db`. |

This is a single-cell template (the `_cell-types` shape): instantiate it under any name --
`split` and `dispatch` are the usual ones -- and the instance IS the cell.

```bash
curl -s -X POST http://127.0.0.1:PORT/colony/mutations -H 'Content-Type: application/json' \
  -d '{"scope":"/agent","ctx":{},"diff":{
        "add_nodes":[{"name":"split","template":"dispatcher"}],
        "add_edges":[ ... the routes below, in the SAME mutation ... ]}}'
```

## Ports

Entry is the brain's output; there is one lane in and four out, all on `hop.route`.

| route | to | what travels |
|---|---|---|
| `calls` | the collector's `in_calls` port | the assistant turn **verbatim** (all of it, a text turn next to the calls included) -- the expectation set of the round. `hop.call_count` sizes it (the number of `tool_call` turns in the bundle, as a string), `hop.async_calls` names the ids the fan-in must not wait for |
| `tool` | one tool cell per name | one `tool_call` turn with the **raw arguments**; `hop.tool_name` selects the cell, `hop.tool_call_id` correlates the result |
| `result` | the collector's `in_tool` port | a synthetic error `tool_result` for a call that will never run; `hop.error_code` says which kind |
| `answer` | the collector's `in_answer` port, or the reply sink | the brain's final turn, `hop.finish_reason` carried along -- **or**, with `hop.interim = "1"`, the sentence that stood next to a bundle |

The tool lanes guard the key they discriminate on:

```json
{ "from": "./dispatcher", "to": "./collect",
  "condition": "has(hop.route) && hop.route == 'calls'",
  "modifier": {"set_hop": {"route": "'in_calls'"}} },
{ "from": "./dispatcher", "to": "./collect",
  "condition": "has(hop.route) && hop.route == 'result'",
  "modifier": {"set_hop": {"route": "'in_tool'"}} },
{ "from": "./dispatcher", "to": "./search",
  "condition": "has(hop.tool_name) && hop.tool_name == 'web_search'" },
{ "from": "./dispatcher", "to": "./shell",
  "condition": "has(hop.tool_name) && hop.tool_name == 'bash'" }
```

The collector endpoint is the **hive**, never a cell inside it: `collector` is sealed
(`params.ports: []`), so an edge naming `./collect/assemble` in a mutation is refused with
`hive_port_boundary`. Nothing is lost by dropping the segment -- the `set_hop` already names
the lane, and the lane is what the hive dispatches on.

The `has()` is not decoration: `hop` is single-hop, so the `calls`, `result` and `answer`
emissions carry no `tool_name` at all. A bare `hop.tool_name == 'web_search'` does not
evaluate to `false` on those, it **errors** (CEL semantics) and the substrate skips the
edge with a log line per lane per message. Same rule as everywhere else --
[`docs/store-backed-tool-loop.md`](../../docs/store-backed-tool-loop.md).

## Knobs

**Env knobs are an experimental surface.** Until this template's knobs move onto the `params`
block of the cells that read them, their names carry no compatibility promise and may change in
any `0.x` release; provider credentials keep living in `.env` either way. The migration is
tracked in [#138](https://github.com/mmeyerlein/meclaw/issues/138), with the
`collector@1.2.0` migration ([#136](https://github.com/mmeyerlein/meclaw/issues/136)) as the
reference pattern.

| env var | default | meaning |
|---|---|---|
| `DISPATCHER_MAX_CALLS` | `16` | per-answer call budget. **At** the cap the bundle runs; one call over it, the bundle is refused **as a whole** and every id is answered with `call budget exceeded`. |
| `DISPATCHER_ASYNC_TOOLS` | (empty) | comma-separated tool names that answer on a lane of their own instead of inside the round. Empty = no call is ever async. |
| `DISPATCHER_HANDOFF_TOOLS` | (empty) | comma-separated tool names whose call ends the **turn**, because the answer comes from a later one. Naming a tool here declares it async as well -- the two lists are unioned. Empty = every async call is fire-and-forget. |

One knob per concern: the first bounds **one brain answer**, the second says which calls
the round is allowed to end without, the third which of those end the turn with them.
None of them bounds the loop -- see below.

## The async class (GH #28) and the handoff class (GH #372)

An advisor that thinks does not fit inside a round. Waiting for it would mean betting the
round's idle window against thinking time -- and losing that bet means a synthetic "tool
result lost" in the transcript. So the class is declared, once, here:

```
DISPATCHER_HANDOFF_TOOLS=consult_cogny
DISPATCHER_ASYNC_TOOLS=write_journal
```

**`remember` used to be the example in that second line, and it is not one any more.**
Per-turn memory extraction stopped being a tool call in `talky@4.1.0`: the model writes a
fenced block into its own answer and a splitter cell cuts it out
([#379](https://github.com/mmeyerlein/meclaw/issues/379)). The async class itself is
unchanged and stays documented here -- it is what any fire-and-forget tool an instance
wires needs -- but its one shipped user is gone, and the open substrate bug underneath it
([#378](https://github.com/mmeyerlein/meclaw/issues/378): a completion mixing text with an
asynchronous call strands its round) is now something a NEW async tool would walk into,
not something the shipped tree walks into every turn.

**Two lists, because "does not answer inside the round" and "does not answer this turn at
all" are two facts.** A consult is a **handoff**: the advisor's answer comes back as its
own turn, so the round the call leaves behind is over even when the model sent no sentence
beside it. A memory write is **fire-and-forget**: it answers nothing and never comes back,
so the model still owes this turn a sentence -- and the fan-in leaves that round OPEN for
the iteration it has not spent ([#372](https://github.com/mmeyerlein/meclaw/issues/372)).
Naming a tool in the handoff list declares it async as well, so `consult_cogny` belongs in
exactly one of the two.

What changes for a call whose name is on either list:

| key | value |
|---|---|
| `hop.async_calls` (on the `calls` lane) | the comma-joined `tool_call_id`s of the async calls in this bundle -- the fan-in opens no expectation for them |
| `hop.handoff_calls` (on the `calls` lane) | the subset of those whose tool is on `DISPATCHER_HANDOFF_TOOLS`. Always present, empty included: a hop key that is sometimes absent makes a CEL modifier fail, and a failed modifier skips the edge |
| `hop.async` (on the `tool` lane) | `"1"` |
| `hop.consult_id` | `arguments.consult_id` when the model answers a question the advisor asked back, otherwise the `tool_call_id`. One correlation across the whole exchange, in both directions. |
| `hop.consult_eta` | `arguments.eta` -- the model's own coarse duration estimate, produced in the SAME turn (GH #123). **Logged, never consumed.** |

The call itself travels exactly like any other: `hop.tool_name` selects the cell, the raw
arguments are the body. Nothing here knows what an advisor is.

**The duration estimate (GH #123, observe-only).** The estimate is not a component; it is
a habit given to the model in its own context. Put hints like these into the talky's
system prompt and let it fill `arguments.eta` in the same call:

```
consult_cogny: when you ask, add a coarse eta -- "seconds" for a lookup,
"a minute" for a web search, "minutes" for deep reasoning.
```

Nothing in the tree reads the value. It rides on the hop, lands in the message log, and
the first question it answers is how well the model estimates at all.

## Three things that belong on an edge, not in here

**1. The loop bound and `restore_ttl` (GH #82).** A tool round is a dozen routing hops, so
a loop that runs on the colony default budget of 64 dies in its fifth round -- terminal,
straight to the dead-letter queue, nothing emitted towards the origin. The fix is one
modifier on the **re-entry** edge (collector → brain), never one per tool answer:

```json
{ "from": "./collect", "to": "./brain",
  "condition": "has(hop.route) && hop.route == 'brain' && int(context.iter) < 12",
  "modifier": {"set_context": {"iter": "int(context.iter) + 1"}, "restore_ttl": true} }
```

A restoring edge **must** carry a condition -- the substrate refuses an unconditional one
at config load and at `add_edges`, because the iteration bound is then the only thing left
stopping the loop. Restore once per **round**, not once per tool result: `iter` counts
brain answers, and a bundle of fifteen calls is one answer, one iteration, one restore.
Derivation and hop table: [`docs/store-backed-tool-loop.md`](../../docs/store-backed-tool-loop.md).

**2. A failed inference.** A turn that is neither a bundle nor a final answer is terminal
here (empty multi-send). The `llm` cell's error path echoes the **input** conversation back
with `finish_reason: "error"`, and forwarding that onto the answer lane would file the
prompt as the agent's own words. Give the error its own edge off the brain, in front of the
dispatcher edge:

```json
{ "from": "./brain", "to": "./notify",
  "condition": "has(hop.finish_reason) && hop.finish_reason == 'error'" }
```

**3. A tool name nobody answers to.** A model can invent a name, and a name with no edge
is a routing error by design -- the message dead-letters, loudly, and the round then waits
for a result that never comes. If that matters in your tree, give the lane a floor: a
catch-all edge from the dispatcher into a cell that replies with a `tool_result` under the
same `tool_call_id`, keyed on the names you did **not** wire.

## Reading the script

| input | emissions |
|---|---|
| `tool_call` turns present, count ≤ budget | `calls` (the assistant turn), then the interim `answer` if a text turn stood next to them, then one `tool` per call, in bundle order |
| `tool_call` turns present, count > budget | `calls`, then the interim `answer` if a text turn stood next to them (the sentence is appended **before** the budget branch runs, so an over-budget bundle still says it), then one `result` per call: `call budget exceeded`, no tool message at all |
| a call whose `text` is not `{name, arguments}` | `result` with `error_code: malformed_tool_call` in place of that one call; the sound calls still run |
| no calls, `finish_reason == 'stop'` | one `answer` |
| anything else | nothing (empty multi-send, terminal) |

The OpenAI unwrap is the only content work the cell does: the `llm` cell emits a
`tool_call` turn whose `text` is the stringified `function` object, and a tool cell wants
the arguments alone. The `id` survives that unwrap unchanged -- everything downstream
correlates on it.

Pinned in [`crates/meclaw-cells/tests/dispatcher_template.rs`](../../crates/meclaw-cells/tests/dispatcher_template.rs):
the script half runs the shipped `script_inline` against real stdin documents, the colony
half boots this template and routes a two-tool round through real edges.
