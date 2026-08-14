# `talky@1.0.0`

A whole conversational agent as one template. Five units under one hive:
[`session-keeper@1`](../session-keeper/) as `keeper`, [`collector@1`](../collector/) as
`collector`, [`dispatcher@1`](../dispatcher/) as `split`,
[`summarizer@1`](../summarizer/) as `summary`, plus an `llm` brain and one error
collector. No new cell type, no Rust.

**The Egon rollout wired this by hand.** Keeper in the ingress, collector at the seam,
dispatcher for the fan-out, summarizer on the close path -- twenty-two edges, each of
them a decision that had already been made in a README. That is the definition of a
composite: a recurring unit that should be instantiated, not re-derived. Here it is one
`add_nodes` plus the four port edges the parent has to draw anyway.

## What it delivers

- **One id per conversation, minted once.** The keeper stamps every inbound turn with
  the generation its channel is currently in; the internal edge promotes
  `hop.session_id` to context, and that promotion *is* the stamp. Everything downstream
  reads it and nobody else mints one.
- **The seam, already bounded.** The collector hands the assembled context to the brain
  over ONE edge, and that edge carries the two things the loop needs: the iteration
  counter and `restore_ttl`. A tool round is a dozen routing hops; without the restoring
  edge the fifth round dies mid fan-in with nothing emitted towards the surface.
- **A tool round that only needs its tools.** `brain -> split -> (your tools) ->
  collector -> brain` is pre-wired except for the one lane that is genuinely
  per-instance: which cell answers to `web_search`. Adding a tool is one edge pair, never
  a topology change.
- **A close that hands the day on twice.** When a generation ends, the collector's batch
  leaves on the write port (the parent decides where a day belongs) **and** enters the
  summarizer, whose one recency-weighted summary lands in the brain's `system.handover`
  slot -- without a provider call, because a system update carries no `messages[]`. The
  next generation opens lazily on the first morning turn and already knows yesterday.
- **One place errors leave from.** The brain's failed inference and the summarizer's
  failed summary fan into `./errors` and leave as one normalised report. A parent drains
  one edge, not three.

## Cells

| path | type | from |
|---|---|---|
| `keeper/{stamp,close,sessions,night}` | `code`, `code`, `store`, `timer` | `session-keeper@1` |
| `collector/{assemble,window}` | `code`, `store` | `collector@1` |
| `split` | `code` | `dispatcher@1` (a single-cell template) |
| `brain` | `llm` | this template |
| `summary/{prep,writer}` | `code`, `llm` | `summarizer@1` |
| `errors` | `code` | this template |

### How the sub-units are referenced: materialised copies, pinned

The substrate has **no template-in-template reference**. Instantiation is a recursive
directory copy (`docs/meclaw-overview.md` § Instanziierungs-Flow), and a `template.json`
inside the tree would only register a second template with the scanner. So the four
sub-units live here as **byte copies of their `config.json` files** -- no
`template.json`, no README, nothing patched.

That is a fork risk, and it is pinned rather than hoped away:
`crates/meclaw-cells/tests/talky_composite.rs` asserts every copied `config.json` is
byte-identical to its source template. A change to `collector@1` that does not travel
into `talky/collector/` fails there, in the same test run, instead of drifting into
production.

## Ports

**Four external ports, and the parent wires all four in the SAME mutation that
instantiates the composite** -- an island without a crossing edge derives inactive and
its timer never spawns.

| port | endpoint | direction | what travels |
|---|---|---|---|
| ingress | `./keeper/stamp` | in | the surface turn, lane `in_turn` |
| reply | `./collector/assemble` | out | `hop.route == 'answer'` |
| write | `./collector/assemble` | out | `hop.route == 'write'` -- the closed session as one batch |
| error drain | `./errors` | out | `hop.route == 'error'` |

Plus, per instance, the two **advisor lanes** to an agent core -- see below.

```json
{"from": "<surface>", "to": "./talky/keeper/stamp",
 "condition": "has(hop.user_id) && int(hop.user_id) == 12345",
 "modifier": {"set_hop": {"route": "'in_turn'"},
              "set_context": {"channel": "hop.chat_id"}}},
{"from": "./talky/collector/assemble", "to": "<reply sink>",
 "condition": "has(hop.route) && hop.route == 'answer' && !has(hop.round_capped)"},
{"from": "./talky/collector/assemble", "to": "<day archive or memory>",
 "condition": "has(hop.route) && hop.route == 'write'",
 "modifier": {"set_hop": {"route": "'in_batch'"}}},
{"from": "./talky/errors", "to": "<drain or alarm>",
 "condition": "has(hop.route) && hop.route == 'error'"}
```

