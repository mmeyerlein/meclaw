# `cogny@1.3.0`

The agent core as one template. Three units under one hive:
[`collector@1`](../collector/) as `collector`, [`dispatcher@1`](../dispatcher/) as
`split`, plus **two** `llm` brains -- `brain` on a thinking model and `brain_fast` on a
fast one. No new cell type, no Rust.

**Structurally a talky without a channel.** The advisor split (GH #28, R-CG-1) gives an
agent two brains: a fast [`talky@1`](../talky/) that owns the channel, and this one, which
owns the thinking. The core therefore carries no session keeper, no summarizer and no
proxy -- it has no channel, no sessions and no night. Its "conversation" is the errands
the channel voices send it, and its memory is the central hive rather than a window over
one chat.

**One core, N channel voices.** Cogny is a *sibling* hive of the talkies at agent level
(`<agent>/{talky…, cogny, memory, archive}`), never a cell inside one and never one per
talky (R-CG-2). Two talkies consulting the same core is the normal shape.

## What it delivers

- **The seam, already bounded, and its own.** The collector hands the assembled errand to
  the brain over ONE edge carrying the iteration counter and `restore_ttl` -- a second
  copy of the mechanism the talky has, with its own bound, because a consultation is a
  longer round than a chat turn.
- **A tool round that only needs its tools.** `brain -> split -> (your tools) ->
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
| `collector/{assemble,window}` | `code`, `store` | `collector@1` |
| `split` | `code` | `dispatcher@1` (a single-cell template) |
| `brain` | `llm` | this template -- the thinking lane |
| `brain_fast` | `llm` | this template -- the lookup lane (1.1.0) |

### How the sub-units are referenced: materialised copies, pinned

The substrate has **no template-in-template reference**. Instantiation is a recursive
directory copy (`docs/meclaw-overview.md` § Instanziierungs-Flow), and a `template.json`
inside the tree would only register a second template with the scanner. So the two
sub-units live here as **byte copies of their `config.json` files** -- no
`template.json`, no README, nothing patched.

That is a fork risk, and it is pinned rather than hoped away:
`crates/meclaw-cells/tests/cogny_template.rs` asserts every copied `config.json` is
byte-identical to its source template. A change to `collector@1` that does not travel
into `cogny/collector/` fails there, in the same test run, instead of drifting into
production.

## Ports

**Two external ports, and the parent wires both in the SAME mutation that instantiates
the composite** -- an island without a crossing edge derives inactive.

| port | endpoint | direction | what travels |
|---|---|---|---|
| consult ingress | `./collector/assemble` | in | the errand, lane `in_turn`, carrying `context.consult_id` **and `context.consult_class`** |
| advice exit | `./collector/assemble` | out | `hop.route == 'answer'` -- the advice **or** a question back |

```json
{"from": "<agent>/talky/split", "to": "./cogny/collector/assemble",
 "condition": "has(hop.tool_name) && hop.tool_name == 'consult_cogny'",
 "modifier": {"set_hop": {"route": "'in_turn'"},
              "set_context": {"consult_id": "hop.consult_id",
                              "consult_class": "'consult'", "col_phase": "''"},
              "restore_ttl": true}},
{"from": "<agent>/talky/split", "to": "./cogny/collector/assemble",
 "condition": "has(hop.tool_name) && hop.tool_name == 'ask_memory'",
 "modifier": {"set_hop": {"route": "'in_turn'"},
              "set_context": {"consult_id": "hop.consult_id",
                              "consult_class": "'lookup'", "col_phase": "''"},
              "restore_ttl": true}},
{"from": "./cogny/collector/assemble", "to": "<agent>/talky/collector/assemble",
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

### Per-instance lanes (not ports of this template)

**Tools stay outside.** The tool set is the per-agent choice, so the composite carries no
tool cells and no map of them. Wiring a tool is one edge pair:

```json
{"from": "./cogny/split", "to": "./cogny/search",
 "condition": "has(hop.tool_name) && hop.tool_name == 'web_search'"},
{"from": "./cogny/search", "to": "./cogny/collector/assemble",
 "modifier": {"set_hop": {"route": "'in_tool'"}}}
```

The `has()` is not decoration: the `calls`, `result` and `answer` emissions carry no
`tool_name` at all, and an unguarded comparison **errors** in CEL, which skips the edge
with a log line per lane per message.

Three more lanes, all of them the parent's decision:

| lane | endpoint | when |
|---|---|---|
| **memory** | `./collector/assemble` route `recall` out, lane `in_bundle` in | **this is where the memory leg lives now** (R-CG-1): the central hive is the core's memory, and `memory_tier` sits at THIS collector |
| memory tool | `./cogny/split` on `hop.tool_name == 'memory_recall'` → `./collector/assemble` lane `in_memory_call` | GH #78 -- one more tool edge, plus the recall pair above. `#88`'s query-hygiene guard exists for exactly this consumer |
| error drain | `./cogny/brain` **and `./cogny/brain_fast`** on `hop.finish_reason == 'error' \|\| hop.finish_reason == 'content_filter'` | a failed inference. Unlike `talky@1` this composite carries **no** `errors` cell (R-CG-2 names three units and nothing else); an unwired brain error dead-letters. Two brains, two drain edges -- forgetting the second one is the quiet way to lose the lookup lane's failures |
| housekeeping | `./collector/assemble` lanes `in_prune`, `in_round_sweep` | a timer; the template never fires them itself |

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
collector/assemble ══════════════════════════════════════════> brain_fast
        ║                        (everything else)                  │
        ╚════════════════════════════════════════════> brain        │
                                                          │         │
                          split <─────(stop | tool_calls)──┴─────────┘
                            │
        escalate_to_deep ───┴──> collector/assemble  in_turn, consult_class := 'consult'
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
`escalate_to_deep` with the question restated; the split routes that call by name like any
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

Ten edges in this hive's `params.graph`, plus the two the collector brings with it:

```
collector/assemble ==(brain, iter < 12, NOT lookup, restore_ttl)==> brain       <- THE SEAM,
collector/assemble ==(brain, iter < 12, lookup,     restore_ttl)==> brain_fast     two lanes
brain      --(stop | tool_calls)--> split
brain_fast --(stop | tool_calls)--> split
brain      --(length)-------------> collector/assemble  in_answer
brain_fast --(length)-------------> collector/assemble  in_answer

split --(calls)---> collector/assemble  in_calls      split --(tool)--> [your tools]
split --(result)--> collector/assemble  in_tool
split --(answer)--> collector/assemble  in_answer     -> and out of the advice port
split --(tool_name == escalate_to_deep)--> collector/assemble  in_turn, class := consult
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

The collector's knobs are **params of `./collector/assemble`** (since `collector@1.2.0`):
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
| `thread_recall` | param | `"1"` | collector -- the `thread_recall` tool; wire it at `./split` next to `memory_recall` (`hop.tool_name == 'thread_recall'` -> lane `in_thread_call`) or the stubs the curator leaves have no way back |
| `thread_recall_budget` | param | `0.2` | collector -- share of the budget one turn's recalls may spend; over it the call is refused, never truncated |
| `DISPATCHER_MAX_CALLS` | env | `16` | split -- per-answer call budget |
| `DISPATCHER_ASYNC_TOOLS` | env | (empty) | split -- the core's OWN async tools. **`escalate_to_deep` belongs here** (1.1.0); the `consult_cogny` / `ask_memory` declarations belong on the **talky** side. The key is colony-global, so in practice one list carries all three |

**Every `env` knob above and everywhere else in a cogny tree is an EXPERIMENTAL config
surface.** They are colony-global by construction and will follow the collector's knobs onto
`params`, one template at a time, in a 0.x release -- so treat a name in that column as
something that moves, not as a stable contract (`refs #138`).

**The sharp edge is gone (1.3.0).** Until `collector@1.1.0` every collector knob was a
colony-global env name, and because the sub-units are byte copies, a `cogny` and a `talky` in
the same colony read the *same* `COLLECTOR_*` keys. R-CG-1 moves the memory leg to the core --
but setting `COLLECTOR_MEMORY_TIER` in `.env` turned it on at *every* collector in the tree,
including talkies whose `recall` port is not wired. The two ways out were "wire the talkies'
recall port too and pay for the extra leg" and "`override_params` on
`…/assemble.params.script_inline`", which was a fork of the script that the byte pin did not
cover.

Now the knob is set where it belongs, and the byte pin still holds:

```json
{"op": "instantiate", "template": "cogny@1.3.0", "at": "/cores/deep",
 "override_params": {"collector/assemble": {"memory_tier": "1",
                                            "context_window": 200000,
                                            "recoverability": "lookup:repeatable,write:env"}}}
```

**The curator knobs sat on the same edge and are now on the same footing.** A cogny core
wants `context_window` set -- it is the topology the curator exists for -- while a talky in
the same colony wants nothing of the kind. Set it at the core's `collector/assemble` and the
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

## What it is not

- **Not a channel voice.** No proxy, no chat id, no tone. Who talks to the user is the
  talky's job; the core answers the talky.
- **Not a session.** Nothing here mints a `session_id` or closes a generation. The
  `session_id` that rides in on the port edge is the *channel's*, and it is what keeps one
  consultation inside the conversation that asked for it.
- **Not a memory.** The recall leg is optional and the hive it asks belongs to the agent,
  not to this template.
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
  Plus the byte-identity pin over the sub-unit copies.
- `crates/meclaw-cells/tests/talky_cogny_advisor.rs` -- the other half: the bilateral
  advisor connection end to end, from a talky's interim answer to the correlated
  follow-up in the channel.
- The sub-units keep their own pins: `collector_window.rs`, `collector_colony.rs`,
  `dispatcher_split.rs`.
