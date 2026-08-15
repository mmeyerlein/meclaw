# `collector@1.2.0`

Context assembly as a hive of existing cell types -- no new cell type, no Rust. Two cells:
`assemble` (a `code` cell, the state machine) and `window` (a `store` cell, the state).

The collector is the orchestrator of one agent's context window. It decides **what enters
the window and what leaves it**, in one place, and hands the result to the brain over
**one** message on **one** route.

## What it delivers

- **A rolling short-term conversation window.** Every inbound turn and every answer is
  written into a session-scoped, append-only table; the assembly reads the newest N back
  before it calls the brain. That is the whole "the agent only knows its current turn"
  gap: turn 3 can name what turn 1 said because the window carried it, not because a
  retrieval leg happened to find it.
- **The memory bundle, through one door.** With `memory_tier` set, every turn
  asks the memory hive once, and the bundle enters the context in `system.memory`. The
  collector renders nothing of its own; it only chooses which of the two forms the memory
  hive emitted travels on, and how much of it.
- **A memory the model can ask itself.** That ambient leg is fired before the model has
  seen the turn, so nothing in an agent could ever *decide* to ask about a **time range**
  (GH #78). The `in_memory_call` lane closes it: the brain emits a `memory_recall`
  `tool_call`, the dispatcher routes it by name like any other tool, and the cell behind
  that edge is the collector -- which serves the call on the recall port it already owns
  and answers it under the original `tool_call_id`. The round ends where it began.
- **The tool round, fanned back in.** The store-backed fan-in of the example pattern, with
  one difference that matters: the re-entry carries the conversation window and the memory
  bundle with it, because the round is assembled at the same place the context is.
- **Eviction as policy, not as judgement.** A turn cap that runs in the store, a byte cap
  that runs in the assembler, and the same discipline over the other two legs: a per-item
  cap on tool results, a byte cap over the whole round, a cap on the memory bundle. Whole
  turns and whole iterations leave, never halves; the turn being answered never leaves;
  and what left is reported on the hop rather than silently gone.
- **A bound on the round itself.** A model that keeps asking for tools is stopped by the
  seam that started the round (`max_iter`) -- not by a TTL that dies silently,
  and not by an edge that has to know about iterations.
- **A deterministic exit for a round that cannot complete.** A result lost in flight used
  to park the fan-in forever (GH #103). Now a round whose last progress lies behind
  `round_idle_ms` is closed at the next occasion: the missing calls get
  synthetic error results under their own `tool_call_id`, and the round fires through its
  regular route with `hop.round_stale=1`. Pure arithmetic, no model judgement.
- **A decided answer to the mid-round user turn.** A turn arriving while a round runs is
  written into the window but starts **no** second assembly: at most one open brain call
  per session. It rides with the next regular assembly, marked `hop.round_deferred=1`
  (see "Round robustness" below for the rejected alternative).
- **The closed session, handed on whole.** When a session keeper says a generation is
  over, the collector reads its own two tables back and emits the entire session as **one**
  batch on route `write`. Append-all, no judgement: what is extracted and what is kept
  belongs to whoever the edge leads to. The same multi-send writes a row into the
  `batched` ledger -- the delivery evidence the prune lane runs on.
- **Housekeeping with evidence, never on a hunch.** A long-lived session accumulates rows
  (GH #76). The `in_prune` lane cuts them -- but only rows of sessions whose close batch
  already **left** the collector (the ledger row is the proof) and whose delivery is older
  than `prune_after_ms`. Without a ledger row nothing is ever pruned: rather
  grow than silently lose a turn. The cut is reported on the hop, per session.

## Cells

| cell | type | what it holds |
|---|---|---|
| `assemble` | `code` | the whole state machine: eleven entry lanes, the fan-in gate, the eviction policy, the seam, the round-robustness exits, the prune chain |
| `window` | `store` | `turns` (the rolling conversation) and `round` (the per-turn slate: the assembled legs plus the tool round) -- both carry `session_id`, which is what makes them readable as a whole session at close time, and write times, which is what makes them prunable. Plus `batched`, the delivery ledger of the close lane. |

## Ports

Entry lanes go **into `./assemble`**. The parent edge names the lane with
`set_hop: {"route": "'<lane>'"}`; `session_id` rides in the message context.

| lane | who sends it | what it does |
|---|---|---|
| `in_turn` | the inbound surface (proxy, intake) | writes the turn, opens the assembly, asks memory |
| `in_advice` | an async tool's return lane (an advisor core), carrying `context.consult_id` | the SAME chain as `in_turn`, filed under role `advice`: an event that arrives after its turn ended and opens a fresh round |
| `in_bundle` | the memory hive's recall port | becomes the memory leg of this turn -- **or**, when the request carried a `memory_call_id`, the tool result of a `memory_recall` call |
| `in_calls` | the tool dispatcher | the assistant `tool_call` turn of the round; `hop.async_calls` names the ids this fan-in must **not** wait for |
| `in_tool` | a tool cell | one tool result |
| `in_memory_call` | the tool dispatcher, on `hop.tool_name == 'memory_recall'` | the memory tool: the collector serves the call itself (GH #78) |
| `in_thread_call` | the tool dispatcher, on `hop.tool_name == 'thread_recall'` | the thread tool: brings an elided payload of THIS turn back, uncapped, out of the collector's own slate (wave 11) |
| `in_answer` | the brain, on `finish_reason == 'stop'` | writes the answer into the window and lets it out |
| `in_close` | the session keeper, on `hop.route == 'close'` | reads the whole session back and batches it out |
| `in_prune` | a timer or an operator, on `hop.route == 'prune'` | prunes delivered-and-aged sessions; the template **never fires this itself** |
| `in_round_sweep` | a timer or an operator, on `hop.route == 'sweep'` | re-checks every open tool round and closes the stale ones; equally **never fired by the template itself** |

Exits leave **from `./assemble`** on `hop.route`:

| route | to | notes |
|---|---|---|
| `brain` | the agent LLM | THE seam. Promote `hop.turn_id`, `hop.session_id` and `hop.iter` to context on this edge. `system.consult.open` carries the correlation ids of the advice turns still in the window. Since wave 11 it also reports what the curator did: `hop.tokens_window`, `hop.tokens_projected`, `hop.tokens_estimated`, `hop.curate_mark`, `hop.curate_stage`, `hop.curate_elided`, `hop.curate_saved`. |
| `answer` | the reply sink | the brain's final turn, after it is in the window -- **or** a turn that reached `max_iter`, marked `hop.round_capped=1` |
| `recall` | the memory hive's recall port | the per-turn leg (only when `memory_tier` is set) **and** every `memory_recall` call; promote `recall_query`, `memory_tier`, `memory_call_id`, `recall_window_from`, `recall_window_to`, `session_id`, `turn_id`, `iter` |
| `write` | wherever a closed session belongs | one batch per close: `messages[]` the whole conversation, the raw round rows in the top-level slot `rounds` |
| `turn_write` | the SAME batch consumer, per turn | only with `turn_write` set: the day so far, after every stored turn and every stored answer -- the same document as `write`, without `rounds`. See "Per-turn episodes" below. |
| `prune` | a log sink or the operator surface | one report per pruned session (`hop.session_id`, `hop.pruned_turns`, `hop.pruned_rounds`, `hop.prune_boundary`) -- or a single zero report when nothing was eligible |
| `condense` | -- | **reserved, never emitted today.** The value is declared in the enum so the fold lane can be wired later without widening a published contract; nothing in this cell writes it. |

The enum itself is `contract.emits.hop.route` in `assemble/config.json` -- that declaration
is the authority, this table is its prose.

**`./assemble` is the port address, and the address is the contract.** Every lane in and
every route out crosses that one endpoint, and the working colonies under
[`../../examples/`](../../examples/) address it literally as `<parent>/collector/assemble`
-- it is a stable **address**, not implementation detail that happens to be reachable. The
second cell, `./window`, is internal and may be rearranged in a version bump; `./assemble`
may not, and moving it is a breaking change to every parent that wired it: a CHANGELOG
Breaking entry and a new major version, never a patch.

Wire the ports in the **same mutation** that instantiates the hive: an island without a
crossing edge derives inactive and never spawns.

## Knobs

Every knob below is a **param of `./assemble`**: it ships with its default in that cell's
`config.json` under `params`, and the script reads it off its stdin `params` object. Nothing
here reads the environment (since `collector@1.2.0`; see "The knobs are per instance" below
for how to retune one, and for what `override_params` can and cannot do).

| param | default | meaning |
|---|---|---|
| `window_turns` | `12` | how many of the newest turns enter the context. The cut runs in the store (`order by id desc`, `limit`). |
| `window_bytes` | `8000` | byte cap over the turn texts, counted from the newest turn backwards. Whole turns are dropped, never truncated. |
| `turn_chars` | `4000` | per-turn character cap applied before the byte cap, so one pathological turn cannot eat the window. |
| `tool_chars` | `4000` | per-item character cap on tool **result** texts before they enter the seam. |
| `round_bytes` | `16000` | byte cap over the whole tool round, counted from the newest iteration backwards. What does not fit falls as a whole **iteration**. |
| `memory_chars` | `8000` | character cap on the memory bundle as it enters `system.memory` -- the readable block and every `{text}` leaf of the machine-readable form. |
| `max_iter` | `8` | how often a turn may re-enter the brain with a tool round. At the cap the seam leaves on `answer` instead. |
| `round_idle_ms` | `120000` | idle window of one tool round (two minutes). A round whose last progress is older **and** whose fan-in is incomplete is closed at the next occasion with synthetic error results and fires with `hop.round_stale=1`. |
| `memory_tier` | `""` | empty = no memory leg at all, and the assembly waits for the window leg alone. `"0"` / `"1"` / `"2"` request that recall tier once per turn. |
| `memory_form` | `"readable"` | which form of the bundle reaches the brain: `readable` (the rendered block a model reads), `json` (the machine-readable bundle), `both`. Applies to the ambient leg and to a `memory_recall` result alike. |
| `memory_call_tier` | `"1"` | recall tier of the **memory tool** (GH #78). Configuration, never a model argument. Empty switches the tool off: a call is then answered with a typed error result instead of being asked into a void. |
| `async_tools` | -- | **not a collector knob.** The async class is declared once, at the dispatcher (`DISPATCHER_ASYNC_TOOLS`), and travels as `hop.async_calls`. |
| `prune_after_ms` | `604800000` | age gate on the prune lane (seven days). A session is pruned only when its close batch left **and** that delivery is older than this. |
| `turn_write` | `""` | empty = off, and nothing about the collector moves. Set, and every stored turn hands the day out again on route `turn_write`. Leave it off unless that route is wired: an unrouted emission per turn is a dead letter per turn. |
| `context_window` | `0` | **the curator's budget**, in tokens. `0` or empty = curation off and every byte of behaviour is the pre-wave-11 behaviour. See "The curator" below. |
| `curate_soft` | `0.5` | the working mark, as a fraction of the budget: at or above it the curator elides in stages until the projection fits under it again. |
| `curate_hard` | `0.75` | the emergency mark. It changes no behaviour of its own -- it is *reported* as `hop.curate_mark='hard'` and means the curator is out of stages. |
| `keep_rounds` | `2` | how many of the newest tool iterations stay verbatim whatever the budget says. |
| `recoverability` | `""` | what may be elided, **declared per tool name**: `read_file:repeatable,write_file:env`. Everything not named is `unique` and is never elided. |
| `thread_recall` | `"1"` | the thread tool. Empty switches it off and a call is answered with a typed error. Switching it off should mean switching curation off. |
| `thread_recall_budget` | `0.2` | the share of the budget one turn's recalls may spend. Over it the call is refused, never truncated. |

A knob set to `null` or to a blank string means "not configured" and falls back to the default
above, so an operator who empties a line gets the shipped behaviour rather than a dead cell.
The numeric knobs also accept their value as a string, which is what a `${VAR}`-substituted
param produces.

`collector` is the **reference migration** for this move: every other template's `${VAR}`
knobs are a declared EXPERIMENTAL config surface that follows the same route onto `params`,
one template at a time (`refs #136`, `refs #138`).

### The curator (wave 11)

Every knob above bounds **one item** or **one iteration**, and none of them has ever known how
large the window it was building actually is. A `tool_chars` of 4000 over twenty
rounds is 80 000 characters of tool output that no cap in this cell can see as a total. That
is the gap, and it is the gap that ends every long-running agent.

The answer here is deliberately **not** the one every coding CLI ships. Those fire one
threshold, make one model call, replace the history with prose, and have no way back;
Anthropic's own measurement of what survives such a fold is **3/3 high-level facts and 0/3
obscure details**. What runs here instead:

> The context window is not a history that is occasionally folded up. It is a **projection**
> over an append-only ledger, rebuilt deterministically at **every** assembly, with a
> recovery key for everything it leaves out.

**Continuous, not at a cliff.** The curator runs on every round and does nothing at all while
the window is comfortable (`hop.curate_mark='none'`). A procedure that only starts at 50-90 %
fill has to compress very much into very little, in one step, in the hot path, with one
attempt.

**Model-free, therefore drift-free.** Nothing is paraphrased, so nothing can be paraphrased
wrong. The failure class "the fold dropped exactly the detail I needed later" cannot occur
here, because no text is ever rewritten -- payloads are replaced by references, or they stay.

**Restorable.** Every stub carries its own way back:

```
[elided tool_result w0 - 4006 chars - tool=write_file - kind=env
 - sha256:2f5d65d98328 - recall: thread_recall(call_id="w0")]
```

The name is a **content hash**, so the same payload elided twice reads as the same reference
and deduplicates itself inside one window.

#### The three stages

Cheapest and safest first. After each stage the projection is measured again and the work
stops as soon as it fits: the curator does the least that is enough, and `hop.curate_stage`
reports how far it had to go.

| stage | what leaves | what stays |
|---|---|---|
| 1 | `tool_result` payloads whose class is `env` | the stub, the call, the tool name |
| 2 | `tool_result` payloads whose class is `repeatable` | the stub, the call, the tool name |
| 3 | `tool_call` **arguments** of old calls | the call and its name |

#### What is never touched, at any stage and any budget

- the **conversation window** -- every user and assistant turn, verbatim
- **`system.*`** -- instructions, handover, the memory bundle. It counts towards the budget
  and is never a candidate, which is exactly why hard constraints belong there and not in
  the chat: what is only in the conversation is compaction-mortal everywhere else
- the **`tool_call` name** of every round -- the action protocol (Anthropic's
  `clear_tool_inputs: false`). The record of *what* was done is what stops an agent from
  doing it a second time
- the newest **`keep_rounds`** iterations, verbatim
- every result whose class is **`unique`**

That list is the reason the invariance gate below holds. Constraints, obscure details and
time markers live in exactly those places, and the curator has no path to any of them.

#### `recoverability` is declared, never guessed

Three classes, and the choice between them is a statement about the **environment**, not
about the text:

- **`env`** -- the effect is already in the world (a file written, a row inserted, a message
  sent). The result text is a receipt.
- **`repeatable`** -- a read or a search. Running it again produces it again.
- **`unique`** -- not reproducible: a web search result, a person's words. **The default**,
  and never elided.

An undeclared tool therefore costs context, never correctness. That default is what makes the
whole scheme safe to switch on before every tool in a tree has been classified.

#### The trigger is a budget, not a counter

`hop.tokens_window` is the fill of the window **about to be sent**, against
`context_window`. Two sources, in this order:

1. **`hop.tokens_prompt`** -- the provider's own count, stored with the round it belongs to
   when it reaches this cell. The one number in the system that is not an estimate.
2. **A byte estimate**, marked `hop.tokens_estimated=1`, when no usage field arrived.

The estimate is a deliberate **lower** bound (`chars/4`; Anthropic's post-4.7 tokenizer
yields ~30 % more tokens for the same text). It can therefore fire too late, never too early
-- and it never fires on a window that did not need it.

**It never parks a turn.** The trigger is computed inside the cell rather than on a CEL edge,
because a threshold edge over an absent `hop.tokens_prompt` fails to evaluate, drops *both*
branches of an exact partition, and parks the turn in silence. A missing usage field costs an
estimate and a flag here, never a turn.

#### The curation-invariance gate

Nobody in the field measures multi-fold degradation; it is folklore with anecdotes. The gate
in `crates/meclaw-cells/tests/w11_curator.rs` measures it, in the three classes the research
names -- **constraint preservation**, **detail preservation** (Anthropic's 0/3 is the bar)
and **time-marker preservation** ("The Sleeping Agent": 3.05 % -> 62.39 % from a prompt fix
alone). It holds across four budgets, all three stages, and twice in a row.

It holds for a structural reason rather than a hopeful one: a second curation over an
already-curated window is a **fixed point**, because eliding an already-elided payload is a
no-op rather than a second lossy pass. That is the property a prose fold can never have.

### The thread tool (`thread_recall`)

A stub without a way back is a loss with better manners. `thread_recall` is the way back, and
it is the same bauform as the memory tool one section down: the model emits a `tool_call`, the
dispatcher routes it by `hop.tool_name`, and the cell behind that edge is **this** one --
because the collector owns the slate the stub points at.

```jsonc
{"from": "./split", "to": "./collector/assemble",
 "condition": "hop.route == 'tool' && hop.tool_name == 'thread_recall'",
 "modifier": {"set_hop": {"route": "'in_thread_call'"}}}
```

The tool schema is a **seed**, not a contract of this template -- what the brain may ask for
is decided where the brain's `system.tools` is written:

```jsonc
"thread_recall": {
  "description": "Bring back the full text of a tool result that was elided from this turn's context.",
  "parameters": {
    "type": "object",
    "properties": {
      "call_id": {"type": "string", "description": "the id printed in the [elided ...] stub"},
      "round":   {"type": "string", "description": "an iteration number, to get a whole round back"},
      "query":   {"type": "string", "description": "a substring to look for in this turn's results"}
    }
  }
}
```

Discipline, deliberately identical to the memory tool's:

- **It stays inside its own turn.** A recall reads the `round` table `where turn_id = <this
  turn>` and nothing else. What happened in an earlier session is `memory_recall`'s question,
  not this one's.
- **It answers uncapped.** A recall that re-applies the cap it was called to undo is theatre.
- **The budget is a wall, not a cap.** Over `thread_recall_budget` the call is
  *refused* with a typed error naming the number, because a silently halved recall is a lie
  about what the model was shown. Spend is counted per turn across all recalls.
- **A call that cannot be served is answered, never parked** -- with the tool switched off, or
  without a usable argument, a typed error result completes the fan-in.
- **One representation per prompt.** Once a payload has been recalled, the original result row
  is elided whatever its class says: the content stands in the window once, and the stub stays
  only as the *answer* to its call. Dropping the row would leave a `tool_call` unanswered,
  which every provider rejects.

### The knobs are per instance (since `collector@1.2.0`)

Until 1.1.0 every knob above was an **environment**-class substitution token
(`COLLECTOR_WINDOW_TURNS` and friends): it resolved from the root `.env`, and it resolved the
same way for every collector in the colony. Two `talky` instances and one `cogny` in one tree
read the *same* keys, which made a real production edge: `COLLECTOR_TURN_WRITE` set for the
talky that owns the conversation also fired at a cogny core whose `turn_write` route was
unrouted or, worse, wired to the same drain, and the budget of a thinking core is not the
budget of a channel voice.

Since 0.9.0 a `code` script receives a read-only, secret-filtered copy of its `params` on
stdin (`docs/cell-types.md` § `code`). **1.2.0 moves every knob there**, and the environment
route is gone -- there is no `COLLECTOR_*` fallback left to read. Three consequences:

1. **Per instance.** Instantiation is a directory copy, so every instance owns its own
   `assemble/config.json`. A value written there reaches that collector and no other, and two
   collectors in one colony are tuned apart without a fork of the script.
2. **Visible.** The values ship under `params` in that file, beside the `contract.settings`
   that document them, instead of living in an `.env` nobody exports.
3. **No script fork.** The old escape hatch -- overriding `…/assemble.params.script_inline`
   to rewrite the literals -- was a fork of the script that no byte pin covered. Setting a
   knob no longer touches the script at all.

```jsonc
// <root>/agents/deep/collector/assemble/config.json
"params": {
  "runner": "python3",
  "context_window": 200000,
  "recoverability": "read_file:repeatable,write_file:env",
  "turn_write": "1",
  …
}
```

**What `override_params` cannot do, and why.** `add_nodes[].override_params` is **rejected**
on a subtree template (`schema`, R10 ruling 2026-06-11: there is no sub-cell addressing, and
the earlier silent no-op was worse than a reject). `collector` is a two-cell subtree, so the
knobs cannot be set in the `add_nodes` entry that instantiates it -- neither before this
change nor after it. The tree writer sets them **in the instantiated tree**: write the values
into `…/assemble/config.json` after the mutation lands, or have the parent that owns the tree
write the file directly. `params` are read when the cell spawns, so the value is live from the
next boot of that cell.

A colony-global value is still reachable where one is actually wanted: a param may carry a
`${VAR}`-substitution token, which resolves at bootstrap and at mutation instantiation exactly
as before. The difference is that sharing is now a **choice made per instance** instead of the
only shape available.

`context_window` still defaults to *off* rather than to a number, but no longer because of the
sharing edge: a curator budget is a property of the model behind the seam, and inventing one
for a template that does not know its brain would be a guess.

### A cap is a preview, never a delete

Every cap in this hive is a **read-time cut**. The full tool result stays in the `round`
table, the full conversation stays in `turns`, and no cap ever removes anything -- a capped
value is a bounded preview of something the environment still holds, which is why the cut
is reported (`hop.round_dropped`, `hop.round_capped`, `hop.memory_capped`, next to the
existing `hop.window_*`) instead of happening quietly.

### Pruning: evidence first (GH #76)

Caps bound what a brain *sees*; they do not stop the two session tables from growing
without bound. The `in_prune` lane is the housekeeping answer, and it is **policy with
evidence**, decided on the three questions the issue asks:

- **Who prunes?** An explicit operator lane. The parent edge names it
  (`hop.route == 'prune'` → `set_hop {"route": "'in_prune'"}`); a parent tree typically
  wires a **timer cell** to it. The template itself carries **no schedule and never fires
  the lane on its own** -- whether a colony prunes at all is the parent's decision, not
  the collector's.
- **From when?** Only sessions whose day already **left this hive as a close batch**, and
  only after a grace period: the `close-fire` emission writes a row into the `batched`
  ledger (`session_id`, `batched_at`) **in the same multi-send** that carries the `write`
  batch out -- delivery and evidence are one emission. A prune request selects ledger rows
  older than `prune_after_ms` (default seven days) and cuts, per session, the
  `turns` and `round` rows recorded **up to that session's boundary**. A session without a
  ledger row -- however old -- is never touched.
- **What is the evidence?** The ledger row *is* it, and the boundary makes it precise: it
  is stamped with the close request's **arrival** time, so every turn the collector
  processed before the close is inside the batch (one actor, ordered mailbox) and every
  later turn is stamped younger and survives. The cut itself is reported on the hop
  (`hop.pruned_turns`, `hop.pruned_rounds`, `hop.prune_boundary`, `hop.session_id`, one
  report per session) -- and a prune that found nothing eligible answers with a zero
  report instead of silence.

Details that keep the policy honest:

- **A row the prune cannot date is a row it never cuts.** `round` rows carry
  `recorded_at` since this change; rows written before it (`NULL`) predate the policy and
  are left alone -- the failure mode is growth, never loss.
- **Used evidence is marked, not deleted.** After a cut the ledger row gets `pruned_at`,
  so the next prune does not re-spend it; the ledger itself is kept as the audit trail of
  what left and when. A later re-close of the same session writes a fresh row and starts
  a fresh seven days.
- **Prune is idempotent and race-tolerant.** Deleting up to a boundary twice deletes
  nothing the second time; two overlapping prune requests cannot cut past each other's
  evidence.

**Why this is not a No-Delete violation:** the substrate's No-Delete-Policy
(`docs/meclaw-overview.md` § No-Delete-Policy) is a statement about the **filesystem**:
"No file in `{root}` is ever deleted or moved." Store rows are cell state
*inside* a `cell.db` -- and `delete` is a first-class operation of the `store` cell type
(`docs/cell-types.md` § store). No file is deleted or moved by a prune; the durable record
of the session left with the batch, and R-OS-6 places it with the memory hive, not here.

### Round robustness (GH #103)

Two edge cases of the tool round, both real in production shape, both decided instead of
accidental:

**Partial returns.** The fan-in gate waits for `expected ⊆ received`. Tool cells answer
their own timeouts with typed error results, so the *common* failure completes the round --
but a message lost in flight (a tool dying mid-restart) used to park the round forever,
and the iteration cap cannot help: it counts at fire time, and a round that never fires is
never counted. The exit is a **round idle window**, pure arithmetic:

- A round's **progress** is the newest `recorded_at` of its slate rows -- the round start
  (the assistant row) or the last result that arrived. The state is derived entirely from
  the existing `round` table; nothing new is stored.
- A round whose progress lies behind `round_idle_ms` **and** whose fan-in is
  incomplete is closed at the **next occasion**: each missing call gets a synthetic
  `tool result lost` result under its own `tool_call_id` (the dispatcher-lid pattern --
  the brain sees the failure and has to answer it), the fan-in completes, and the round
  fires through the **regular** guard and seam -- `brain` or `answer` exactly as the
  existing logic decides -- with `hop.round_stale=1`.
- An **occasion** is any message that reaches the cell anyway: the next result of the
  round, the next user turn of the session, or an `in_round_sweep` request. The template
  has **no timer of its own**; a parent tree that wants a guaranteed occasion wires a
  timer cell to the sweep lane (the session-keeper pattern):
  `hop.route == 'sweep'` → `set_hop {"route": "'in_round_sweep'"}`.
- A late real result **wins** over its synthetic stand-in: the store keeps both rows, the
  wire carries one result per call id, and the emission then does not call itself stale.
- A round the policy cannot date (rows from before `recorded_at`) keeps its pre-#103
  behaviour -- it parks, and it does not defer anybody (the R-P3 direction: what the
  policy cannot date, the policy leaves alone).

**Mid-round user turns.** A turn arriving while a round of its session is open used to
start a second assembly -- two interleaved brain calls on one channel, answers crossing in
undefined order. Decided behaviour now:

- The turn is written into the window (nothing is ever lost) and **stamped**
  (`turns.deferred = 1` -- a lifecycle bit like `round.fired`, never content). Its parked
  arrival says so on the hop (`round_deferred=1`) and starts **no** assembly.
- It **rides with the next regular assembly** after the round is over: the next window
  read carries it, the seam marks that arrival `hop.round_deferred=1`, and the stamp is
  cleared in the same multi-send -- the flag marks the arrival, not every later window
  that still contains the turn.
- Consequence: at most **one open brain call per session** -- the telephone model
  (R-OS-3): you answer when the sentence is finished.
- **Rejected alternative -- allow the second assembly.** Humans do answer two messages at
  once, so it was considered. Rejected because two interleaved rounds on one session
  share one fan-in slate keyed by session and iteration, their answers cross at the
  surface in undefined order, and the second brain call pays full context cost for a
  question the first call is often about to answer anyway. A colony that truly wants
  parallel questions runs them as parallel *sessions* -- that is what `session_id` is for.
- Known limit, on purpose: a deferred turn whose session never speaks again waits until
  the next turn or the session close -- it is answered *with* the next exchange, not by a
  timer. The mid-round turn also still fires its recall request (the memory leg is asked
  before the open round is known); an unused bundle row is harmless and leaves with the
  close batch.

The check costs the first assembly of every turn one extra store round-trip (open-round
select), two routing hops -- the tool round itself is unchanged.

### The memory tool (GH #78)

The per-turn leg above is the **free floor**: it is fired the moment a turn arrives, at a
fixed tier, before the model has read a word of it. That covers the ambient case and it
cannot cover the other one -- a question about a **time range**. The recall cell has
understood `recall_window_from` / `recall_window_to` since P15, but nothing in an agent
could ever *decide* to send them, because nobody who had seen the turn was ever the one
asking. The memory tool is that missing producer, and it sits at the **consumer**.

**From the dispatcher's side it is a tool like any other.** The brain emits a `tool_call`
named `memory_recall`, the dispatcher routes it by `hop.tool_name`, and an edge knows the
cell -- exactly as for a web search. The dispatcher learns nothing: routing is fan-out,
and this is fan-out. **From the collector's side it is the one tool it serves itself**,
because it is the memory specialist of this hive already (it owns the recall port for the
per-turn leg, R-OS-5). So the round ends in the collector and memory never learns a word
of dispatcher vocabulary (R-OS-2).

Two edges, and neither of them is new machinery:

```jsonc
// 1. the dispatcher's memory lane -- the same shape as any tool edge
{"from": "./split", "to": "./collector/assemble",
 "condition": "hop.route == 'tool' && hop.tool_name == 'memory_recall'",
 "modifier": {"set_hop": {"route": "'in_memory_call'"}}}

// 2. the recall port the per-turn leg already used, carrying five keys now
{"from": "./collector/assemble", "to": "<memory hive>/recall",
 "condition": "hop.route == 'recall'",
 "modifier": {"set_context": {"recall_query": "hop.recall_query",
                              "memory_tier": "hop.memory_tier",
                              "memory_call_id": "hop.memory_call_id",
                              "recall_window_from": "hop.recall_window_from",
                              "recall_window_to": "hop.recall_window_to"}}}
```

`memory_call_id` is the whole correlation: the ambient leg travels the same edge with the
key **empty**, and the returning bundle is filed as the memory leg of the turn; a bundle
that comes back with the key set is filed as the `tool_result` of that call, under the
original `tool_call_id`, on the ordinary `round-w` phase. One port, two meanings, told
apart by what the request carried out. Every key is always present and empty rather than
absent -- a missing hop key makes the promoting CEL modifier fail, and a failed modifier
skips the edge.

The tool schema is a **seed**, not a contract of this template -- what the brain may ask
for is decided where the brain's `system.tools` is written:

```jsonc
"memory_recall": {
  "description": "Ask long-term memory about something, optionally restricted to a time range.",
  "parameters": {
    "type": "object",
    "properties": {
      "query":       {"type": "string", "description": "what to look for"},
      "window_from": {"type": "string", "description": "ISO-8601 start of the time range (optional)"},
      "window_to":   {"type": "string", "description": "ISO-8601 end of the time range (optional)"}
    },
    "required": ["query"]
  }
}
```

The collector reads exactly those three argument names, passes the window through as the
recall port's own keys, and takes the **tier** from `memory_call_tier`. A tier
is a cost decision of the tree, not something a model gets to raise from inside a prompt.

Discipline, unchanged in every direction:

- **It counts as a normal call.** The `memory_recall` id is a member of the round's
  expectation set like any other, `tool_chars` cuts its result like any other,
  `round_bytes` and `max_iter` bound the round it belongs to.
- **A call that cannot be served is answered, never parked.** With
  `memory_call_tier` empty the collector answers the call itself with a typed
  error result -- the dispatcher-lid pattern, one lane further in.
- **A port that is not wired ends in the idle exit.** Without the `recall` edge the
  request is unroutable and no answer ever comes; the round then parks and is closed by
  the round idle window of GH #103 (synthetic result, `hop.round_stale=1`) -- the same
  exit a tool that died mid-flight gets. No second machinery for a memory tool.
- **The ambient tier-0 bundle stays the free floor.** It does not step aside when the
  model asks for itself: the two are different questions (what is always true about this
  person vs. what happened between these two dates), and a turn that pays for both is a
  turn whose model asked for the second one on purpose.

### Per-turn episodes (`turn_write`)

A closed session leaves on `write`. That is the right shape for whoever keeps the day --
and the wrong *cadence* for a memory: until the session closes, nothing the user said is
retrievable, and a question about the last exchange gets answered out of an empty store.
The lane closes that hole without a single model call.

Switched on, two moments hand the day out again on route `turn_write`: the echo of the
stored **turn** and the echo of the stored **answer**. What leaves is the same document
the close lane produces -- the same table, the same order, the same role mapping -- minus
the `rounds` slot, which only a close needs.

```
in_turn  -> insert turns row -> (round check)  +  select turns of the session -> ROUTE turn_write
in_answer -> insert turns row                  +  select turns of the session -> ROUTE turn_write
```

**Wire it to the same consumer the `write` route feeds**, and to nothing else. Two
properties depend on that and neither is decorative:

- The consumer sees the *same* conversation in the *same* order on both routes, so
  whatever it derives per turn (an episode id, an index) it derives identically at close
  time. A second consumer with its own numbering is a second memory, not a faster one.
- The consumer's own idempotence -- not the collector's -- is what makes the repetition
  free. The day is handed out *whole* every time; a consumer that recognises what it has
  already taken writes only the difference. `memory-drain@1` is built exactly that way.

The close route keeps its consumers and its cadence. It becomes the **safety net**: a turn
the per-turn lane lost (a restart, a lane switched on mid-session) is still in the close
batch, and the count gate over that batch is what proves nothing went missing.

Uncapped by design, like the close lane: the knobs above bound a *context window*, and
this is the durable record leaving. A byte cap here would silently renumber the day.

### `window` here is a context window, not a recall window

The `window` cell holds the turns an agent is about to *see*. The memory hive's "recall
window" is a **time range** it is asked about (`recall_window_from` / `_to`) -- a different
thing under a similar word. Nothing in this hive is retrieved by similarity or by date; it
is retrieved by recency, and that is the whole contract.

### The TTL budget (GH #82)

The tool round of this hive is a read-modify-write conversation with `window`, and every leg
of it is a routing hop that decrements `ttl`. A tool round therefore costs about **twelve**
hops, so the colony-wide default of 64 holds roughly **five** rounds and the sixth dies mid
fan-in -- terminal, straight to the dead-letter queue, with nothing emitted toward the
origin. Two ways out, and the first is the recommended one:

1. **Let the re-entry edge restore the budget.** Since the `restore_ttl` ruling
   (2026-08-13) an edge may declare `"modifier": {"restore_ttl": true}`; colony then lifts
   the follow-up's `ttl` back to `message_default_ttl` when that edge takes a message. The
   loop then only has to fit **one** round into the budget instead of all of them, and the
   colony default can stay at 64. The substrate **refuses a restoring edge without a
   `condition`**, so put it on the loopback edge next to the iteration counter that edge
   already carries.
2. **Size the budget instead.** For a shape that does not restore:
   `message_default_ttl >= 4 + rounds * 12` in the instantiating colony's `colony.json`.

A `memory_recall` call rides on top of that: the request leaves the hive, crosses the
memory hive's own chain and comes back, so it costs whatever that hive costs plus the four
hops of this one (dispatcher edge, recall edge, return edge, the `round-w` write). With a
restoring re-entry edge that is still one round's worth of budget; without one, size for it.

Either way, TTL is not what bounds the round. This hive bounds it itself with
`max_iter`, which is why a runaway round ends in a message on the `answer` lane
rather than in a silence. Hop table and derivation:
[`docs/store-backed-tool-loop.md`](../../docs/store-backed-tool-loop.md).

## The protocol, row by row

A turn runs through the `round` table twice: once to assemble, once per tool round.

```
in_turn   -> insert turns(user)          phase turn-w
          -> [recall request]            (only with a memory tier)
turn-w    -> select round (open rounds)  phase turn-open    <- GH #103
turn-open -> no open round:
             select turns (limit N)      phase win
          -> open round: update turns
             set deferred=1              phase defer-w      <- the turn parks
             (+ per stale round: the round-check below)
win       -> insert round(leg-window)    phase collect      <- the byte cap runs here
in_bundle -> insert round(leg-memory)    phase collect
collect   -> select round                phase gate
gate      -> update round set fired=1    phase fire-guard   <- exactly-once guard
fire-guard-> select round                phase fire
fire      -> ROUTE brain                                    <- the seam
          (+ update turns set deferred=0 phase defer-clear, when a deferred
             turn travelled: round_deferred=1 marks the arrival)
```

and, when the brain asked for tools:

```
in_calls  -> insert round(assistant)     phase round-w
          -> per async id: insert
             round(tool, acknowledged)   phase round-w      <- R-CG-3: no expectation
             (the assistant row is written fired=1 when NOTHING else was asked)
in_advice -> insert turns(advice)        phase turn-w       <- the return lane, into
             (+ recall request)                                the turn chain above
in_tool   -> insert round(tool)          phase round-w
in_memory_call -> ROUTE recall           (memory_call_id = the tool_call_id,  <- GH #78
                                          recall_window_from/_to = the args)
in_bundle (with a memory_call_id)
          -> insert round(tool)          phase round-w      <- back in the regular fan-in
round-w   -> select round                phase round-check
round-check-> complete: update round
             set fired=1                 phase round-guard  <- per ITERATION
          -> incomplete + idle: insert
             round(tool, 'tool result
             lost') per missing call     phase round-w      <- GH #103, back into
round-guard-> select round               phase round-fire      the regular fan-in
round-fire-> ROUTE brain (iter + 1)                         <- the same seam
          -> ROUTE answer (round_capped) <- at max_iter, instead of the brain
```

and, when a timer (or an operator) asks whether a round is stuck:

```
in_round_sweep -> select round (assistant, fired=0)  phase sweep
sweep          -> per stale round: select round      phase round-check
                                                     <- the regular re-check, under the
                                                        round's own turn/iter/session
```

and, when a session ends:

```
in_close   -> select turns (session, asc) phase close-turns <- the hop id carries the
close-turns-> insert round(leg-close)     phase close-w        arrival time: the prune
close-w    -> select round (session)      phase close-fire     boundary of this close
close-fire -> ROUTE write                                   <- one batch, append-all
           +  insert batched(session, B)  phase close-ledger<- the delivery evidence,
                                                               in the SAME multi-send
```

and, when a timer (or an operator) asks for housekeeping:

```
in_prune    -> select batched (aged, unused)  phase prune-ledger
prune-ledger-> per session: delete turns      phase prune-t  <- boundary in the hop id;
prune-t     -> delete round (same boundary)   phase prune-r     no ledger row -> zero
prune-r     -> update batched set pruned_at   phase prune-mark  report, NO delete
            +  ROUTE prune (the report)                     <- pruned_turns / _rounds
```

An incomplete fan-in and a lost guard race emit **nothing** (empty multi-send, terminal by
design) -- the same discipline as the store-backed tool loop this grew out of.

## The async class and the return lane (GH #28, R-CG-3)

A tool that thinks does not fit inside a round. An advisor core answers in minutes; a
fan-in that waited for it would be betting `round_idle_ms` against thinking
time, and losing that bet writes "tool result lost" into the transcript. So the round
does not wait at all:

1. **The dispatcher classifies.** `DISPATCHER_ASYNC_TOOLS=consult_cogny` makes the
   dispatcher name the affected `tool_call_id`s in `hop.async_calls` on the `calls` lane.
   One declaration, in the one cell that sees the whole bundle.
2. **The collector opens no expectation.** Each named id is answered on the spot with a
   plain `tool_result` under its own `tool_call_id` -- the assistant turn stays
   well-formed for every provider -- and when *nothing else* was asked, the assistant row
   is written `fired=1`. There is no open round: no guard to win, nothing for
   `in_round_sweep` to find, no idle exit. The turn ends with the interim answer the
   dispatcher already sent to the channel.
3. **The answer comes back as an event.** Whatever the advisor produces -- a result, or a
   question back -- arrives on `in_advice` with `context.consult_id`. It runs the *turn*
   chain: written into the window under role `advice`, memory leg fired like on any turn,
   gate closed, seam fired. The brain sees a fresh round and verbalises the follow-up in
   the channel's own voice.
4. **The reply finds its thread.** Every advice turn still in the window contributes its
   id to `system.consult.open`. The model passes one back in its next consult call
   (`arguments.consult_id`), the dispatcher promotes it to `hop.consult_id`, and the
   advisor keeps one thread across question and answer.

An `advice` row is inbound on the wire (`origin: user`), because that is the only inbound
role a provider knows. In the store it keeps its own role, so a batch and a prune can
still tell an event from a user's word.

```json
{ "from": "./split", "to": "/agent/cogny/collector/assemble",
  "condition": "has(hop.tool_name) && hop.tool_name == 'consult_cogny'",
  "modifier": {"set_hop": {"route": "'in_turn'"},
               "set_context": {"consult_id": "hop.consult_id"},
               "restore_ttl": true} },
{ "from": "/agent/cogny/collector/assemble", "to": "./collector/assemble",
  "condition": "has(hop.route) && hop.route == 'answer'",
  "modifier": {"set_hop": {"route": "'in_advice'"}, "restore_ttl": true} }
```

## What it is not

- **Not a persona.** Identity is not context assembly. A persona cell upstream still owns
  `system.identity`, and the brain edge stays one seam so a later split of the agent can
  happen behind it without re-cutting the collector.
- **Not a dispatcher.** Routing a `tool_call` to the right tool is a fan-OUT; the collector
  only fans results back in.
- **Not a session keeper.** It consumes a `session_id` and it answers a close request; it
  does not decide when a session begins or ends.
- **Not memory.** The collector reads the recall bundle and hands the closed session on
  unfiltered, but what is worth remembering out of that batch is the receiver's question,
  never the collector's. It **serves** the `memory_recall` call (GH #78) because it owns
  the port and the round, and it answers it with what the memory hive said, verbatim up to
  the cap -- it retrieves nothing, ranks nothing and remembers nothing of its own.
- **Not an eager deleter.** Every *cap* is a read-time cut; rows fall only on the
  `in_prune` lane, only with delivery evidence in the `batched` ledger, only behind the
  age gate, and never by the collector's own initiative -- the lane has no schedule of
  its own (see "Pruning: evidence first").

## Pins

- `crates/meclaw-cells/tests/collector_window.rs` -- the shipped `script_inline` against
  real stdin documents: assembly, the eviction policy, the caps, the gate, the seam and
  its bound, the close lane, the delivery ledger, the prune chain, the round idle exit,
  the mid-round deferral and the memory tool (the request, its window arguments, the
  answer as a tool result, the switched-off tier).
- `crates/meclaw-cells/tests/w9a_per_turn_episodes.rs` -- the per-turn lane at script
  level: the two occasions, the lane switched off by default, the day it hands out, and
  the proof that the `turn_write` document and the `write` document are the same
  conversation.
- `crates/meclaw-cells/tests/collector_colony.rs` -- a running colony with no memory hive
  in it at all, so a turn that references an earlier turn can only have been answered from
  the window; plus a 100 KB tool result that arrives capped, a runaway round that the seam
  ends, a session that leaves as one batch, a batched session that is pruned while
  the living session keeps every byte, a lost tool result whose round a sweep closes, and
  a mid-round turn that defers and rides with the next assembly. Two more trees run the
  memory tool against the **shipped `dispatcher@1`** -- one with the recall port wired
  (both results fan in, the model's time range reaches the request) and one without it
  (the call is unroutable and the round ends in the idle exit).