**The channel promotion is the parent's duty, and it is not optional.** Without
`set_context: {"channel": ...}` on the ingress edge every chat of the colony lands on
the channel `default` -- the right answer for a single-surface colony, the wrong one for
a bot with many chats. Whatever a surface calls "the same conversation partner" goes in
there: a Telegram/Slack `hop.chat_id`, a room, a phone number.

**Numbers on the hop need `int()`.** A proxy delivers JSON integers, CEL deserialises
them as `uint`, and a bare `hop.user_id == 12345` is silently **false** -- no error, no
log line. Every numeric condition on the ingress edge carries the cast.

**The reply lane carries two sorts.** A real answer and a round that hit
`COLLECTOR_MAX_ITER`, told apart by `hop.round_capped == "1"`. The composite does not
decide which of them a user sees: guard the reply edge with `!has(hop.round_capped)` and
give the capped sort its own edge (the error drain is the usual target) -- or let it
through deliberately.

### Per-instance lanes (not ports of this template)

**Tools stay outside.** The tool set is the per-agent choice, so the composite carries no
tool cells and no map of them. Wiring a tool is one edge pair:

```json
{"from": "./talky/split", "to": "./search",
 "condition": "has(hop.tool_name) && hop.tool_name == 'web_search'"},
{"from": "./search", "to": "./talky/collector/assemble",
 "modifier": {"set_hop": {"route": "'in_tool'"}}}
```

The `has()` is not decoration: the `calls`, `result` and `answer` emissions carry no
`tool_name` at all, and an unguarded comparison **errors** in CEL, which skips the edge
with a log line per lane per message. A tool name nobody answers to dead-letters and
stalls that round until the collector's idle window closes it (`COLLECTOR_ROUND_IDLE_MS`).

