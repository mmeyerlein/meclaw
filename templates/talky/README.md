# `talky@1.2.0`

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

Optional, and off unless the instance switches it on: a fifth exit `turn_write` on
`./collector/assemble`, the **same** batch after every stored turn -- see "Per-turn
episodes" below.

**These four addresses are the port contract.** `./keeper/stamp`,
`./collector/assemble`, `./split` and `./errors` are stable **addresses**, not
implementation detail that happens to be reachable: the working colonies under
[`../../examples/`](../../examples/) wire them literally, and so does anything
built from this template. Internal cell names inside the composite may be
rearranged in a version bump; these four may not — moving one is a breaking
change to every parent that wired it, and it gets a CHANGELOG Breaking entry and
a new major version, not a patch.


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
`max_iter`, told apart by `hop.round_capped == "1"`. The composite does not
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
stalls that round until the collector's idle window closes it (`round_idle_ms`).

**One tool is served inside the composite:** `memory_recall` (GH #78). It is wired like any
other tool -- `./talky/split` on `hop.tool_name == 'memory_recall'` -- except that the cell
behind the edge is the collector itself (`set_hop {"route": "'in_memory_call'"}`), because
it already owns the recall port. Its schema and the second half of the wiring live in
[`../collector/README.md`](../collector/README.md) § The memory tool.

The tool SCHEMAS are a different thing again: they live in the brain's `system.tools`,
seeded (`brain/seed/system.jsonl`) or written by a system update. The composite carries
neither -- identity, instructions and tools are the agent, not the topology.

### The sentence a memory-carrying persona has to contain

Whatever else an agent's identity says, one boundary is topology-invariant and belongs in
its instructions **verbatim**:

> What stands in this conversation window is your own knowledge, not something to look up.
> A question about what was just said you answer immediately, with no tool and no bridging
> sentence. The core / the memory is for what is **not** in the window.

Without it the front model asks memory for what it was handed a moment ago (#150, measured
in production): the answer is *correct*, so nothing looks broken — it just cost a bridging
sentence, a consult round trip and about six seconds instead of one and a half. The
instructions of a persona naturally enumerate what the core is FOR (deep thinking,
research, planning, long-term memory) and, without this sentence, never say what it is not
for. "What did I just tell you" reads, literally, as a memory question.

The same sentence is what keeps the boundary honest in the other direction: it names the
window as the model's own knowledge **and** the long-term store as the thing it must ask
for, so a question about an earlier day still leaves through the lane it should.

Three more optional lanes, all of them the parent's decision:

| lane | endpoint | when |
|---|---|---|
| memory recall | `./collector/assemble` route `recall` out, lane `in_bundle` in | the per-turn leg only with `memory_tier` set; the same pair also serves the memory **tool** below |
| memory tool | `./talky/split` on `hop.tool_name == 'memory_recall'` → `./collector/assemble` lane `in_memory_call` | GH #78 -- one more tool edge, plus the recall pair above |
| forced sweep | `./keeper/close` lane `in_sweep` | an operator or a second schedule |
| housekeeping | `./collector/assemble` lanes `in_prune`, `in_round_sweep` | a timer; the template never fires them itself |
| per-turn write | `./collector/assemble` route `turn_write` out | only with `turn_write` set -- see below |
| memory lookup | `./talky/split` on `hop.tool_name == 'ask_memory'` -> the cogny's ingress | the fast errand lane (GH #124); same edge as `consult_cogny` plus `consult_class` |
| inline extraction | `./talky/split` on `hop.tool_name == 'remember'` -> the hive's `inline-extraction` port, **plus** its `inline-reject` egress back into `./errors` | the write leg of the front model -- see "The memory tool `remember`" |

### Per-turn episodes (`turn_write`)

The write port fires at the **close**. For a day archive that is right; for a memory it
means nothing said today is retrievable until the night sweep has run, so a question
about the last exchange is answered out of an empty store. Set `turn_write=1`
and the collector hands the same batch out on route `turn_write` after every stored turn
and every stored answer. No model call is involved: this is the collector's own table
leaving one turn earlier.

```json
{"from": "./talky/collector/assemble", "to": "./memdrain/drain",
 "condition": "has(hop.route) && hop.route == 'turn_write'",
 "modifier": {"set_hop": {"route": "'in_batch'"},
              "set_context": {"session_id": "hop.session_id"}}},
{"from": "./talky/collector/assemble", "to": "./memdrain/drain",
 "condition": "has(hop.route) && hop.route == 'write'",
 "modifier": {"set_hop": {"route": "'in_batch'"},
              "set_context": {"session_id": "hop.session_id"}}}
```

**Both edges into the same consumer, or not at all.** The two routes carry the same
conversation in the same order; a consumer that recognises what it has already taken --
`memory-drain@1` does, through its ledger -- writes only the difference and mints the
same ids either way. Two *different* consumers would be two memories. And the write route
keeps its own job: it is the safety net that catches the turns the per-turn lane lost,
and the count gate over its batch is what proves none went missing.

Whoever consumes `write` for something else (a day archive, the summarizer inside this
hive) is untouched: `turn_write` is a route of its own precisely so that the close-only
consumers stay close-only -- firing the summarizer per turn would put a provider call on
a path that is model-free by design.

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
belt, not the policy: the round is bounded by `max_iter` (default 8), which
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
take -- "about ten seconds" for a memory lookup, "half a minute" for real
reasoning, "a minute or more" once a web search is involved. Say what you are
doing in the same reply; that sentence reaches the user immediately.
```

Those three phrasings are **measured, not chosen** (GH #124). Read out of a running
colony's own message log with [`scripts/trace_latency.py`](../../scripts/trace_latency.py),
the lookup lane sits around ten seconds and the thinking lane spreads much wider, with
its single largest gap being the core model's own turn. The earlier wording -- "seconds"
for a lookup, "a minute" for a search -- was a guess, and it was optimistic in the
direction that costs trust: a user told "seconds" who waits eleven of them has been
misled by the system, not by the model.

Measure your own deployment before you copy these words. The tool needs nothing but the
colony root, costs nothing, and takes a second:

```
scripts/trace_latency.py <colony-root> --lane brain_fast --lane brain --breakdown
```

The estimate still rides out as `hop.consult_eta` and **nothing reads it**. Consuming it
-- routing by expected class rather than by tool name -- is the open half of #124.

**The second errand name: `ask_memory` (GH #124, cogny 1.1.0).** The core answers on two
lanes -- a thinking one and a fast one -- and the lane is chosen by **which tool the model
called**, not by a number anybody had to measure. So the talky's brain carries TWO errand
tools, and the cogny's ingress edge turns the choice into `context.consult_class`:

```json
{"from": "./talky/split", "to": "/agent/cogny/collector/assemble",
 "condition": "has(hop.tool_name) && hop.tool_name == 'ask_memory'",
 "modifier": {"set_hop": {"route": "'in_turn'"},
              "set_context": {"consult_id": "hop.consult_id", "col_phase": "''",
                              "consult_class": "'lookup'"},
              "restore_ttl": true}}
```

The edge is the `consult_cogny` one with one key more; the return lane is shared, so
nothing else is drawn twice. Both names go in the knob:
`DISPATCHER_ASYNC_TOOLS=consult_cogny,ask_memory`. The wording of the two tool
descriptions is where the class boundary is sharpened -- `ask_memory` for "what do we
know about X", `consult_cogny` for anything that has to be thought about or looked up
outside. A misfiled errand costs a worse SENTENCE and never a wrong fact: both lanes hang
off the same collector, the same window and the same memory bundle, and the fast lane can
escalate. The details, the escalation and the model slots live in
[`../cogny/README.md`](../cogny/README.md), section "The two lanes".

### The memory tool `remember` (inline extraction)

The two lanes above ASK the memory. This one WRITES to it, and it is the only lane on
which the brain does two jobs in one call: it answers, and in the same response it emits
the durable memory the turn carried. That saves a second inference over the whole window
-- but the reason to do it is freshness, not tokens: a fact extracted at night cannot
answer a question asked this afternoon.

```json
"DISPATCHER_ASYNC_TOOLS": "consult_cogny,ask_memory,remember"
```

```json
{"from": "./talky/split", "to": "/agent/memory/extract-glue",
 "condition": "has(hop.tool_name) && hop.tool_name == 'remember'",
 "modifier": {"set_context": {"store_origin": "'inline'", "mem_phase": "'inline'"}}},
{"from": "/agent/memory/extract-glue", "to": "./talky/errors",
 "condition": "has(hop.route) && hop.route == 'reject'"}
```

**Two edges, never one.** The first is the memory hive's `inline-extraction` port; the
second is its `inline-reject` egress, and a hive egress nobody drains is an unrouted dead
end -- a block the hive discarded would vanish without a line anywhere, and the memory it
was meant to write would silently never exist. Draining it into `./errors` is enough: the
composite already normalises what leaves there.

**`remember` is an async tool, and that is what makes it free for the turn.** Named in
`DISPATCHER_ASYNC_TOOLS`, the collector opens no fan-in expectation for it: the turn ends
with the interim answer the dispatcher already sent to the channel while the write is
still travelling. Left out of the knob, the round waits for a result that never comes and
dies at its idle window instead.

**The session travels by itself, and it is load-bearing.** The seam edge promotes
`hop.session_id` into the context long before the answer exists, so the tool call arrives
at the hive carrying the conversation it was written in. That is what the hive binds the
block to -- the front model names no episode, because an episode id is a uuid the hive
mints and no model has ever seen one. A `remember` call that reaches the port without a
session in its context is rejected, by design.

**The schema, for the brain's `system.tools`.** Like every tool schema it is instance
state (`brain/seed/system.jsonl` or a system update), never template:

```json
{"type": "function", "function": {
  "name": "remember",
  "description": "<the contract block -- see below>",
  "parameters": {
    "type": "object",
    "properties": {
      "facts": {"type": "array", "items": {
        "type": "object",
        "properties": {
          "subject": {"type": "string"},
          "predicate": {"type": "string",
                        "description": "snake_case English key, lower case, no spaces"},
          "claim": {"type": "string"},
          "fact_kind": {"type": "string", "enum": ["world", "experience", "foresight"]},
          "valid_from": {"type": "string"},
          "confidence": {"type": "integer", "minimum": 0, "maximum": 100}},
        "required": ["subject", "predicate", "claim", "fact_kind"]}}},
    "required": ["facts"]}}}
```

**What is NOT in the schema is the point.** There is no `episode_id`, because the model
cannot know one and an invented id would file the facts against the wrong turn. And there
is no `valid_until`, because a validity a model derives from the range a QUESTION asked
about closes the fact on arrival -- invisible to the as-of leg, visible to keyword and
semantic, which is worse than a duplicate. Both were measured in a running colony. A
field a schema does not offer is a field constrained decoding cannot produce; the hive
enforces the same two rules again at its end, because it does not own the persona.

**The description IS the contract**, and it is shipped:
`templates/memory-hive/inline-contract.md`. Paste the fenced block from there into the
tool description rather than writing a new one -- the file is the authority, a drift lock
holds it against the batched extractor's own prompt, and a discipline each persona invents
for itself is a discipline nothing can hold to account.

**Order the instructions so the answer is written first.** The block belongs AFTER the
answer, not beside it: a model that produces its structured field before its reasoning
answers from nothing, which is the one robust finding in the format-constraint
literature. The shipped contract says so in its first line.

**It needs per-turn episodes.** A block is bound to the turn it answered, and that turn
has to BE in the memory when the call arrives. Set `turn_write=1` and wire the
`turn_write` lane above; without it the hive has nothing to bind to, rejects every block,
and the batched extractor keeps doing the work at night. That is the safe direction and it
is also the whole reason wave 9 came first.

## Knobs

Two classes since `collector@1.2.0`. The **env** knobs are `${VAR:-default}` literals that
travel into the instance and bind **late**, at every read, so a `.env` change plus a reboot
moves them without touching a config -- and they move every unit in the colony at once. The
**param** knobs ship with their defaults inside `./collector/assemble/config.json` and are
retuned per instance with `override_params` on `collector/assemble.params.<name>`, so this
talky can differ from the cogny next to it.

**Env knobs are an experimental surface.** Until this template's knobs move onto the `params`
block of the cells that read them, their names carry no compatibility promise and may change in
any `0.x` release; provider credentials keep living in `.env` either way. The migration is
tracked in [#138](https://github.com/mmeyerlein/meclaw/issues/138), with `collector@1`
([#136](https://github.com/mmeyerlein/meclaw/issues/136)) as the reference pattern.

| knob | where | default | unit |
|---|---|---|---|
| `KEEPER_IDLE_MS` | env | `7200000` | keeper -- silence before a generation may end (2 h) |
| `KEEPER_NIGHT_CRON` | env | `0 0,30 22-23,0-3 * * *` | keeper -- the sweep, **in UTC** (summer image of 00:00-05:30 CEST) |
| `KEEPER_CLOSE_LIMIT` | env | `50` | keeper -- generations one firing may seal |
| `window_turns` | param | `12` | collector -- newest turns entering the context |
| `window_bytes` | param | `8000` | collector -- byte cap over the window |
| `turn_chars` | param | `4000` | collector -- per-turn cap before the byte cap |
| `tool_chars` | param | `4000` | collector -- per-item cap on tool results |
| `round_bytes` | param | `16000` | collector -- byte cap over the whole tool round |
| `memory_chars` | param | `8000` | collector -- cap on the memory bundle |
| `max_iter` | param | `8` | collector -- **the loop bound**; at the cap the seam leaves on `answer` |
| `round_idle_ms` | param | `120000` | collector -- idle window of one tool round |
| `memory_tier` | param | `""` | collector -- empty = no memory leg at all |
| `memory_call_tier` | param | `"1"` | collector -- tier of the `memory_recall` tool; empty = tool off |
| `memory_form` | param | `"readable"` | collector -- `readable` / `json` / `both` |
| `prune_after_ms` | param | `604800000` | collector -- age gate on the prune lane (7 d) |
| `turn_write` | param | `""` | collector -- empty = off; set, the day leaves on route `turn_write` after every stored turn |
| `context_window` | param | `0` | collector -- the curator's budget in tokens; `0` = curation off. A channel voice is the shape the curator was **not** built for; leave it off unless the window is genuinely large. The full curator table is in [`collector`](../collector/#knobs) |
| `DISPATCHER_MAX_CALLS` | env | `16` | split -- per-answer call budget |
| `DISPATCHER_ASYNC_TOOLS` | (empty) | split -- comma-separated tools that answer on their own lane instead of inside the round (`consult_cogny,ask_memory,remember`). The key is colony-global, so in practice ONE list carries every async name of the tree |
| `SUMMARIZER_RECENT_TURNS` | `12` | summary -- newest turns travelling verbatim |
| `SUMMARIZER_PHASEOUT_CHARS` | `200` | summary -- per-turn cap on the phased-out turns |
| `SUMMARIZER_TOOL_CHARS` | `200` | summary -- per-item cap on tool previews |
| `SUMMARIZER_ROUND_LINES` | `40` | summary -- tool-activity lines at most |

**`ctx.model` is the one instantiation-class knob** and it is strict: `add_nodes` without
it is rejected with `ctx_key_missing`. Two equally valid forms (session ruling
2026-08-15): pass a **resolved literal** (the K-H2 builder convention — the builder
resolves `MODEL_<ROLE>` from `.env` itself), or pass the **`${MODEL_<ROLE>}` token**
verbatim so the cell re-resolves it from `.env` at spawn — the examples use the token
form to stay vendor-neutral. Both `llm` cells
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
- `crates/meclaw-cells/tests/w9a_per_turn_colony.rs` -- the per-turn lane in a colony
  that carries this composite, the shipped `memory-drain@1` and the memory hive's real
  write path: one turn, and the turn AND the answer are `episodes` rows before anything
  closes; then the close batch runs into the same drain and moves no row.
- `crates/meclaw-cells/tests/w10b_remember_colony.rs` -- the `remember` lane in a
  colony that carries this composite, the shipped `memory-drain@1` and the memory hive's
  real write AND extraction path: one turn whose single response carries the answer and
  the tool call, the answer in the channel untouched, and the fact a candidate on the
  episode of the turn it answered, under the drain's own `turn_id`. Plus the other half,
  which is the one that makes inline extraction defensible at all: a block with a broken
  payload leaves through `inline-reject`, writes nothing, covers no turn -- and the
  channel got its sentence anyway.
- The sub-units keep their own pins: `session_keeper.rs`, `collector_window.rs`,
  `collector_colony.rs`, `dispatcher_split.rs`, `summarizer_prep.rs`,
  `summarizer_colony.rs`.
