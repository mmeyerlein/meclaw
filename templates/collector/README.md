# `collector@3.0.2`

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
- **The memory bundle, as the evidence of a round.** With `memory_tier` set, every turn
  asks the memory hive once, and what comes back enters the round as a synthetic
  `memory_recall` **pair** at the end of `messages[]`: a `tool_call` nobody emitted, under
  a call id derived from the bundle itself (`call_recall_` + a sha256 over `as_of` and the
  query, so a re-assembly of the same turn is the same call and not a second question; a
  tier-0 bundle carries no `as_of` at all -- it is a deterministic projection, not a dated
  lookup -- so the hash falls back to the rendered block, and two tier-0 turns whose bundle
  renders identically share an id, which is the honest reading: it is the same evidence,
  handed over again), and the bundle as its `tool_result` (`collector@2.1.0`,
  [#278](https://github.com/mmeyerlein/meclaw/issues/278)). What is left under
  `system.memory` is the **revocation** of the slot the bundle used to occupy, and nothing
  else. The collector renders nothing of its own; it only chooses which of the two forms
  the memory hive emitted travels on, and how much of it.
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

**This hive is sealed.** `config.json` declares `params.ports: []` (GH #228), which is the
SEALED state: the hive path is the only address, and a mutation naming a cell inside it --
`./assemble`, `./window`, either of them -- is refused with `hive_port_boundary`. What a
caller wants rides on `hop.route`, and the lanes it may use are the ones `params.contract`
declares.

Entry lanes therefore address the **hive path** and name themselves on the hop. The parent
edge names the lane with `set_hop: {"route": "'<lane>'"}`; `session_id` rides in the
message context.

| lane | who sends it | what it does |
|---|---|---|
| `in_turn` | the inbound surface (proxy, intake) | writes the turn, opens the assembly, asks memory |
| `in_advice` | an async tool's return lane (an advisor core), carrying `context.consult_id` | the SAME chain as `in_turn`, filed under role `advice`: an event that arrives after its turn ended and opens a fresh round |
| `in_bundle` | the memory hive's recall port | becomes the memory leg of this turn -- **or**, when the request carried a `memory_call_id`, the tool result of a `memory_recall` call |
| `in_calls` | the tool dispatcher | the assistant `tool_call` turn of the round; `hop.async_calls` names the ids this fan-in must **not** wait for |
| `in_tool` | a tool cell | one tool result: **every** `tool_result` turn of its `messages[]`, each filed under the call id it answers. See "What a tool result may carry" below |
| `in_memory_call` | the tool dispatcher, on `hop.tool_name == 'memory_recall'` | the memory tool: the collector serves the call itself (GH #78) |
| `in_thread_call` | the tool dispatcher, on `hop.tool_name == 'thread_recall'` | the thread tool: brings an elided payload of THIS turn back, uncapped, out of the collector's own slate (wave 11) |
| `in_answer` | the brain, on `finish_reason == 'stop'` | writes the answer into the window and lets it out |
| `in_close` | the session keeper, on `hop.route == 'close'` | reads the whole session back and batches it out |
| `in_prune` | a timer or an operator, on `hop.route == 'prune'` | prunes delivered-and-aged sessions; the template **never fires this itself** |
| `in_round_sweep` | a timer or an operator, on `hop.route == 'sweep'` | re-checks every open tool round and closes the stale ones; equally **never fired by the template itself** |

Exits leave **from the hive path** on `hop.route`:

| route | to | notes |
|---|---|---|
| `brain` | the agent LLM | THE seam. Promote `hop.turn_id`, `hop.session_id` and `hop.iter` to context on this edge. `system.consult.open` carries the correlation ids of the advice turns still in the window -- **always**, empty included (`collector@2.0.3`): the `llm` cell upserts `system.*` per slot path, so a path that is not sent is a path that is not touched, and a slot that is only ever set keeps naming a consultation that closed long ago. `system.memory` follows the same rule and, since `collector@2.1.0`, carries nothing but that rule: the bundle itself is no longer anywhere in that subtree (GH #278) -- it travels as the `memory_recall` tool result at the end of `messages[]`. What the collector still sends there on every turn is the revocation, unconditionally and no longer tied to `memory_form`: an empty `text` on the FIXED path `system.memory.recall`, which clears a bundle an older collector may have left standing and contributes nothing to the system prompt, plus the `"$replace": true` marker on the whole `system.memory` node (`collector@2.0.4`, GH #264), which is what lets it revoke the `json` form's keys -- named by the memory hive per bundle, and therefore nameable by no fixed path. **Consequence for an `llm` cell with a `system_writable` allowlist, unchanged by the move**: the allowlist must carry `memory` as a prefix -- the replace ROOT is checked too, and `memory.recall` alone does not suffice. Since wave 11 it also reports what the curator did: `hop.tokens_window`, `hop.tokens_projected`, `hop.tokens_estimated`, `hop.curate_mark`, `hop.curate_stage`, `hop.curate_elided`, `hop.curate_saved`. |
| `answer` | the reply sink | the brain's final turn, after it is in the window -- **or** a turn that reached `max_iter`, marked `hop.round_capped=1` -- **or**, since `collector@2.1.1`, a turn that could not be assembled because the store refused, marked `hop.degraded=1` with `hop.store_error` and `hop.store_operation` beside it (see "When the store says no") |
| `recall` | the memory hive's recall port | the per-turn leg (only when `memory_tier` is set) **and** every `memory_recall` call; promote `recall_query`, `memory_tier`, `memory_call_id`, `recall_window_from`, `recall_window_to`, `session_id`, `turn_id`, `iter` |
| `write` | wherever a closed session belongs | one batch per close: `messages[]` the whole conversation, the raw round rows in the top-level slot `rounds`. `messages[]` is what a PARTICIPANT said and nothing else (GH #282) -- interim answers, `advice` rows and any other role stay in the window; `origin` comes from an explicit `user`/`assistant` mapping, never from a fallback. See "Per-turn episodes" below. |
| `turn_write` | a memory hive's episode lane | **one message per turn, never a batch** (GH #298): after every stored turn and every stored answer, every turn of the session that has not been written yet leaves as its own message -- one `user`/`assistant` turn in `messages[]`, `hop.turn_id` = `<session_id>#<index>`, `hop.turn_index` and `hop.happened_at` beside it. Filtered and attributed by the same rule as `write`, but **not the same document**: `write` is a closed day with its `rounds`, this is a turn. On by default. See "Per-turn episodes" below. |
| `prune` | a log sink or the operator surface | one report per pruned session (`hop.session_id`, `hop.pruned_turns`, `hop.pruned_rounds`, `hop.prune_boundary`) -- or a single zero report when nothing was eligible -- or, since `collector@2.1.1`, a zero report marked `hop.degraded=1` because the store refused one of the prune chain's own reads or deletes |
| `condense` | -- | **reserved, never emitted today.** The value is declared in the enum so the fold lane can be wired later without widening a published contract; nothing in this cell writes it. |
| `cstore` | `window`, inside the hive | **interior, and it never crosses the hive path.** Every store round-trip of the state machine rides on it (`hop.phase` carries the state, `hop.turn_id` the turn). It is in the enum because the assembler emits it, and it is in no parent's wiring because the seal gives it nowhere to go. |

The enum itself is `contract.emits.hop.route` in `assemble/config.json` -- that declaration
is the authority, this table is its prose. Six of its eight values are the hive's declared
exits (`params.contract.emits` in `config.json`, the list a parent may wire): `cstore` stays
inside, and `condense` is reserved.

**The hive path is the address, and the lane is the contract.** Every lane in and every
route out crosses that one endpoint, and the working colonies under
[`../../examples/`](../../examples/) address it as `<parent>/collector` -- it is a stable
**address**, not implementation detail that happens to be reachable. Both cells behind the
door are **unroutable** from outside -- no edge may name either of them. Their *names* are
not free, though: `override_params` addresses a knob by the cell path **inside** the
template (`assemble`, or `collector/assemble` from a composite -- see "The knobs are per
instance" below), and `examples/never-forgets/grow.json` does exactly that. Renaming or
splitting `assemble` therefore breaks every parent that tunes a knob. What is genuinely
free behind the door is the *implementation*, not the layout. What may not move at all is
the set of LANE NAMES above, and dropping one is a breaking change to every parent that
wired it: a CHANGELOG Breaking entry and a new major version, never a patch.

Until `collector@2.0.5` this section said the opposite three times over -- entry lanes went
"into `./assemble`", exits left "from `./assemble`", and that cell was called "the port
address" -- while the sentence beside it already wrote the correct one. Every one of those
addresses is refused by the seal, and it is refused in precisely the mutation the next
paragraph tells you to write ([#311](https://github.com/mmeyerlein/meclaw/issues/311)).

Wire the ports in the **same mutation** that instantiates the hive: an island without a
crossing edge derives inactive and never spawns.

### What a tool result may carry (GH #252)

A tool result is its **`messages[]`** -- every turn of it, in the order the tool wrote
them, each turn correlated to the call it answers by `id`. That is the whole interface,
and there is no second one.

```json
{"header": {"route": "res"},
 "messages": [{"origin": "tool", "type": "tool_result", "id": "c1", "text": "..."},
              {"origin": "tool", "type": "tool_result", "id": "c2", "text": "..."}]}
```

Two consequences, both deliberate:

- **A result may answer more than one call.** A batch tool that gets the whole bundle in
  one message answers all of it in one message, and the fan-in closes every call in it.
  Until `collector@2.0.2` the lane kept `messages[0]`, so the other calls stayed open and
  the round waited for results that had already arrived until `round_idle_ms` expired.
- **A `system` slot on this lane is dropped, and so is a top-level body slot.** What
  leaves the seam in `system.*` is UPSERTed into the brain cell's own `cell.db` and stands
  in the prompt until something overwrites that exact slot path -- it is durable state of
  the agent, not evidence of one round. A single tool result gets no second chance to
  correct itself, and a brief about one subject would still be in the prompt three
  subjects later. `system.*` is also out of the curator's reach on purpose -- it is where
  hard *constraints* belong -- so a tool writing there would grow the prompt with nothing
  left able to cut it, against a slot budget the `llm` cell caps at 256 (GH #118).

  **Retracted in `collector@2.1.0`
  ([#278](https://github.com/mmeyerlein/meclaw/issues/278)).** Up to `collector@2.0.6`
  this paragraph made one exception and named it here: the recall bundle, it argued,
  *survives* durable treatment because it is re-sent under a fixed path on **every** turn
  and can therefore never go stale. That argument is withdrawn, and the bundle has left
  `system.*` altogether. Three consequences were measured, and re-sending addresses none
  of them: a model shown a lookup in the place its instructions live **discounts** it, the
  way it discounts any configuration; a slot nothing expires goes **stale in silence** the
  first turn that does not re-send it -- a restart, a tier switched off, a recall port that
  stopped answering -- and nothing in the prompt says so; and `system.*` is out of the
  curator's reach, so the bytes of the one payload that grows with an agent's memory were
  counted as an anonymous lump of `sys_chars` that no stage could attribute to anything.
  The bundle now travels as the `memory_recall` `tool_result` of its own round -- the
  channel this hive already served on `in_memory_call` (GH #78) -- where it is evidence
  under a name, expires with the round it was fetched for, and is counted item by item
  beside every other result. The `in_bundle` lane still keeps `system`, because that is how
  the bundle reaches this cell at all; what changed is where it goes from here.

**So a tool with structure to hand back puts it in the text of its result**, serialised
however its caller can read it. That is not a workaround for a missing channel; it is the
channel. A provider sees a tool result as one string on one `tool_call_id`, and anything
richer would have to be flattened for the wire anyway -- the only question is who does it,
and the tool that produced the structure knows its own shape best. The `affinity` template does
exactly this: the receipt line, then the disclosed pack as JSON behind it.

**If you want a durable constraint rather than an answer**, that is a different lane and a
different cell: address the `llm` cell's `system` tree directly (the push lane of
`affinity` is the worked example), where the write is meant to outlive the round and the
`system_writable` allowlist decides who may make it.

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
| `memory_chars` | `8000` | character cap on the memory bundle **where the bundle travels**: the synthetic `memory_recall` tool result. ONE cap over the whole result text, so under `memory_form: both` it bounds the readable block and the machine-readable form *together* rather than each of them separately. `hop.memory_capped` is measured on that result. |
| `max_iter` | `8` | how often a turn may re-enter the brain with a tool round. At the cap the seam leaves on `answer` instead. |
| `round_idle_ms` | `120000` | idle window of one tool round (two minutes). A round whose last progress is older **and** whose fan-in is incomplete is closed at the next occasion with synthetic error results and fires with `hop.round_stale=1`. |
| `memory_tier` | `""` | empty = no memory leg at all, and the assembly waits for the window leg alone. `"0"` / `"1"` / `"2"` request that recall tier once per turn, and **the ambient leg arrives as a synthetic `memory_recall` result** at the end of the round -- never as durable system state (`collector@2.1.0`, GH #278). |
| `memory_form` | `"readable"` | which form of the bundle reaches the brain **in that tool result**: `readable` (the rendered block a model reads), `json` (the machine-readable bundle), `both` (the two joined by a newline, under one call id and one cap). Applies to the ambient leg and to a model's own `memory_recall` call alike. Whatever the form, `system.memory` carries only the revocation -- the empty leaf on the fixed path `recall` plus the `$replace` marker on the node above it (see the `brain` lane, `collector@2.0.4`) -- and both halves are sent unconditionally, no longer chosen by this knob: an instance retuned from `readable` to `json` would otherwise carry its last leaf, or its last keys, for the rest of its life. |
| `memory_call_tier` | `"1"` | recall tier of the **memory tool** (GH #78). Configuration, never a model argument. Empty switches the tool off: a call is then answered with a typed error result instead of being asked into a void. |
| `async_tools` | -- | **not a collector knob.** The async class is declared once, at the dispatcher (`DISPATCHER_ASYNC_TOOLS`), and travels as `hop.async_calls`. |
| `prune_after_ms` | `604800000` | age gate on the prune lane (seven days). A session is pruned only when its close batch left **and** that delivery is older than this. |
| `turn_write` | `"1"` | **on by default since GH #298** -- it is the only path from a conversation into an episodes table, and a shipped "off" would be a shipped agent that remembers nothing. Every stored turn hands out one message per unwritten turn on route `turn_write`. `""` or `"0"` switch it off, and off means nothing said in this session reaches a memory *at all*, not that it reaches one later. Switch it off only where that route is unwired: an unrouted emission per turn is a dead letter per turn. |
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
- **`system.*`** -- instructions and handover. It counts towards the budget and is never a
  candidate, which is exactly why hard constraints belong there and not in the chat: what
  is only in the conversation is compaction-mortal everywhere else. Since
  `collector@2.1.0` the **memory bundle is no longer on this list** (GH #278): it is an
  ordinary row of the round now, in `messages[]` beside every other result. Saying so
  explicitly, because "not on the never-touched list" reads like "elidable" and it is not:
  curation only ever touches results carrying an iteration tag, and the synthetic pair
  belongs to no iteration -- and even if it did, `recoverability` classes everything it
  does not name as **`unique`**, so `memory_recall` is `unique` unless somebody declares it
  otherwise, and `unique` is never elided
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
{"from": "./dispatcher", "to": "./collector",
 "condition": "hop.route == 'tool' && hop.tool_name == 'thread_recall'",
 "modifier": {"set_hop": {"route": "'in_thread_call'"}}}
```

`in_thread_call` is a declared lane of the hive contract since `collector@2.0.1`
([#245](https://github.com/mmeyerlein/meclaw/issues/245)) -- before that the edge above was
refused with `hive_contract` at mutation time, so every stub the curator left pointed at a tool
no caller could wire. A composite that carries this collector as a sub-unit has to declare the
lane **at its own hive path** as well and forward it through its door edge; `talky` does (since `talky@3.0.1`),
`cogny` does not (its seal admits one lane in total, [#240](https://github.com/mmeyerlein/meclaw/issues/240)).

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

**What `override_params` does, and the one key that looks right and is not.**
`add_nodes[].override_params` reaches these knobs at birth. On a subtree template it is
**addressed** ([#140](https://github.com/mmeyerlein/meclaw/issues/140), which superseded the
R10 blanket reject of 2026-06-11 -- R10's finding was a flat override that committed as a
silent no-op, and addressing removes the cause instead of the feature): each key is a cell's
path inside the template, `""` being the subtree root.

```json
{"add_nodes": [{"name": "collector", "template": "collector",
                "override_params": {"assemble": {"turn_write": "1", "max_iter": 12}}}]}
```

The knobs are params of `assemble`, so the key is `assemble` -- and from a composite that
carries a collector as a sub-unit (`talky`, `cogny`) it is `collector/assemble`, never
`collector`. **A key that stops at a HIVE is the trap**: `""` here, the sub-unit's root
there, both valid cell paths and both accepted. A hive reads only `graph`, `ports`,
`required_drains` and `contract`, so params set on one are read by nothing and the instance
comes up unconfigured with no diagnostic anywhere
([#212](https://github.com/mmeyerlein/meclaw/issues/212)). A key that names no cell at all is
the loud case R10 protected: refused pre-destructively, with the template's actual cells in
the message.

Setting them **in the instantiated tree** stays available and is what a parent that owns the
tree does: write the values into `…/assemble/config.json` after the mutation lands. `params`
are read when the cell spawns, so the value is live from the next boot of that cell.

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

### When the store says no (GH #343, since `collector@2.1.1`)

Since `collector@3.0.2` most of the assembler's reads travel as **bundles**, and a bundle reply
never stamps `error_code` on the hop: that header means "the whole reply is a refusal and
carries no payload", and a bundle whose second leg failed still hands back the first leg's rows.
It stamps `hop.bundle_errors` instead, with the per-leg codes in `results[]`. For this cell one
refused leg is one too many -- an assembly that fires on a half-read round is exactly the
"answered with no conversation at all" failure this section exists to prevent -- so the guard
reads `bundle_errors`, takes the first refused leg's `error_code` and `operation`, and degrades
the turn with `hop.degraded`, `hop.store_error` and `hop.store_operation` exactly as a refused
single op does.

The assembler is a state machine over `(context.col_phase, hop.operation)`: it sends the
store one op, and the reply's `operation` tells it which branch it is in. That reading has
one hole, and the hole is the whole of this section.

`operation` says **which op answered**. It does not say **whether it worked**. The store
stamps `hop.operation` on its failing replies too -- it always did for SQL-level failures
(`unknown_table`, `unknown_column`, `constraint_violation`, `sql_error` all travel through
the ordinary reply builder), and since [#331](https://github.com/mmeyerlein/meclaw/issues/331)
it does for `invalid_input`, `query_timeout` and `write_denied` as well, because a return
edge conditioned on `hop.operation` must not lose exactly the replies that report a
failure. So a refusal arrives looking **exactly** like an answer: same phase in the
context, same op in the hop, and an error sentence where the rows should be.

Read as an answer, that sentence became zero rows. Measured: a `query_timeout` on the
window read wrote an **empty** window leg into the round table, the fan-in completed, the
seam fired, and the model answered the turn with no conversation at all -- honestly, and
wrongly, and silently.

Every branch of the machine now reads **both** fields, and a refusal is terminal:

- no further store op leaves -- the phase does not advance;
- the failure is **said**, on a lane the parent already drains. The prune chain reports on
  `prune` (the same lane its zero-report already used); every other phase reports on
  `answer`, which is where that turn was going anyway.
- the report carries `hop.degraded=1`, `hop.store_error` (the store's own `error_code`)
  and `hop.store_operation` (the op it refused), and the text names all three.

`hop.store_error` is a free string, not an enum: the store's code list is open, and a
declaration that had to grow with it would turn the next new code into a failed emit.

This is the shape [#308](https://github.com/mmeyerlein/meclaw/issues/308) put into
`builder-librarian/retrieve` after the same failure was found there. It is not a
degradation *strategy* -- the collector does not guess a window it could not read. It
refuses to pretend it read one.

### Round robustness (GH #103)

Unchanged by `collector@3.0.2`, and worth one sentence about where it now happens: the idle exit
(`lost_results`, `hop.round_stale=1`) and the defer rule are decided out of the **same bundle
reply** the round is read in, rather than one hop behind it. What they decide, and on what
evidence, did not move.

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
{"from": "./dispatcher", "to": "./collector",
 "condition": "hop.route == 'tool' && hop.tool_name == 'memory_recall'",
 "modifier": {"set_hop": {"route": "'in_memory_call'"}}}

// 2. the recall port the per-turn leg already used, carrying five keys now
{"from": "./collector", "to": "<memory hive>/recall",
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

The block above is prose; the **canonical copy of that schema is shipped**, in
`templates/talky/brain/seed/system.jsonl` ([#55](https://github.com/mmeyerlein/meclaw/issues/55)) --
a talky instantiated from the library carries it already, and anyone writing the schema into
another brain's `system.tools` should copy the seeded bytes rather than retype this paragraph.

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
The lane closes that hole without a single model call, and since GH #298 it is the **only**
path from a conversation into an episodes table -- which is why it ships on.

Two moments ask: the echo of the stored **turn** and the echo of the stored **answer**.
Each asks the session's turns back and hands out **one message per turn that has not been
written yet** -- never a batch. That shape is not a preference: a memory hive's writer takes
the *first* `user`/`assistant` text turn of a body and ignores the rest, so one turn per
message is what the port accepts.

```
in_turn   -> insert turns row -> (round check)  +  select turns of the session -> tw-scan
in_answer -> insert turns row                   +  select turns of the session -> tw-scan

tw-scan -> per unwritten turn:  ROUTE turn_write (one turn)
                              + update turns set episode_written=1
                                where {id, episode_written: {or_null: {eq: 0}}}
```

Each message carries the full key set a port edge reads, empty included -- a missing hop key
makes a CEL modifier fail and a failed modifier *skips* the edge:

| key | value |
|---|---|
| `hop.turn_id` | `<session_id>#<index>` -- deterministic, and the same formula a bulk import mints |
| `hop.turn_index` | the index alone, as a string |
| `hop.happened_at` | the row's `recorded_at`, so the writer's event time is the turn's and not the writer's clock |
| `hop.session_id`, `hop.iter`, `hop.phase` | as on every emission of this cell |

The **index counts drainable turns, not rows**: an interim answer or an `advice` row (see
below) is neither handed out nor counted, so the turn after it keeps the index it would have
had without it. That is what makes the id an identity rather than a counter -- a session
whose lane was switched on late mints exactly the ids the turn-by-turn run would have.

**Idempotence is a column of this cell's own table, not a ledger in the consumer.**
`turns.episode_written` (`int`) is set by a guarded
`update ... where {id, episode_written: {or_null: {eq: 0}}}` that rides in the **same
emission** as the episode it covers: what left and what says it left are one decision, never
two. The scan deliberately does not take "the newest row" -- with two turns in flight the
newest is the wrong one and the other is lost, which is the bug a high-water mark used to
prevent.

The guard reads `or_null` and not a plain `0` on purpose, and the reason is broader than a
migration: **no insert this cell makes names the column.** It arrives by
`ALTER TABLE ADD COLUMN`, SQLite fills no default, and neither the turn insert nor the
answer insert writes it -- so *every* row reads `NULL` until this guard sets it, and `NULL`
is not `0`. A guard comparing to `0` alone would match no row at all, and the scan would
hand every turn out again on every occasion, for ever. `NULL` means *not written*, on the
read side (`int(... or 0)`) and in the guard alike.

**On an existing tree, the first scan after the upgrade re-delivers.** Every row the old
lane already handed out reads "not written" -- nothing marked it -- and is handed out
**once** more. Where a
`memory-drain` sat behind this route, its own ledger answers that with a skip; where nothing
does, the memory hive sees the same `turn_id` twice, and the deterministic id is what makes
the repeat recognisable at all.

**Not everything in the window is a turn of the conversation.** What leaves on `turn_write`
-- and on `write`, from the same helper -- is what somebody said: a row whose `role` is
`user` or `assistant` **and** whose `interim` column is `0` (GH #282). Three classes stay
behind, one clause each:

- an **interim** answer (`interim = 1`) -- the sentence of the advisor split that buys time
  ("one moment, I'm thinking about that.", R-CG-3): an answer on the wire, so the model must
  know it said it, but nothing anybody told anybody, and before the column existed it became
  an episode once per deferred turn;
- an **advice** row (`role = "advice"`) -- what the advisor came back with on `in_advice`:
  the agent is about to speak about it in its own voice, so it belongs in the window, but
  nobody in this conversation said it and it is itself a rendering of what the memory
  already holds, so draining it feeds the memory its own answers;
- **any other role** -- anything this cell writes into `turns` that is not one of the two
  above: context for the model, never an episode.

The same decision names the **attribution**: `origin` on both routes is read from an
explicit `{"user": "user", "assistant": "assistant"}` mapping, applied after the filter has
already refused everything else. There is no fallback, so there is no role either route can
attribute to a speaker who did not speak -- the way `advice` used to reach the memory as
`sender=user`. Nothing on this lane writes a *speaker* either: the episode inherits the
`context` of the chain it belongs to, which is the turn's own chain, so per-turn identity
travels without anybody asserting it in a body.

Nothing invents a role **before** the filter either: the close lane parks its leg with the
role the row carried, empty included. A default there put a roleless row past the filter as
`user` while the per-turn lane, reading the raw rows, dropped it -- two lanes, two answers,
over a row nobody writes today. Both now ask the one filter.

The prompt window is deliberately not affected: `leg-window` is read by the seam and by no
write path, so an advice turn still reads there as something said, still carries its
`consult_id`, and a turn without a role is still shown as the other side of the
conversation. Nothing downstream turns that reading into an episode.

The column is added additively, so a running collector migrates itself (`ALTER TABLE ADD
COLUMN`). Rows written before the change read `0`, which is the correct answer for every
turn the old lane wrote **except** the interim ones -- those are indistinguishable after the
fact and keep whatever they already reached.

**Wire it at a memory hive's episode lane**, and wire the `write` route somewhere else or
nowhere. Until GH #298 the advice here was the opposite -- both routes into the *same*
consumer, because both carried the same day and the consumer's own ledger made the repeat
free. That is retracted: `turn_write` carries a turn and `write` carries a closed day, the
per-turn lane is the whole write path rather than a fast half of one, and a `write` edge
into the same memory would be a second writer over turns already written. The close route
keeps whatever else it feeds -- an archive, the summarizer -- at its own cadence.

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
in_turn   -> ONE message, four calls      phase turn-open   <- GH #419
             c-open-turn:  insert turns(user)
             c-open-round: select round (open rounds)       <- GH #103
             c-open-win:   select turns (limit N)           <- read unconditionally
             c-open-day:   select turns (the day)           <- only with turn_write
          -> [recall request]            (only with a memory tier)
turn-open -> no open round: insert round(leg-window)
             + select round               phase collect     <- the byte cap runs here
          -> open round: update turns
             set deferred=1               phase defer-w     <- the turn parks
             (+ per stale round: the round-check below)
in_bundle -> insert round(leg-memory)
             + select round               phase collect
collect   -> complete: ROUTE brain                          <- the seam
             + update round set fired=1   phase collect-done<- the round has answered
          (+ update turns set deferred=0  phase defer-clear, when a deferred
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
in_tool   -> insert round(tool)          phase round-w      <- the WHOLE messages[]:
                                                               one result may answer
                                                               several calls (#252)
in_memory_call -> ROUTE recall           (memory_call_id = the tool_call_id,  <- GH #78
                                          recall_window_from/_to = the args)
in_bundle (with a memory_call_id)
          -> insert round(tool)          phase round-w      <- back in the regular fan-in
round-check-> complete: ROUTE brain (iter + 1)              <- the same seam
             + update round set fired=1  phase round-done   <- per ITERATION
          -> ROUTE answer (round_capped) <- at max_iter, instead of the brain
          -> incomplete + idle: insert
             round(tool, 'tool result
             lost') per missing call     phase round-check  <- GH #103, back into
                                                               the regular fan-in
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
close-turns-> insert round(leg-close)      phase close-fire    arrival time: the prune
            +  select round (session)                          boundary of this close
close-fire -> ROUTE write                                   <- one batch, append-all
           +  insert batched(session, B)  phase close-ledger<- the delivery evidence,
                                                               in the SAME multi-send
```

and, when a timer (or an operator) asks for housekeeping:

```
in_prune    -> select batched (aged, unused)  phase prune-ledger
prune-ledger-> per session: delete turns       phase prune-cut<- boundary in the hop id;
            +  delete round (same boundary)                    no ledger row -> zero
prune-cut   -> update batched set pruned_at   phase prune-mark  report, NO delete
            +  ROUTE prune (the report)                     <- pruned_turns / _rounds
                                                               out of results[]
```

An incomplete fan-in emits **nothing** (empty multi-send, terminal by design) -- the same
discipline as the store-backed tool loop this grew out of.

**How the election works since `collector@3.0.2`** (GH #419). Every leg of a round parks its row
and reads the round table back in the **same message**. The `store` is a stateful cell -- one
task, one connection, one message at a time -- and a bundle is one message whose ops run in call
order, so the trailing `select` sees the `insert` in front of it and **of N legs parking
concurrently exactly one reads a complete set**. That one assembles. There is no guard row to
win and no message to win it with: `gate` (a select), `fire-guard` (a guarded update) and `fire`
(a second select) are gone, and so are `round-w`/`round-guard`/`round-fire`, `turn-w`, `win` and
`close-w`.

**What did NOT go with them, and this is the half that matters**: the `fired` column. The
guarded update did two jobs. It elected among the legs racing to complete the round -- that is
the read-back's job now -- and it made the election **permanent**: a leg that lands AFTER the
turn has left (the advisor's late event is the ordinary case) reads a complete set too, and
without the mark it would assemble the turn a second time. So the mark still travels, now
**beside** the seam in the same multi-send rather than one hop in front of it, and the election
reads it. A round marked `fired` never fires again -- which is also what `turn-open` and the
idle sweep read to tell an open round from an answered one (GH #103).

## The async class and the return lane (GH #28, R-CG-3, GH #372)

A tool that thinks does not fit inside a round. An advisor core answers in minutes; a
fan-in that waited for it would be betting `round_idle_ms` against thinking
time, and losing that bet writes "tool result lost" into the transcript. So the round
does not wait at all:

1. **The dispatcher classifies, on two lists.**
   `DISPATCHER_HANDOFF_TOOLS=consult_cogny` makes the dispatcher name the affected
   `tool_call_id`s in `hop.async_calls` **and** in `hop.handoff_calls` on the `calls` lane;
   a tool on `DISPATCHER_ASYNC_TOOLS` alone (`remember`) is named on the first only. One
   declaration per tool, in the one cell that sees the whole bundle. The second list is
   what says the answer comes from a **later turn** rather than from this one -- step 2
   reads it, and the classification itself is never this cell's.
2. **The collector opens no expectation.** Each named id is answered on the spot with a
   plain `tool_result` under its own `tool_call_id` -- the assistant turn stays
   well-formed for every provider -- and when *nothing else* was asked **and the turn is
   going to be answered anyway**, the assistant row is written `fired=1`. There is no open
   round: no guard to win, nothing for `in_round_sweep` to find, no idle exit. The turn
   ends with the interim answer the dispatcher already sent to the channel.

   **"Answered anyway" is a condition, not an assumption ([#372](https://github.com/mmeyerlein/meclaw/issues/372)).**
   Exactly two things satisfy it, and the lane reads both off the message it was handed:

   | | what says so | who answers the turn |
   |---|---|---|
   | the model spoke beside the bundle | a non-empty text turn in `messages[]` -- the same reading the dispatcher used when it sent the interim answer | the interim answer, already on the channel |
   | a **handoff** call took the turn with it | `hop.handoff_calls` names it (`DISPATCHER_HANDOFF_TOOLS`) | a later turn: an advisor's event, an escalation re-entering the seam |

   **Neither, and the round stays open.** A bare fire-and-forget call -- `remember` with no
   sentence beside it -- used to be filed as fired, and the channel then got *nothing*: no
   interim, no final, no error, and `round_idle_ms` does not fire on a quiet channel.
   Measured in three of five full harness runs, and the per-turn contract (GH #298) makes
   call-only iterations common. So the acknowledgement completes the fan-in, the regular
   guard fires, and the seam re-enters the brain for the iteration the model has not spent.
   Nothing new stops it -- `params.max_iter` bounds this round like any other, and a spent
   budget leaves the same seam on route `answer` with `hop.round_capped=1`. **A round
   always ends in an answer**: a real one, `round_capped`, or `degraded` (GH #343).

   The classification itself is *not* this cell's: which of the two classes a tool belongs
   to is tool semantics, and it is declared once, at the dispatcher, which is the only cell
   that sees the whole bundle.
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
{ "from": "./dispatcher", "to": "/front/cogny",
  "condition": "has(hop.route) && hop.route == 'tool' && has(hop.tool_name) && hop.tool_name == 'consult_cogny'",
  "modifier": {"set_hop": {"route": "'in_turn'"},
               "set_context": {"consult_id": "hop.consult_id"},
               "restore_ttl": true} },
{ "from": "/front/cogny", "to": "./collector",
  "condition": "has(hop.route) && hop.route == 'answer'",
  "modifier": {"set_hop": {"route": "'in_advice'"}, "restore_ttl": true} }
```

## What it is not

- **Not a persona.** Identity is not context assembly. A persona cell upstream still owns
  `system.identity`, and the brain edge stays one seam so a later split of the agent can
  happen behind it without re-cutting the collector.
- **Not a dispatcher.** Routing a `tool_call` to the right tool is a fan-OUT; the collector
  only fans results back in.
- **Not a second body format for tool results.** A result is the `tool_result` turns of its
  `messages[]`; `system.*` and top-level body slots on that lane are not part of a result
  and do not travel (see "What a tool result may carry").
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
  answer as a tool result, the switched-off tier). Since 2.0.2 also what a tool result may
  carry: a result answering two calls in one message closes both, and a `system` slot on
  that lane stays at the door. Since 2.0.3 also the REVOCATION of the two `system` slots
  the collector owns -- each pinned over two rounds, because a pin on the first round is
  green with and without the repair. Since 2.0.4 the same question for the `json` form,
  which has no path to send empty and is revoked by the marker on `system.memory`
  instead: the key of a bundle the next turn does not name is gone, one marker covers
  both legs under `both`, and -- the counter-pin that matters more -- the marker sits on
  that node and on no other, so `system.consult` and every slot the collector never wrote
  stay untouched.
- `crates/meclaw-cells/tests/gh278_the_ambient_recall_is_a_tool_result.rs` -- the channel
  itself, since 2.1.0: the ambient bundle leaves the seam as the last `tool_call` /
  `tool_result` pair of `messages[]`, the call id is derived and therefore stable across a
  re-assembly of the same turn, `system.memory` carries the revocation and no data under
  every `memory_form`, `memory_chars` caps the result text, the pair is appended before the
  curator runs and is never a curation candidate, and a model's OWN `memory_recall` still
  answers under its original `tool_call_id` without a synthetic pair.
- `crates/meclaw-cells/tests/w9a_per_turn_episodes.rs` -- the per-turn lane's CADENCE at
  script level: the two occasions, the knob (on by default since GH #298, off in both
  spellings), one message per turn in the order of the day, and the same day delivered
  twice writing no second episode. Its header records what ruling Q11 retired: the close
  drain's completeness claim and the byte-identical-replay claim, because the two routes
  hand out different documents now.
- `crates/meclaw-cells/tests/gh298_the_turn_writes_its_own_episode.rs` -- the SHAPE:
  one message per unwritten turn, the deterministic `<session>#<index>` and its
  `turn_index` and `happened_at`, every hop key present rather than absent, the guarded
  `episode_written` mark in the same multi-send, a written day handing out nothing, and
  the index that counts turns rather than rows.
- `crates/meclaw-colony/tests/gh245_a_stub_names_a_lane_the_hive_admits.rs` -- the lane
  a curator stub names against the SHIPPED hive files: an edge stamping `in_thread_call`
  into the collector commits, an edge stamping `in_batch` is refused now that nothing
  behind the door reads it, and a real call on the lane crosses `talky`'s door and the
  collector's door and lands on the assembler.
- `crates/meclaw-cells/tests/collector_colony.rs` -- a running colony with no memory hive
  in it at all, so a turn that references an earlier turn can only have been answered from
  the window; plus a 100 KB tool result that arrives capped, a runaway round that the seam
  ends, a session that leaves as one batch, a batched session that is pruned while
  the living session keeps every byte, a lost tool result whose round a sweep closes, and
  a mid-round turn that defers and rides with the next assembly, and a batching
  dispatcher whose tool answers the whole bundle in one message. Two more trees run the
  memory tool against the **shipped `dispatcher`** -- one with the recall port wired
  (both results fan in, the model's time range reaches the request) and one without it
  (the call is unroutable and the round ends in the idle exit).
