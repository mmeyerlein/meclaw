# `cogny@3.0.9`

The agent core as one template. Four units under one hive:
[`collector@2`](../collector/) and [`dispatcher@1`](../dispatcher/) -- each carrying its
template's own name -- plus **two** `llm` brains, `brain` on a thinking model and
`brain_fast` on a fast one. No new cell type, no Rust.

**Structurally a talky without a channel.** The advisor split (GH #28, R-CG-1) gives an
agent two brains: a fast [`talky@2`](../talky/) that owns the channel, and this one, which
owns the thinking. The core therefore carries no session keeper, no summarizer and no
proxy -- it has no channel, no sessions and no night. Its "conversation" is the errands
the channel voices send it, and the memory it reads is the member's central hive rather
than a window over one chat.

**One core, N channel voices.** Cogny is a *sibling* hive of the talkies at agent level
(`<agent>/{talky…, cogny}`), never a cell inside one and never one per talky (R-CG-2). Two
talkies consulting the same core is the normal shape. The memory and its archive are not in
that set: a `memory-hive` is the source of truth of a **member**, not of an agent, so it
sits beside the agent rather than inside it
(`<member>/{assistants/<agent>/{talky…, cogny}, memory, archive}`). Every agent the member
runs is a lens on the same hive, and a second one inherits what the member already knows.

## What it delivers

- **The seam, already bounded, and its own.** The collector hands the assembled errand to
  the brain over ONE edge carrying the iteration counter and `restore_ttl` -- a second
  copy of the mechanism the talky has, with its own bound, because a consultation is a
  longer round than a chat turn.
- **A tool round that only needs its tools.** `brain -> dispatcher -> (your tools) ->
  collector -> brain` is pre-wired except for the one lane that is genuinely
  per-instance: which cell answers to `web_search`. Adding a tool is one edge pair.
- **A consultation that looks like a turn.** The errand arrives on the collector's
  `in_turn` lane and is filed as the turn it is: the talky IS the core's user. Nothing in
  here knows that its user is a machine.
- **An answer that is an event.** The advice leaves on the ordinary `answer` route and
  becomes the asking talky's `in_advice` event -- which is why the same lane carries a
  *question back* without a second mechanism.
- **Nobody waits.** The consult is classified async at the talky's dispatcher
  (`DISPATCHER_ASYNC_TOOLS=consult_cogny,ask_memory`), so the talky's fan-in opens no
  expectation for it. Thinking time never races an idle window. That property lives on the
  *talky* side; this template is the half that is allowed to be slow.
- **And a lookup does not wait either (1.1.0).** The seam has two lanes. A memory question
  and a research question are two classes with two answer times, and one `llm` cell is one
  serial mailbox -- so the class picks a *cell*, not a parameter. See
  [The two lanes](#the-two-lanes-gh-124) below.

## Cells

| path | type | from |
|---|---|---|
| `collector/{assemble,window}` | `code`, `store` | `collector@2` **(sealed)** |
| `dispatcher` | `code` | `dispatcher@1` (a single-cell template) |
| `brain` | `llm` | this template -- the thinking lane |
| `brain_fast` | `llm` | this template -- the lookup lane (1.1.0) |

**The braces are an inventory, not an address list.** `collector` declares
`params.ports: []`, so `./collector` is the only address an edge from outside may name and
`./collector/assemble` is refused with `hive_port_boundary`; which cell inside takes the
message is decided by the `in_` lane the edge sets.

### How the sub-units are referenced: by name and version (GH #277)

The two sub-units are **references**, not copies. Each of the two directories holds one
`config.json` and nothing else:

```json
{"cell": {"type": "ref", "template": "collector@2.1.0"}}
```

At instantiation the referenced template's tree takes that position, so the instance is
byte-for-byte the tree the copies used to produce -- and every cell inside it now records
the template it really came from: `collector/assemble` is stamped `collector@2.1.0`, with
`cogny@3.0.9` above it in its provenance chain.

**The library has to carry both.** A reference resolves against the colony's template
registry, so `collector` and `dispatcher` have to sit in the same `templates/` directory
as `cogny` -- as they do in the shipped library. A tree that copied `cogny` alone gets
`template not found` at the mutation, not at boot.

**The version is pinned on purpose.** A bare `collector` would resolve to whatever the
highest version on disk happens to be, so a standalone bump would silently re-point this
composite. The pin makes the composite say which version it was built against; moving it
is a `cogny` bump, in the same commit.

Until GH #277 the sub-units lived here as byte copies of their `config.json` files, held
against their sources by a byte-identity pin. Its successor is
`crates/meclaw-colony/tests/gh277_composite_instantiation_is_byte_identical.rs`: the two
golden manifests prove the instantiated bytes did not move, and
`a_cell_inside_talky_is_stamped_with_its_own_template_and_names_talky_above_it` proves
the origin is recorded.

## Ports

**Two external ports, and the parent wires both in the SAME mutation that instantiates
the composite** -- an island without a crossing edge derives inactive. Both meet at the
hive path; five further lanes (`in_tool`, `in_bundle`, `tool`, `recall`, `error`) meet
there too and are wired per instance, see [Lanes](#lanes).

| port | endpoint | direction | what travels |
|---|---|---|---|
| consult ingress | `./cogny` | in | the errand on lane `in_turn`, carrying `context.consult_id` **and `context.consult_class`** |
| advice exit | `./cogny` | out | `hop.route == 'answer'` -- the advice **or** a question back |

```json
{"from": "<front>/talky", "to": "./cogny",
 "condition": "has(hop.route) && hop.route == 'tool' && has(hop.tool_name) && hop.tool_name == 'consult_cogny'",
 "modifier": {"set_hop": {"route": "'in_turn'"},
              "set_context": {"consult_id": "hop.consult_id",
                              "consult_class": "'consult'", "col_phase": "''"},
              "restore_ttl": true}},
{"from": "<front>/talky", "to": "./cogny",
 "condition": "has(hop.route) && hop.route == 'tool' && has(hop.tool_name) && hop.tool_name == 'ask_memory'",
 "modifier": {"set_hop": {"route": "'in_turn'"},
              "set_context": {"consult_id": "hop.consult_id",
                              "consult_class": "'lookup'", "col_phase": "''"},
              "restore_ttl": true}},
{"from": "./cogny", "to": "<front>/talky",
 "condition": "has(hop.route) && hop.route == 'answer'",
 "modifier": {"set_hop": {"route": "'in_advice'"},
              "set_context": {"col_phase": "''"},
              "restore_ttl": true}}
```

The ingress is TWO edges since 1.1.0 and they differ in exactly one literal. Everything
else about them is identical, on purpose: the class picks the lane, never the evidence.
Wiring only the first one is legal and gives you 1.0.0 behaviour -- without
`context.consult_class` every errand takes the thinking lane and `brain_fast` never sees a
message.

Five things in those edges are load-bearing, and four of them are not decoration:

- **`col_phase` must be cleared, on BOTH edges.** Each message leaves *another*
  collector's chain and carries whatever step that chain was in. A collector's `in_turn` /
  `in_advice` refuses a message that arrives mid-assembly, so the port edge resets the
  key. Everything else in the context rides along on purpose.
- **`consult_id` becomes context**, because the hop decays at the next cell and the
  correlation has to survive the core's whole chain and come home with the answer. A
  *fresh* consult is named by the call that opened it; a reply to a question the core
  asked back passes the id it was shown -- the dispatcher decides which, and both arrive
  here as the same key.
- **`consult_class` becomes context too**, for the same reason and read at the same place:
  the seam edge inside the composite is the only consumer, and by then the hop the class
  arrived on is three cells old.
- **`restore_ttl` on all three**, with the condition they already carry: an errand is a
  fresh journey, not the tail of the turn that started it, and the advice home is another.
- **The errand arrives as a `tool_call` turn.** Its text is the raw arguments the model
  wrote. The core's collector files that as the turn.

**The hive path is the address, and the lanes are the contract.** Since the seal
(GH #228) `params.ports` is empty: no edge from outside may name `./cogny/collector` or
any other cell in here, and every port above and every lane below meets at `./cogny`
itself. Which cells sit behind that path may be rearranged in a version bump without a
caller noticing -- that is what the seal bought. What may NOT change silently is the set
of lanes: dropping one or renaming one is a breaking change to every parent that wired it,
a CHANGELOG Breaking entry and a new major version, never a patch. Adding a lane nothing
ever promised is additive and takes the minor digit; giving a hive that shipped sealed the
contract it already implied is a repair and takes the third, which is what 3.0.1 was.

### Per-instance lanes (not ports of this template)

**Tools stay outside.** The tool set is the per-agent choice, so the composite carries no
tool cells and no map of them. Wiring a tool is one edge pair:

```json
{"from": "./cogny", "to": "./search",
 "condition": "has(hop.route) && hop.route == 'tool' && has(hop.tool_name) && hop.tool_name == 'web_search'"},
{"from": "./search", "to": "./cogny",
 "modifier": {"set_hop": {"route": "'in_tool'"}}}
```

`hop.route == 'tool'` is the lane; the `has()` guards are not decoration either, because
the `calls`, `result` and `answer` emissions carry no `tool_name` at all and an unguarded
comparison **errors** in CEL, which skips the edge with a log line per lane per message.

`escalate_to_deep` never leaves. The exit edge carries
`(!has(hop.tool_name) || hop.tool_name != 'escalate_to_deep')` so the reserved name stays
the composite's own lane between the two brains, exactly as it was before the tool lane
existed.

**The memory leg is the second pair**, and it is the one R-CG-1 moved onto this collector:

```json
{"from": "./cogny", "to": "<member>/memory",
 "condition": "has(hop.route) && hop.route == 'recall'",
 "modifier": {"set_hop": {"route": "'in_query'"}}},
{"from": "<member>/memory", "to": "./cogny",
 "condition": "has(hop.route) && hop.route == 'bundle'",
 "modifier": {"set_hop": {"route": "'in_bundle'"}}}
```

`params.memory_tier` on `./collector/assemble` is what decides how deep this core asks --
it lives here rather than on the talkies, and until 3.0.1 it had nowhere to ask.

**The error drain is one edge**, because the two brains are normalised onto the lane by
the exit edges themselves rather than by a cell:

```json
{"from": "./cogny", "to": "<parent>/drain",
 "condition": "has(hop.route) && hop.route == 'error'"}
```

**Wire it.** `talky` says the same thing about its own error lane and for the same reason:
undrained, a failed inference dead-letters as `no_route` and nothing upstream ever learns
the consultation died.

**What this composite still does NOT declare**, and it is a limit rather than an omission:

| wanted | state |
|---|---|
| memory tool (`in_memory_call` back into the collector) | not declared; inside the composite it would be a loop at the hive path, as `talky` does it |
| housekeeping (`in_prune`, `in_round_sweep`) | not declared |
| a normalising `errors` cell | R-CG-2 names "collector + dispatcher + llm, and nothing else" (the lookup lane made the llm slot two in 1.1.0); an `errors` cell is not among them, so the two brains are joined by the exit edges' `set_hop.route` instead. That is enough to make the failure reachable; it is not enough to give it a body a reader can grep, which is what `talky/errors` adds |
| the thread tool (`in_thread_call` back into the collector) | not declared. The sub-unit's collector **does** accept the lane (since `collector@2.0.1`, [#245](https://github.com/mmeyerlein/meclaw/issues/245)), but this composite draws no edge to it, so a `thread_recall` call leaves on the `tool` lane and finds no cell. Wiring it is a change to `params.graph` -- a parent cannot draw it, because the seal refuses an outside edge naming `./cogny/dispatcher` -- and the edge has to come with the same exclusion the tool exit already carries for `escalate_to_deep`, or the call fans out twice |

The tool SCHEMAS are a different thing again: they live in the brain's `system.tools`,
seeded (`brain/seed/system.jsonl`) or written by a system update. The composite carries
neither -- identity, core instructions and tools are the agent, not the topology.

## The two lanes (GH #124)

The measurement that produced this version: over eighteen provider calls, the substrate's
share of a consult's latency was **zero** -- one wire attempt, persist under 16 ms,
translate under 5 ms. The seconds were two things and neither was overhead.

1. **Generation.** A long answer takes as long as it takes to write.
2. **The serial mailbox.** An `llm` cell is a stateful cell: one long-lived task, one
   mpsc mailbox, strictly one message at a time (`docs/meclaw-overview.md`, the
   concurrency section). A trivial lookup arriving six seconds into a twenty-two-second
   research answer waited **15.5 s** for a call it finished in 2.5 s.

Both are answered by topology rather than by a knob. A second lane is a second mailbox, so
a lookup stops queueing; and the lane that only verbalises what the collector already
found can run a fast model under a short cap.

```
                                 context.consult_class == 'lookup'
collector ══════════════════════════════════════════> brain_fast
        ║                        (everything else)                  │
        ╚════════════════════════════════════════════> brain        │
                                                          │         │
                     dispatcher <─────(stop | tool_calls)──┴─────────┘
                            │
        escalate_to_deep ───┴──> collector  in_turn, consult_class := 'consult'
```

**The class is a tool name, not an estimate.** The asking model chooses between
`consult_cogny` and `ask_memory`; the ingress edge lifts that choice into
`context.consult_class`. `hop.consult_eta` -- the model's own coarse duration guess, GH
#123 -- stays **observe-only** and routes nothing: it is free text without a unit and
nobody has measured how well the model guesses. A tool name is a closed value set the
model declares, it is the documented place to sharpen the class boundary, and a later
Affinity holder inherits the lane by having one edge change its target.

**The class picks the lane, never the evidence.** Both lanes are fed by the same
collector, the same window, the same memory bundle -- the assembly happens *before* the
seam edge chooses. A misclassification can therefore cost a worse *formulation*, never a
wrong fact. That is the north star, and it is the reason this is a lane split rather than
a cheaper memory path.

**And when even the formulation will not do, the fast lane escalates.** It calls
`escalate_to_deep` with the question restated; the dispatcher routes that call by name like any
other, and one edge in this hive hands it back into the seam as a fresh turn with the
class flipped to `'consult'`. The deep lane then answers, one extra assembly later. A
misclassification costs about two seconds -- still cheaper than the fifteen this version
abolishes, and infinitely cheaper than a wrong answer.

> **Wire `escalate_to_deep` into `DISPATCHER_ASYNC_TOOLS`.** The escalation is answered on
> a lane of its own (a new turn), not inside the round it left. Declared async, the
> dispatcher names its id in `hop.async_calls`, the fan-in opens no expectation and the
> abandoned round is filed as already fired. Left undeclared, that round stays open until
> the idle exit and re-enters the seam for nothing.

## The internal wiring, edge by edge

Seventeen edges in this hive's `params.graph`, plus the four the sealed collector brings
with it -- those four are its own door and store edges and are neither drawn nor wireable
from here. Every edge below names `collector` by its HIVE path; the lane in the third
column is what the door behind it reads:

```
collector ==(brain, iter < 12, NOT lookup, restore_ttl)==> brain       <- THE SEAM,
collector ==(brain, iter < 12, lookup,     restore_ttl)==> brain_fast     two lanes
brain      --(stop | tool_calls)--> dispatcher
brain_fast --(stop | tool_calls)--> dispatcher
brain      --(length)-------------> collector  in_answer
brain_fast --(length)-------------> collector  in_answer

dispatcher --(calls)---> collector  in_calls
dispatcher --(result)--> collector  in_tool
dispatcher --(answer)--> collector  in_answer     -> and out of the advice port
dispatcher --(tool_name == escalate_to_deep)--> collector  in_turn, class := consult

.          --(in_turn)-----------> collector         THE DOORS
.          --(in_tool|in_bundle)-> collector
collector  --(answer)-----------> .                  THE EXITS
collector  --(recall)-----------> .
dispatcher --(tool, not escalate_to_deep)--> .
brain      --(error|content_filter)--> .  route := 'error'
brain_fast --(error|content_filter)--> .  route := 'error'
```

**The two seam conditions are complementary, and that is a correctness property, not
tidiness.** Fan-out copies a message to *every* matching edge -- two overlapping conditions
would run both brains on one errand and answer twice. The deep edge therefore carries
`(!has(context.consult_class) || context.consult_class != 'lookup')`: the `has()` is what
makes an unwired class fall to the thinking lane instead of erroring the edge away.

**`escalate_to_deep` is a reserved tool name inside this composite.** The edge above claims
it; a per-instance tool of that name would be swallowed by the escalation lane.

**The loopback bound is an edge literal, on purpose.** `int(hop.iter) < 12` is a safety
belt, not the policy: the round is bounded by `max_iter`, which ends a runaway
round with a message on the `answer` lane instead of a silence. The edge number only has
to be larger. Env substitution does not reach edge conditions -- a `${VAR}` there would be
registered verbatim and fail to parse as CEL -- so raising it is a mutation:
`remove_edges` first, `add_edges` second, in **two** mutations.

**`restore_ttl` sits on the seam, once per round.** `iter` counts brain answers, and a
bundle of fifteen calls is one answer, one iteration, one restore.

## Knobs

The collector's knobs are **params of `./collector`** (since `collector@1.2.0`):
they ship with their defaults inside the sub-unit copy and are retuned in the instantiated
tree, per core. The dispatcher's are still `${VAR:-default}` env literals that travel into the
instance and bind **late**, at every read -- and therefore move every unit in the colony at
once.

| knob | where | default | unit |
|---|---|---|---|
| `window_turns` | param | `12` | collector -- newest errands entering the context |
| `window_bytes` | param | `8000` | collector -- byte cap over the window |
| `turn_chars` | param | `4000` | collector -- per-turn cap before the byte cap |
| `tool_chars` | param | `4000` | collector -- per-item cap on tool results |
| `round_bytes` | param | `16000` | collector -- byte cap over the whole tool round |
| `memory_chars` | param | `8000` | collector -- cap on the memory bundle |
| `max_iter` | param | `8` | collector -- **the loop bound**; at the cap the seam leaves on `answer` |
| `round_idle_ms` | param | `120000` | collector -- idle window of one tool round |
| `memory_tier` | param | `""` | collector -- **the core's memory leg**; empty = no leg at all. Set it HERE, at the core, and nowhere else -- see below |
| `memory_call_tier` | param | `"1"` | collector -- tier of the `memory_recall` tool; empty = tool off |
| `memory_form` | param | `"readable"` | collector -- `readable` / `json` / `both` |
| `prune_after_ms` | param | `604800000` | collector -- age gate on the prune lane (7 d) |
| `turn_write` | param | `""` | collector -- per-turn episodes; empty = off. Belongs at the **talky**, not here -- see below |
| `context_window` | param | `0` | collector -- **the curator's budget in tokens**; `0`/empty = curation off. This is the knob the core wants and the channel voice does not: a cogny is exactly the shape the curator was built for (few turns, huge tool results), a talky is the other one |
| `curate_soft` / `curate_hard` | param | `0.5` / `0.75` | collector -- the working mark and the emergency mark, as fractions of the budget |
| `keep_rounds` | param | `2` | collector -- newest tool iterations kept verbatim whatever the budget says |
| `recoverability` | param | `""` | collector -- what may be elided, declared per tool NAME (`lookup:repeatable,write:env`). Undeclared = `unique` = never elided. **Declare the core's own tools here**, because the core is where the large results are |
| `thread_recall` | param | `"1"` | collector -- the `thread_recall` tool. **This composite does not wire it** and a parent cannot: the edge is `./dispatcher -> ./collector` on `hop.tool_name == 'thread_recall'` with `set_hop {"route": "'in_thread_call'"}`, which lives in this template's own `params.graph` and needs the same `escalate_to_deep`-style exclusion on the tool exit. Until it is drawn, the stubs the curator leaves have no way back -- so switching `context_window` on here means switching curation on without a recall path |
| `thread_recall_budget` | param | `0.2` | collector -- share of the budget one turn's recalls may spend; over it the call is refused, never truncated |
| `DISPATCHER_MAX_CALLS` | env | `16` | dispatcher -- per-answer call budget |
| `DISPATCHER_ASYNC_TOOLS` | env | (empty) | dispatcher -- the core's OWN async tools. **`escalate_to_deep` belongs here** (1.1.0); the `consult_cogny` / `ask_memory` declarations belong on the **talky** side. The key is colony-global, so in practice one list carries all three |

**Every `env` knob above and everywhere else in a cogny tree is an EXPERIMENTAL config
surface.** They are colony-global by construction and will follow the collector's knobs onto
`params`, one template at a time, in a 0.x release -- so treat a name in that column as
something that moves, not as a stable contract (`refs #138`).

**The sharp edge is gone (1.3.0).** Until `collector@1.1.0` every collector knob was a
colony-global env name, and because an env key is colony-global by construction, a `cogny` and
a `talky` in the same colony read the *same* `COLLECTOR_*` keys. R-CG-1 moves the memory leg to the core --
but setting `COLLECTOR_MEMORY_TIER` in `.env` turned it on at *every* collector in the tree,
including talkies whose `recall` port is not wired. The two ways out were "wire the talkies'
recall port too and pay for the extra leg" and "`override_params` on
`…/assemble.params.script_inline`", which was a fork of the script that the byte pin of the
day did not cover.

Now the knob is set where it belongs, and the sub-unit stays a reference to the standalone
`collector`:

```json
{"op": "instantiate", "template": "cogny@3.0.9", "at": "/cores/deep",
 "override_params": {"collector/assemble": {"memory_tier": "1",
                                            "context_window": 200000,
                                            "recoverability": "lookup:repeatable,write:env"}}}
```

The key is `collector/assemble`, not `collector`. Since
[#140](https://github.com/mmeyerlein/meclaw/issues/140) an `override_params` key is a
cell's path inside the template, and `collector` is a valid one -- it is the sealed
sub-unit's HIVE. A hive reads only `graph`, `ports` and `contract`, so the validator
accepts the key, nothing consumes the params, and the core comes up configured as if the
override had never been written. The knobs live one level down, on the `code` cell behind
the door.

**The curator knobs sat on the same edge and are now on the same footing.** A cogny core
wants `context_window` set -- it is the topology the curator exists for -- while a talky in
the same colony wants nothing of the kind. Set it at the core's `collector` and the
talky is untouched. `context_window` still defaults to off, so a core instantiated without
the override behaves exactly as before.

**`turn_write` was the sharper of the two, and it is the clearest win.** The core sees only
the turns of a consultation, the talky sees the conversation -- so the per-turn write belongs
on the **talky's** collector, exactly where the close batch already goes. Colony-wide it also
fired at the core's collector, whose `turn_write` route is either unrouted (a dead letter per
consult turn) or, worse, wired to the same drain, where a second session's turns land in
memory as if somebody had said them. Set it on the talky that owns the write route, and the
core never sees it.

**`ctx.model` and `ctx.model_fast` are the instantiation-class knobs** and both are
strict: `add_nodes` without either is rejected with `ctx_key_missing`. Two equally valid
forms (session ruling 2026-08-15): a **resolved literal** (the K-H2 builder convention --
the builder resolves `MODEL_<ROLE>` from `.env` itself), or the `MODEL_<ROLE>` token
verbatim so the cell re-resolves it at every read.

- `ctx.model` -> `brain`. A **thinking** model: the shipped `external_timeout_ms` is 300 s
  and `message_timeout` 400 s, sized for a model that reasons. A fast channel model here
  wastes the split.
- `ctx.model_fast` -> `brain_fast`. A **fast** model, the channel-voice class. Its shipped
  budget is the other end of the scale: `max_tokens` 512, `external_timeout_ms` 60 s,
  `message_timeout` 90 s. A lane that is allowed to think for five minutes is not a
  lookup lane, and the length cap is half of what makes it quick -- the other half is the
  `brevity` slot below.

**The one piece of system state this template ships** is `brain_fast/seed/system.jsonl`:
a single `brevity` leaf carrying the lane's length discipline and its escalation
instruction. It is a slot of its own, *not* `instructions`, precisely so the instance's
own `instructions` write never collides with it -- one writer per system path. Everything
else about the two brains should be seeded identically; a lookup lane that knows less than
the deep lane would answer differently, which is exactly what the class split must not
cause. A seed takes only on a FRESH birth: a `cell.db` that already exists means `Resumed`
and an inert seed.

> The seed row carries an `updated_at` column next to `slot_path` and `value`. It has to:
> instantiation stages the `seed/*.jsonl` files straight into the freshly created
> `cell.db`, whose `system` table declares `updated_at` `NOT NULL`. The `llm` cell's own
> boot-time loader ignores the column and stamps its own.

**The TTL budget.** With `restore_ttl` on the seam and on both port edges the colony
default of 64 carries the loop: only ONE round has to fit the budget.

## Instantiating it

```bash
curl -s -X POST http://127.0.0.1:PORT/colony/mutations -H 'Content-Type: application/json' \
  -d '{"scope":"/main/agent",
       "ctx":{"model":"anthropic/claude-opus-5","model_fast":"openai/gpt-4o-mini"},
       "diff":{
        "add_nodes":[{"name":"cogny","template":"cogny"}],
        "add_edges":[ ... the advisor ports plus the tool lanes, in the SAME mutation ... ]}}'
```

The composite comes up with five cells (plus two hive markers); the `store` and `llm`
cells report `active=true` + `NotYetSpawned`, which is the correct hot/cold form for a
stateful cell. Two things to have ready before the mutation:

1. **A `colony.db` whose three tables agree** (`registry`, `edges`, `hive_scopes`). A
   mutation that is REJECTED leaves a colony whose next boot panics (GH #89), so bring the
   edges in the same diff and check the table counts after any rejection.
2. **The core's identity**, either as `brain/seed/system.jsonl` (which only takes on a
   FRESH birth -- a `cell.db` that already exists means `Resumed` and an inert seed) or as
   a system update message. Neither is this template's business.

One line of the CALLER's identity is this template's business, though, because it decides
how often this core is woken for nothing. The persona of the talky in front of it has to
carry the boundary verbatim — see
[`../talky/README.md`](../talky/README.md) § The sentence a memory-carrying persona has to
contain. Without it the front model consults the core for what it was handed a moment ago
(#150): the answer comes back correct, so nothing looks broken, and every such question
costs a consult round trip instead of a direct reply. A core that is asked what the window
already says is not a slow core — it is a persona that was never told where its own
knowledge ends.

## What it is not

- **Not a channel voice.** No proxy, no chat id, no tone. Who talks to the user is the
  talky's job; the core answers the talky.
- **Not a session.** Nothing here mints a `session_id` or closes a generation. The
  `session_id` that rides in on the port edge is the *channel's*, and it is what keeps one
  consultation inside the conversation that asked for it.
- **Not a memory.** The recall leg is optional and the hive it asks belongs to the member
  the agent works for, not to this template and not to the agent.
- **Not a persona.** Identity, core instructions and tool schemas live in the brains'
  `cell.db`, one writer per `system` path -- and there are two of them now, so a system
  update that reaches only one lane is the drift to watch for. The shipped `brevity` slot
  is the single exception and it is deliberately not `instructions`.
- **Not a classifier.** Which lane an errand takes is decided outside this hive, on the
  ingress edge, from a tool name. Nothing in here inspects a question.
- **Not an initiator.** In v1 the talky triggers and the core answers; cogny never opens a
  conversation of its own (R-CG-3).
- **Not in the turn hot path.** A consultation is not a second seam inside a chat turn
  (R-CG-1, explicitly not v1). Reasoning-upgrading a single turn is an `override_params`
  on the talky's brain, not a cogny job.

## Pins

- `crates/meclaw-cells/tests/cogny_template.rs` -- the shipped template in a running
  colony against the mock OpenAI wire: a consult errand enters on the documented ingress,
  the core runs its OWN tool round, and the advice leaves on the return lane under the
  `consult_id` it was given. Since 1.1.0 also both lookup pins: an `ask_memory` errand
  reaches `brain_fast` under its own `max_tokens` with the `brevity` slot on the wire, and
  a fast lane that calls `escalate_to_deep` gets its answer from `brain` instead. The two
  lanes are told apart by the model id on the wire, which no edge can fake.
- `crates/meclaw-colony/tests/gh277_composite_instantiation_is_byte_identical.rs` -- the
  two golden manifests over the instantiated tree (the sub-unit refs produce the same
  bytes the copies did) plus the stamp pin: a cell inside a referenced sub-unit carries
  its OWN template and names the composite above it.
- `crates/meclaw-cells/tests/talky_cogny_advisor.rs` -- the other half: the bilateral
  advisor connection end to end, from a talky's interim answer to the correlated
  follow-up in the channel.
- The sub-units keep their own pins: `collector_window.rs`, `collector_colony.rs`,
  `dispatcher_template.rs`.

## Lanes

`params.ports` is empty (GH #228): the address is `./cogny` itself and what a caller wants
rides on `hop.route`.

| lane | direction | what travels |
|---|---|---|
| `in_turn` | in | a question for this core -- a consult or a lookup. `context.consult_class` picks the model tier |
| `in_tool` | in | one tool result, coming back from a tool cell the parent wired |
| `in_bundle` | in | a memory bundle, coming back from whatever keeps this agent's memory |
| `answer` | out | the core's answer, for whoever asked |
| `tool` | out | a tool call for a cell the parent wired; `hop.tool_name` says which one |
| `recall` | out | a memory read this turn needs |
| `error` | out | a failed inference on either brain. **Wire it** -- unwired it dead-letters, loudly |

Which cell takes the question, which one calls the tools and which one produces the
answer is this template's business and may change without a caller noticing. The two
brains are joined onto the single `error` lane by the exit edges' own `set_hop.route`,
because this composite carries no `errors` cell of its own -- see
[Per-instance lanes](#per-instance-lanes-not-ports-of-this-template).