**One tool is served inside the composite:** `memory_recall` (GH #78). It is wired like any
other tool -- `./talky/split` on `hop.tool_name == 'memory_recall'` -- except that the cell
behind the edge is the collector itself (`set_hop {"route": "'in_memory_call'"}`), because
it already owns the recall port. Its schema and the second half of the wiring live in
[`../collector/README.md`](../collector/README.md) § The memory tool.

The tool SCHEMAS are a different thing again: they live in the brain's `system.tools`,
seeded (`brain/seed/system.jsonl`) or written by a system update. The composite carries
neither -- identity, instructions and tools are the agent, not the topology.

Three more optional lanes, all of them the parent's decision:

| lane | endpoint | when |
|---|---|---|
| memory recall | `./collector/assemble` route `recall` out, lane `in_bundle` in | the per-turn leg only with `COLLECTOR_MEMORY_TIER` set; the same pair also serves the memory **tool** below |
| memory tool | `./talky/split` on `hop.tool_name == 'memory_recall'` → `./collector/assemble` lane `in_memory_call` | GH #78 -- one more tool edge, plus the recall pair above |
| forced sweep | `./keeper/close` lane `in_sweep` | an operator or a second schedule |
| housekeeping | `./collector/assemble` lanes `in_prune`, `in_round_sweep` | a timer; the template never fires them itself |

## The internal wiring, edge by edge

Twelve edges in this hive's `params.graph`, plus the ten the four sub-units bring with
them. Read it as the round it is:

```
keeper/stamp  --(turn, session_id -> context)-->  collector/assemble  in_turn
keeper/close  --(close, session_id -> context)->  collector/assemble  in_close

collector/assemble ==(brain, int(hop.iter) < 12, restore_ttl)==> brain     <- THE SEAM
brain --(stop | tool_calls)--> split
brain --(length)-------------> collector/assemble  in_answer
brain --(error | content_filter)--> errors

split --(calls)---> collector/assemble  in_calls      split --(tool)--> [your tools]
split --(result)--> collector/assemble  in_tool
split --(answer)--> collector/assemble  in_answer

collector/assemble --(write)--> summary/prep  in_batch     (AND out of the write port)
summary/prep --(summary)------> brain          <- system.handover, no provider call
summary/prep --(summary_error)-> errors
```

**The loopback bound is an edge literal, on purpose.** `int(hop.iter) < 12` is a safety
belt, not the policy: the round is bounded by `COLLECTOR_MAX_ITER` (default 8), which
ends a runaway round with a message on the `answer` lane instead of a silence. The edge
number only has to be larger. Env substitution does not reach edge conditions -- a
`${VAR}` there would be registered verbatim and fail to parse as CEL -- so raising it is
a mutation: `remove_edges` first, `add_edges` second, in **two** mutations. A remove and
an add of the same endpoints in ONE diff match over the post-state and take the new edge
with them.

**`restore_ttl` sits on the seam, once per round.** `iter` counts brain answers, and a
bundle of fifteen calls is one answer, one iteration, one restore. The substrate refuses
a restoring edge without a condition, because the iteration bound is then the only thing
left stopping the loop.

### The advisor lanes (GH #28, R-CG-3)

An agent core (`cogny`) is a **sibling hive** of the talkies, not a cell inside one: one
core, N channel voices. It is reached like a tool and answers like an event, so the
connection is two edges plus one knob.

```json
"DISPATCHER_ASYNC_TOOLS": "consult_cogny"
```

```json
{"from": "./talky/split", "to": "/agent/cogny/collector/assemble",
 "condition": "has(hop.tool_name) && hop.tool_name == 'consult_cogny'",
 "modifier": {"set_hop": {"route": "'in_turn'"},
              "set_context": {"consult_id": "hop.consult_id", "col_phase": "''"},
              "restore_ttl": true}},
{"from": "/agent/cogny/collector/assemble", "to": "./talky/collector/assemble",
 "condition": "has(hop.route) && hop.route == 'answer'",
 "modifier": {"set_hop": {"route": "'in_advice'"},
              "set_context": {"col_phase": "''"},
              "restore_ttl": true}}
```

Four things in that pair are load-bearing:

- **`col_phase` must be cleared.** Both messages leave *another* collector's chain and
  carry whatever step that chain was in. A collector's `in_turn` / `in_advice` refuses a
  message that arrives mid-assembly, so the port edge resets the key. Everything else in
  the context rides along on purpose -- `session_id` above all, which is what keeps one
  consultation inside the channel's own session.
- **`consult_id` becomes context**, because the hop is single-hop and the correlation has
  to survive the core's whole chain and come home with the answer.
- **`restore_ttl` on both**, with the condition they already carry: an errand is a fresh
  journey, not the tail of the turn that started it.
- **The errand arrives as a `tool_call` turn.** Its text is the raw arguments the model
  wrote, and the core's collector files that as the turn: the talky IS the core's user.

What the parent does *not* wire: nothing else. The turn ends with the interim answer the
dispatcher already sent to the channel, and the returning advice starts a fresh talky
round that verbalises it in the channel's own voice.

**The duration estimate (GH #123, observe-only).** Put the hints in the brain's own
instructions and let the model fill `arguments.eta` in the same call it already makes:

```
consult_cogny(question, eta): eta is a coarse guess at how long the answer will
take -- "seconds" for a lookup, "a minute" for a web search, "minutes" for deep
reasoning. Say what you are doing in the same reply; that sentence reaches the
user immediately.
```

The estimate rides out as `hop.consult_eta` and **nothing reads it**. First we watch how
well the model guesses.

## Knobs

Everything the sub-templates parametrise is forwarded verbatim: the `${VAR:-default}`
literals travel into the instance and bind **late**, at every read, so a `.env` change
plus a reboot moves them without touching a config.

| env var | default | unit |
|---|---|---|
| `KEEPER_IDLE_MS` | `7200000` | keeper -- silence before a generation may end (2 h) |
| `KEEPER_NIGHT_CRON` | `0 0,30 22-23,0-3 * * *` | keeper -- the sweep, **in UTC** (summer image of 00:00-05:30 CEST) |
| `KEEPER_CLOSE_LIMIT` | `50` | keeper -- generations one firing may seal |
| `COLLECTOR_WINDOW_TURNS` | `12` | collector -- newest turns entering the context |
| `COLLECTOR_WINDOW_BYTES` | `8000` | collector -- byte cap over the window |
| `COLLECTOR_TURN_CHARS` | `4000` | collector -- per-turn cap before the byte cap |
| `COLLECTOR_TOOL_CHARS` | `4000` | collector -- per-item cap on tool results |
| `COLLECTOR_ROUND_BYTES` | `16000` | collector -- byte cap over the whole tool round |
| `COLLECTOR_MEMORY_CHARS` | `8000` | collector -- cap on the memory bundle |
| `COLLECTOR_MAX_ITER` | `8` | collector -- **the loop bound**; at the cap the seam leaves on `answer` |
| `COLLECTOR_ROUND_IDLE_MS` | `120000` | collector -- idle window of one tool round |
| `COLLECTOR_MEMORY_TIER` | (empty) | collector -- empty = no memory leg at all |
| `COLLECTOR_MEMORY_CALL_TIER` | `1` | collector -- tier of the `memory_recall` tool; empty = tool off |
| `COLLECTOR_MEMORY_FORM` | `readable` | collector -- `readable` / `json` / `both` |
| `COLLECTOR_PRUNE_AFTER_MS` | `604800000` | collector -- age gate on the prune lane (7 d) |
| `DISPATCHER_MAX_CALLS` | `16` | split -- per-answer call budget |
| `DISPATCHER_ASYNC_TOOLS` | (empty) | split -- comma-separated tools that answer on their own lane instead of inside the round (`consult_cogny`) |
| `SUMMARIZER_RECENT_TURNS` | `12` | summary -- newest turns travelling verbatim |
| `SUMMARIZER_PHASEOUT_CHARS` | `200` | summary -- per-turn cap on the phased-out turns |
| `SUMMARIZER_TOOL_CHARS` | `200` | summary -- per-item cap on tool previews |
| `SUMMARIZER_ROUND_LINES` | `40` | summary -- tool-activity lines at most |

**`ctx.model` is the one instantiation-class knob** and it is strict: `add_nodes` without
it is rejected with `ctx_key_missing`. Per the K-H2 convention the builder resolves
`MODEL_<ROLE>` from `.env` itself and passes the **resolved literal**. Both `llm` cells
read the same key; a cheaper summarizer is one `override_params` on
`summary/writer.params.model`, and a subscription or another provider for the brain is
`override_params` on `brain.params` (`provider`, `auth`, `auth_ref`, `base_url`).

**The TTL budget.** With `restore_ttl` on the seam the colony default of 64 carries the
loop: only ONE round has to fit the budget. A tree that removes the restoring edge sizes
`message_default_ttl >= 4 + rounds * 12` in its `colony.json` instead.

## Instantiating it

```bash
curl -s -X POST http://127.0.0.1:PORT/colony/mutations -H 'Content-Type: application/json' \
  -d '{"scope":"/main/agent","ctx":{"model":"openai/gpt-4o-mini"},"diff":{
        "add_nodes":[{"name":"talky","template":"talky"}],
        "add_edges":[ ... the four ports plus the tool lanes, in the SAME mutation ... ]}}'
```

The composite comes up with all eleven cells (plus four hive markers); the `timer` spawns
as soon as the crossing edge makes the subtree active, and the `store`/`llm` cells report
`active=true` + `NotYetSpawned`, which is the correct hot/cold form for a stateful cell.
Two things to have ready before the mutation:

1. **A `colony.db` whose three tables agree** (`registry`, `edges`, `hive_scopes` -- all
   empty or all filled). A mutation that is REJECTED leaves a colony whose next boot
   panics (GH #89), so bring the edges in the same diff and check the table counts after
   any rejection.
2. **The brain's identity**, either as `brain/seed/system.jsonl` (which only takes on a
   FRESH birth -- a `cell.db` that already exists means `Resumed` and an inert seed) or
   as a system update message. Neither is this template's business.

## What it is not

- **Not a surface.** No proxy, no HTTP ingress, no allowlist. Who is allowed to talk to
  the agent is an edge condition on the ingress port, in the parent scope where the
  surface lives.
- **Not a memory.** The recall leg is optional and the write batch leaves unfiltered.
  What a day is worth is the receiver's question.
- **Not a persona.** Identity, instructions and tool schemas live in the brain's
  `cell.db`, one writer per `system` path: the collector owns `messages[]` and
  `system.memory`, the summarizer owns `system.handover`, an affinity cell (if any) owns
  the rest. The topology owns none of it.
- **Not a drain.** `./errors` normalises and forwards; it does not swallow. An unwired
  error port dead-letters, loudly.
- **Not one instance per day.** v1 runs the logical generation: same cells, new id.
- **Not the agent core.** The talky is the channel voice; the thinking, the agent-level
  memory and the heavy tool work belong to a `cogny` hive next to it (R-CG-1). The
  composite carries the two lanes to reach it and nothing of what happens there.

## Pins

- `crates/meclaw-cells/tests/talky_cogny_advisor.rs` -- the advisor connection end to
  end: an interim answer and a consult call out of ONE brain response, a round that
  closes without waiting, the agent core's own tool round, the result home on
  `in_advice`, and the bilateral question-back under one `consult_id`. Plus the pin that
  no idle window ever waits for the core (one-millisecond window, two sweeps, nothing
  swept).
- `crates/meclaw-cells/tests/talky_composite.rs` -- the shipped template in a running
  colony against the mock OpenAI wire: one turn through keeper, seam, brain, split, a
  tool and back to the seam (two provider calls, the second one carrying the tool
  result, the answer carrying the minted session id and `iter=1`); a close whose batch
  reaches the write port AND becomes the handover that the NEXT generation's prompt
  carries -- with exactly one extra provider call, which is what proves the system
  update is silent. Plus the byte-identity pin over the four sub-unit copies.
- The sub-units keep their own pins: `session_keeper.rs`, `collector_window.rs`,
  `collector_colony.rs`, `dispatcher_split.rs`, `summarizer_prep.rs`,
  `summarizer_colony.rs`.
