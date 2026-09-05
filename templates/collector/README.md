# `collector@4.0.0`

Context assembly as a hive of existing cell types -- no new cell type, no Rust. Two cells:
`assemble` (a `code` cell, the state machine) and `window` (a `store` cell, the state). The
one question this hive asks of its own accord -- the tool menu -- is asked on `mutation_committed`,
the mutation receipt the level above carries in; since `4.0.0` there is no timer in here.

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
- **A memory the model can ask itself -- and it is not this cell's to answer** (`4.0.0`,
  [#552](https://github.com/mmeyerlein/meclaw/issues/552)). The ambient leg is fired before
  the model has seen the turn, so nothing in an agent could ever *decide* to ask about a
  **time range** (GH #78). A `memory_recall` tool call closes that half. It was served
  HERE from GH #78 to `collector@3.5.0`, on the recall port this cell already owns, under a
  schema this cell had typed by hand as a projection of the memory hive's own `in_query`
  contract -- and a cell that answers a call whose rules it cannot enforce will drift from
  them. The hive declares and answers the name now; the lane `in_memory_call` and the
  setting `memory_call_tier` are gone, and what is left here is the ambient leg, which asks
  a different question at a different time.
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
  and not by an edge that has to know about iterations. Since `3.5.0` it is stopped
  with a SENTENCE: what leaves is a named partial answer, not the raw end of the round.
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
| `assemble` | `code` | the whole state machine: thirteen entry lanes plus the internal `in_menu_tick`, the fan-in gate, the eviction policy, the seam, the round-robustness exits, the prune chain |
| `window` | `store` | `turns` (the rolling conversation) and `round` (the per-turn slate: the assembled legs plus the tool round) -- both carry `session_id`, which is what makes them readable as a whole session at close time, and write times, which is what makes them prunable. Plus `batched`, the delivery ledger of the close lane, and -- since `3.4.0` -- `menu`, one row per answerer, which is the memory the tool menu is merged out of (GH #529). |

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
| `in_bundle` | the memory hive's recall port | becomes the memory leg of this turn. ONE meaning since `4.0.0` -- it carried a second, the tool result of a `memory_recall` call, told apart by a `memory_call_id` the request carried out ([#552](https://github.com/mmeyerlein/meclaw/issues/552)) |
| `in_calls` | the tool dispatcher | the assistant `tool_call` turn of the round; `hop.async_calls` names the ids this fan-in must **not** wait for |
| `in_tool` | a tool cell | one tool result: **every** `tool_result` turn of its `messages[]`, each filed under the call id it answers. See "What a tool result may carry" below |
| `in_thread_call` | the tool dispatcher, on `hop.tool_name == 'thread_recall'` | the thread tool: brings an elided payload of THIS turn back, uncapped, out of the collector's own slate (wave 11) |
| `in_answer` | the brain, on `finish_reason == 'stop'` | writes the answer into the window and lets it out |
| `in_close` | the session keeper, on `hop.route == 'close'` | reads the whole session back and batches it out |
| `in_prune` | a timer or an operator, on `hop.route == 'prune'` | prunes delivered-and-aged sessions; the template **never fires this itself** |
| `in_round_sweep` | a timer or an operator, on `hop.route == 'sweep'` | re-checks every open tool round and closes the stale ones; equally **never fired by the template itself** |
| `in_pack` | whatever curates this agent's identity -- `affinity`'s push lane is the worked example | a durable `system.*` slot for the brain: `identity`, `persona`, `handover` or `instructions`, and nothing else. The one lane that carries state meant to OUTLIVE the round. See "The door in the wall" below |
| `in_menu` | the tools hive this agent's tools live in, answering on `tool_schemas` | the declarations of the tools this agent DECLARED it uses: `schemas[]` and the names the hive had nothing under. Since `3.4.0` the answer is filed under `context.tool_answerer` as ONE row of the `menu` table and the menu is re-derived as the union over every answerer's row (GH #529). See "The menu is asked for" below |
| `mutation_committed` | the level above, carrying the mutation door's receipt (GH #553) | the occasion to ask for the menu again. The hive's own door turns it into the internal `in_menu_tick`, which is why nothing outside ever names that lane. It replaced `./menu-clock`, a five-minute poll |

Exits leave **from the hive path** on `hop.route`:

| route | to | notes |
|---|---|---|
| `brain` | the agent LLM | THE seam. Promote `hop.turn_id`, `hop.session_id` and `hop.iter` to context on this edge. `system.consult.open` carries the correlation ids of the advice turns still in the window -- **always**, empty included (`collector@2.0.3`): the `llm` cell upserts `system.*` per slot path, so a path that is not sent is a path that is not touched, and a slot that is only ever set keeps naming a consultation that closed long ago. `system.memory` follows the same rule and, since `collector@2.1.0`, carries nothing but that rule: the bundle itself is no longer anywhere in that subtree (GH #278) -- it travels as the `memory_recall` tool result at the end of `messages[]`. What the collector still sends there on every turn is the revocation, unconditionally and no longer tied to `memory_form`: an empty `text` on the FIXED path `system.memory.recall`, which clears a bundle an older collector may have left standing and contributes nothing to the system prompt, plus the `"$replace": true` marker on the whole `system.memory` node (`collector@2.0.4`, GH #264), which is what lets it revoke the `json` form's keys -- named by the memory hive per bundle, and therefore nameable by no fixed path. **Consequence for an `llm` cell with a `system_writable` allowlist, unchanged by the move**: the allowlist must carry `memory` as a prefix -- the replace ROOT is checked too, and `memory.recall` alone does not suffice. Since wave 11 it also reports what the curator did: `hop.tokens_window`, `hop.tokens_projected`, `hop.tokens_estimated`, `hop.curate_mark`, `hop.curate_stage`, `hop.curate_elided`, `hop.curate_saved`. |
| `answer` | the reply sink | the brain's final turn, after it is in the window -- **or** a turn that reached `max_iter`, marked `hop.round_capped=1` **and**, since `collector@3.5.0`, `hop.partial=1`, whose last turn is a named PARTIAL ANSWER rather than the raw end of the tool round (see "A capped round is a partial answer") -- **or**, since `collector@2.1.1`, a turn that could not be assembled because the store refused, marked `hop.degraded=1` with `hop.store_error` and `hop.store_operation` beside it (see "When the store says no") |
| `recall` | the memory hive's recall port | the per-turn leg, and only that (`memory_tier` set); promote `recall_query`, `memory_tier`, `recall_window_from`, `recall_window_to`, `session_id`, `turn_id`, `iter`. A `memory_recall` CALL does not travel here since `4.0.0` -- it leaves the composite on the ordinary `tool` lane and the memory answers it ([#552](https://github.com/mmeyerlein/meclaw/issues/552)) |
| `write` | wherever a closed session belongs | one batch per close: `messages[]` the whole conversation, the raw round rows in the top-level slot `rounds`. `messages[]` is what a PARTICIPANT said and nothing else (GH #282) -- interim answers, `advice` rows and any other role stay in the window; `origin` comes from an explicit `user`/`assistant` mapping, never from a fallback. See "Per-turn episodes" below. |
| `turn_write` | a memory hive's episode lane | **one message per turn, never a batch** (GH #298): after every stored turn and every stored answer, every turn of the session that has not been written yet leaves as its own message -- one `user`/`assistant` turn in `messages[]`, `hop.turn_id` = `<session_id>#<index>`, `hop.turn_index` and `hop.happened_at` beside it. Filtered and attributed by the same rule as `write`, but **not the same document**: `write` is a closed day with its `rounds`, this is a turn. On by default. See "Per-turn episodes" below. |
| `prune` | a log sink or the operator surface | one report per pruned session (`hop.session_id`, `hop.pruned_turns`, `hop.pruned_rounds`, `hop.prune_boundary`) -- or a single zero report when nothing was eligible -- or, since `collector@2.1.1`, a zero report marked `hop.degraded=1` because the store refused one of the prune chain's own reads or deletes |
| `pack` | the agent LLM | an accepted pack, as `system.*` and **no** `messages[]` beside it. Not the `brain` route: that one carries an assembled turn and is bounded by `hop.iter`, and a pack belongs to no turn and no round. A parent that wires `in_pack` MUST wire this into the brain, or every accepted pack dead-letters after this cell already told its sender it was accepted |
| `pack_ack` | back towards whoever pushed | the receipt of one pack, unconditionally: `hop.pack_owner`, `hop.pack_slots`, `hop.error_code` (empty, `slot_unknown` or `pack_empty`), `hop.pack_unknown`. Every key always present and empty rather than absent |
| `schemas` | a tools hive's `in_schemas` door | the tool names `params.tools` declares, as the whole body (`{"tools": [...]}`); `["*"]` asks for everything that hive has. It leaves on a TICK, not per turn. A parent that wires this must wire the answer back, or the tick asks into a dead letter every period |
| `menu` | the agent LLM | the menu as `system.tools` and **no** `messages[]` beside them -- durable, like `pack`, and for the same reason: an `llm` cell upserts the subtree into its own `cell.db` and it stands there until something overwrites that path. The subtree carries `$replace`, so a menu with nothing usable in it writes NOTHING rather than an empty menu that would revoke the model's whole tool set. Since `3.4.0` what travels here is not one answer but the UNION over every answerer's stored row, plus the names this hive serves itself (GH #529); `hop.menu_answerers` names the answerers it was derived from, beside `menu_count`, `menu_self` and `menu_unknown` |
| `condense` | -- | **reserved, never emitted today.** The value is declared in the enum so the fold lane can be wired later without widening a published contract; nothing in this cell writes it. |
| `cstore` | `window`, inside the hive | **interior, and it never crosses the hive path.** Every store round-trip of the state machine rides on it (`hop.phase` carries the state, `hop.turn_id` the turn). It is in the enum because the assembler emits it, and it is in no parent's wiring because the seal gives it nowhere to go. |

The enum itself is `contract.emits.hop.route` in `assemble/config.json` -- that declaration
is the authority, this table is its prose. Ten of its twelve values are the hive's declared
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
  The bundle now travels as the `memory_recall` `tool_result` of its own round -- under the
  name the member's own memory serves since [#552](https://github.com/mmeyerlein/meclaw/issues/552),
  so a model that reacts to it by calling the tool itself reaches a real cell -- where it is evidence
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
| `memory_chars` | `8000` | character cap on the memory bundle **where the bundle travels**: the synthetic `memory_recall` tool result of the AMBIENT leg. ONE cap over the whole result text, so under `memory_form: both` it bounds the readable block and the machine-readable form *together* rather than each of them separately. `hop.memory_capped` is measured on that result. The tool leg has a cap of its own, on `memory-hive/tool` (#552). |
| `max_iter` | `8` | how often a turn may re-enter the brain with a tool round. At the cap the seam leaves on `answer` instead, with `hop.partial=1` and a named partial answer as its last turn (`collector@3.5.0`, GH #570). The count belongs to ONE round, and a turn opens one: since [#541](https://github.com/mmeyerlein/meclaw/issues/541) the two turn-opening lanes (`in_turn`, `in_advice`) start at zero whatever `iter` the arrival carried. `in_advice` is the answer lane of another hive's round and carries ITS count -- a core that spent nine iterations used to hand the surface a turn that was over before it began, and the seam left on `answer` with the raw assembled round where the answer belonged, no brain call at all. |
| `round_idle_ms` | `120000` | idle window of one tool round (two minutes). A round whose last progress is older **and** whose fan-in is incomplete is closed at the next occasion with synthetic error results and fires with `hop.round_stale=1`. |
| `memory_tier` | `""` | empty = no memory leg at all, and the assembly waits for the window leg alone. `"0"` / `"1"` / `"2"` request that recall tier once per turn, and **the ambient leg arrives as a synthetic `memory_recall` result** at the end of the round -- never as durable system state (`collector@2.1.0`, GH #278). |
| `memory_form` | `"readable"` | which form of the bundle reaches the brain **in that tool result**: `readable` (the rendered block a model reads), `json` (the machine-readable bundle), `both` (the two joined by a newline, under one call id and one cap). Applies to the AMBIENT leg alone since `4.0.0` -- a model's own `memory_recall` call is rendered by `memory-hive/tool`, which has a `form` of its own ([#552](https://github.com/mmeyerlein/meclaw/issues/552)). Whatever the form, `system.memory` carries only the revocation -- the empty leaf on the fixed path `recall` plus the `$replace` marker on the node above it (see the `brain` lane, `collector@2.0.4`) -- and both halves are sent unconditionally, no longer chosen by this knob: an instance retuned from `readable` to `json` would otherwise carry its last leaf, or its last keys, for the rest of its life. |
| `async_tools` | -- | **not a collector knob.** The async class is declared once, at the dispatcher (its own `async_tools` param since `dispatcher@1.2.0`), and travels as `hop.async_calls`. |
| `prune_after_ms` | `604800000` | age gate on the prune lane (seven days). A session is pruned only when its close batch left **and** that delivery is older than this. |
| `turn_write` | `"1"` | **on by default since GH #298** -- it is the only path from a conversation into an episodes table, and a shipped "off" would be a shipped agent that remembers nothing. Every stored turn hands out one message per unwritten turn on route `turn_write`. `""` or `"0"` switch it off, and off means nothing said in this session reaches a memory *at all*, not that it reaches one later. Switch it off only where that route is unwired: an unrouted emission per turn is a dead letter per turn. |
| `inline_extraction` | `""` | **the inline extraction contract** (GH #525). Non-empty writes the shipped block to `system.instructions.sidecar` on every turn assembly -- which is what asks the brain for the ```` ```memory ```` block a memory hive's `in_remember` lane reads. It ships OFF, and that is the one place it differs from `turn_write` one row up: what takes the block back OUT of the answer is a `splitter` between the brain and the dispatcher, and this cell cannot see whether one stands behind it -- asking with nothing cutting leaves a json block in the reader's face on every turn. So the COMPOSITE decides: `talky` cuts the block and switches it on, `cogny` has no splitter and leaves it off. The write carries no `$replace` marker, so a person's charter in `instructions.reply` is untouched, and the leaf name sorts AFTER it on purpose -- an `llm` cell walks a family's leaves alphabetically and the block belongs after the answer it follows. The text is byte-identical to the fence of `templates/memory-hive/inline-contract.md`, which stays the authority. |
| `context_window` | `0` | **the curator's budget**, in tokens. `0` or empty = curation off and every byte of behaviour is the pre-wave-11 behaviour. See "The curator" below. |
| `curate_soft` | `0.5` | the working mark, as a fraction of the budget: at or above it the curator elides in stages until the projection fits under it again. |
| `curate_hard` | `0.75` | the emergency mark. It changes no behaviour of its own -- it is *reported* as `hop.curate_mark='hard'` and means the curator is out of stages. |
| `keep_rounds` | `2` | how many of the newest tool iterations stay verbatim whatever the budget says. |
| `recoverability` | `""` | what may be elided, **declared per tool name**: `read_file:repeatable,write_file:env`. Everything not named is `unique` and is never elided. |
| `thread_recall` | `"1"` | the thread tool. Empty switches it off and a call is answered with a typed error. Switching it off should mean switching curation off. |
| `thread_recall_budget` | `0.2` | the share of the budget one turn's recalls may spend. Over it the call is refused, never truncated. |
| `tool_menu` | `""` | the tool menu this collector **owns**, as the provider-native JSON array (GH #451). Empty = the menu lives in the brain's own `system.tools` and this cell writes none, exactly as before. Set it, and the declarations count towards the budget and become stage-4 candidates. A malformed value reads as empty, never as half a menu. |
| `tool_desc_chars` | `200` | how much of a stubbed declaration's description survives beside its name. |
| `tools` | `[]` | the tool names this agent **declares** it uses (GH #464), e.g. `["web_search", "web_fetch"]`; `["*"]` asks for everything the tools hive has. A comma string reads the same way. It is the OTHER half of `tool_menu`: with it set, the menu is asked for; with `tool_menu` set, it is typed and nothing is asked. Empty is the shipped default and asks nothing at all -- a collector standing in a colony with no tools hive is silent rather than noisy. |
| `curate_slot_chars` | `2000` | size above which a `system.*` slot of this cell's **own** making becomes a stage-5 candidate. The protected families are never candidates at any size. `0` switches the stage off. |
| `curate_budget_line` | `"1"` | the deterministic remaining-budget sentence in `system.budget`. `""` or `"0"` sends the leaf **empty** rather than not sending it -- `system.*` is durable, and a number left standing from the last busy turn is worse than none. |

A knob set to `null` or to a blank string means "not configured" and falls back to the default
above, so an operator who empties a line gets the shipped behaviour rather than a dead cell.
The numeric knobs also accept their value as a string, which is what a `${VAR}`-substituted
param produces.

`collector` is the **reference migration** for this move: every other template's `${VAR}`
knobs are a declared EXPERIMENTAL config surface that follows the same route onto `params`,
one template at a time (`refs #136`, `refs #138`).

### The door in the wall (`in_pack`, GH #458)

Everything above this line is a **projection of one round**, rebuilt from the ledger at
every assembly. `in_pack` is the one lane that is not, and that is the whole reason it is
a lane of its own rather than a flag on an existing one.

The `in_tool` lane drops `system.*` on purpose, and the paragraph under "What a tool
result may carry" says why at length: what leaves this seam in `system.*` is UPSERTed into
the brain's own `cell.db` and stands there until something overwrites **that exact path**.
For a tool result that is wrong -- it gets no second chance to correct itself, and a brief
about one subject would still be in the prompt three subjects later. For an *identity* it
is exactly right. The two are told apart by the LANE, so the distinction is drawn by an
edge the colony wrote rather than by a key in a body that a model could have written.

Why the lane has to exist at all: the agent composites this hive stands in (`talky`,
`cogny`) are **sealed**. An edge naming their `./brain` is refused with
`hive_port_boundary`, so every path from outside to a brain runs through this cell -- and
until GH #458 there was none for durable state. `affinity` could push and there was
nowhere to push to.

**What may be written is a closed list** -- `identity`, `persona`, `handover` and
`instructions` -- and it is a subset of `SYS_KEEP` by construction:

```python
SYS_KEEP  = ("handover", "persona", "identity", "instructions", "tools", "budget")
PACK_SLOTS = ("identity", "persona", "handover", "instructions")
```

Two subtractions are left, and both of them are the reason the subset exists. `tools` and
`budget` are re-derived here every round, so a sender writing them would fight this cell
for the same path forever.

`instructions` was a THIRD subtraction until GH #488, and the measurement is what took it
away. It was held out of the lane because an identity that could overwrite the charter
would be an identity that could rewrite what the agent is for -- which assumed the charter
had some OTHER owner. It had none: nothing exported it, no template seeded it, and a grown
generation came up with an empty charter and answered as the vendor's default assistant. A
family nobody may write is not a protected family, it is an empty one, so the charter joins
the lane. What protects it now is the lane itself rather than a hole in the lane: the door
is a route stamped by an EDGE, and that edge is drawn only through the access rule that
lets a brain draw its OWN push edge and no other, from a source whose single writer is
`affinity`'s audited gate. A charter arriving through it was curated, released by a
disclosure decision and written by somebody the audit table names.

What remains is the four families that are durable, protected from the curator at any
budget (stage 5 never touches them), and owned by nobody in here. A drift lock asserts the
subset relation rather than trusting this paragraph.

**Two body shapes, one meaning.** `system` carrying the slot subtrees is what an `llm`
cell upserts and what `affinity`'s push lane emits -- the slots and **no** turn beside
them, so the update costs the agent a write and not an inference (GH #263).
`{"slot": ..., "content": ...}` is the same for a single slot, written by hand. Both may
travel in one message; the single slot is merged over the tree.

The single-slot form is a **convenience, not a body of its own**. A UBF body has to carry
`messages` or `system` -- that is the substrate's `anyOf`, not this template's rule -- so a
hand-written slot rides beside an empty `system`:

```json
{"system": {}, "slot": "persona", "content": {"text": "terse, never chatty"}}
```

Without the `system` key the message is dead-lettered as `invalid_ubf_body` and never
reaches this cell at all. Measured, not reasoned about.

**All or nothing.** An unknown slot refuses the whole pack with `slot_unknown`, an empty
one with `pack_empty`. Writing the half that was understood would hand the sender an `ok`
it cannot tell apart from a complete write, and a half-written identity is worse than
none -- the same reason the substrate delivers a message whose pointers would not all
resolve not at all rather than half-expanded.

**The owner comes off `envelope.reply_to`**, never out of the body: a cell knows no
sender, and the only trustworthy origin in this substrate is what the substrate wrote.

**The receipt is unconditional**, accepted and refused alike, because from the sending
side a push that landed and a push that reached nothing are otherwise the same silence
(`docs/development-rules.md` § 2c). The composites above pair the two lanes in their
`required_drains`.

**`./assemble`'s own cell contract moved with the lane** (`contract.version` 1.4.0):
`messages` became **optional** in `consumes.body` and in `emits.body` alike, because a
pack carries slots and no turn in both directions and a required key would have refused it
at the delivery boundary, before this cell ever saw it. `system`, `slot` and `content` are
declared on the way in; `pack` and `pack_ack` joined the `route` enum on the way out,
together with `pack_owner`, `pack_slots`, `pack_unknown` and `error_code`. Every other
lane still sends turns; a body without them simply carries no text, which the script
already reads as the empty string.

### The menu is asked for, not typed (`in_menu`, GH #464)

`tool_menu` one table up is a list somebody wrote out. It works, and it has the property
every hand-kept list has: adding a tool to a colony means editing the prompt of every
caller that may use it, and no caller can offer a model anything nobody typed.

`3.3.0` turns that around. `params.tools` is a list of **names** -- the tools this agent's
own template says it uses -- and the schemas behind those names are **asked for**:

```json
{"add_nodes": [{"name": "scribe", "template": "collector@4.0.0",
                "override_params": {"assemble": {"tools": ["web_search", "web_fetch"]}}}]}
```

**The template is the contract, and the declaration is where the rest of the contract is.**
The tools hive keeps no table of who asks (`templates/tools/README.md` § *Asking for the
declarations*): whoever designed this agent decided what it uses, so the list lives here,
next to `window_turns` and `max_iter`, and a reader of the instance can see it.

**Two lanes, and the second one is durable state.** `schemas` carries `{"tools": [...]}` out
to the tools hive's `in_schemas` door; `in_menu` brings `schemas[]` and `unknown[]` back.
What leaves on `menu` is `system.tools` and no turn beside it -- the same shape as `pack`
one section up, and durable for the same reason: an `llm` cell upserts `system.*` per slot
path into its own `cell.db`, where it stands until something overwrites that exact path. So
the menu costs **one write per change and nothing per turn**, which is the whole reason it
is not asked in front of every assembly.

**The provider envelope is wrapped here.** The hive answers `{name, description,
parameters}` and stops there on purpose: a hive that wrapped would have to be told which
provider its caller talks to, which is a second thing every caller would have to tell it and
a first thing it would be wrong about. This cell knows its provider, so it produces
`{"type": "function", "function": {...}}` -- the same shape a typed `tool_menu` carries, read
by the same `fn_of` the curator's stage 4 reads.

**The ask has a cause, and the cause is a mutation.** The substrate hands a cell no message
at spawn, so nothing can ask "at boot" by itself. What asks is `mutation_committed` -- the receipt
the mutation door leaves at a hive named in `colony.json` (`mutation_receipts.to`), carried
down one level at a time until it docks at this hive; the hive's own door turns it into the
internal `in_menu_tick` and `./assemble` asks the tools hive for the declarations of the
tools `params.tools` names. **The boot receipt is the first one** (ruling O-0904-2), so an
agent has its menu before its first turn, and a tool ADDED to the hive by mutation reaches
this agent with the receipt of that very mutation -- nothing over there has to push, which
is exactly what that hive's contract says about asking again.

Until `4.0.0` this was a `timer` inside the hive, `./menu-clock`, ticking every five minutes
(`MENU_CRON`). It worked, and it was a poll: in an event-driven substrate a question asked
on a schedule spends availability on an answer that is already known
([#553](https://github.com/mmeyerlein/meclaw/issues/553)). An operator who wants the menu
re-asked without changing anything still has a gesture -- a message on `mutation_committed` at this
hive's own path.

**It is still the one occasion this hive acts on, and that is not a reversal.** The two
schedules it refuses -- the prune and the stale-round exit -- would DESTROY or CLOSE
somebody's turns, and deciding when that happens is an operator's business, not a
template's. A menu ask creates nothing and destroys nothing: it asks a question whose answer
overwrites one slot with the same value until something over there changes.

**An unknown name is named.** A declared name the hive has nothing under comes back in
`unknown[]`, lands in `hop.menu_unknown` on the `menu` message, and is written to stderr --
which a `code` cell puts into `log.jsonl` at warn level and flags with `had_stderr` on the
emission. A declaration pointing at nothing is a defect in this agent's own template, and
the whole value of declaring is that somebody can see it.

**`tool_menu` wins, and then nothing is asked.** A collector carrying a typed menu answers
the tick with silence. Two writers on one `system.tools` path would overwrite each other
every round, and the knob is the manual override rather than a second source.

**`./assemble`'s cell contract moved again** (`contract.version` 1.5.1): `tools`,
`schemas` and `unknown` on the way in, `tools` on the way out, `schemas` and `menu` in the
`route` enum, and `asked_count` / `menu_count` / `menu_self` / `menu_unknown` beside them.

**And the menu keeps the two tools this hive answers itself** (`3.3.1`, GH #512). Two names
on a collector's menu are not the parent's: `memory_recall` is served out of this hive's own
recall port and `thread_recall` out of its own slate, the composite around it routes both
here BY NAME, and no tools hive has -- or could have -- a declaration for it. It used
to ship as a SEED row in the brain's `cell.db`. A seed is written once, at birth; the menu
write is a `$replace`; so the first tick deleted it, and a grown agent lost its tool
minutes into its life while the chain below it stayed wired and idle.

The declaration belongs to the cell that answers the call, so this one holds it and adds
it to every menu it writes. **Whether** it does is not a new knob: it is the switch that
already decides whether the lane is answered at all instead of refused with a typed
error -- `thread_recall`. A collector that declares a tool it would refuse, and one that
refuses a tool it declared, are the same defect from two sides.

**`memory_recall` was the second name and left with `collector@4.0.0`**
([#552](https://github.com/mmeyerlein/meclaw/issues/552)), and it left for exactly the rule
above read one level out: the schema this cell typed was a projection of the MEMORY HIVE's
`in_query` contract, and this cell enforces none of the rules that contract states. The hive
declares it now, on an `in_schemas` lane of its own, and the same menu merge one section
down files it under a third answerer. Both halves went at once, the switch with the edge, or
the tree would have had exactly the defect this paragraph names.

Two properties keep the addition honest. It happens **after** the empty-menu guard and never
instead of it -- the self-served names are not evidence that the hive answered, and a menu
carrying only them would still be the revocation `$replace` makes of an empty write. And a
name the hive DID answer **wins**: a parent that wired a real cell behind `memory_recall` has
overridden this collector, and no menu declares one tool twice. What came from here rather
than from the hive rides on `hop.menu_self`.

#### One menu, several answerers (`3.4.0`, GH #529)

Everything above describes **one** question with **one** answer: parse `schemas[]`, add the
self-served names, write the lot on `system.tools` with `$replace`. That is right while
exactly one thing answers, and it is the whole defect the moment two do. The second answer
would not merge with the first -- `$replace` replaces -- it would **delete** it, and the two
answerers would take the menu away from each other on every tick, forever.

A second answerer is not hypothetical. A tool the composite reaches by an edge on its NAME is
topology of the level that draws that edge, and only the side that ANSWERS a call can declare
it: `consult_cogny` is answered by an advisor core, `web_search` by the tools hive, and
neither of them can declare the other's.

**The union needs a memory, and the memory is a table of this hive's own.** `window` carries
one more table, `menu`, with one row per answerer:

| column | what it holds |
|---|---|
| `answerer` | who delivered this submenu -- the key of the row |
| `tools` (`json`) | that answerer's declarations, already in the provider envelope |
| `unknown` | the names IT had nothing under, comma-joined -- carried rather than reported (see below) |
| `recorded_at` | when the row was written |

So `in_menu` writes no `system.tools` at all any more. It writes **one row** and reads the
whole table back **in one message**: `delete where answerer`, `insert`, `select` -- the #419
bundle form, in phase `menu-merge`. Delete-then-insert rather than an update, because the
store has no upsert and a first answer has no row to update; the `select` rides in the same
bundle, so it runs over the same connection *after* the write and sees it.

**The answerer comes off the message, never out of a body.** `context.tool_answerer` is the
mirror of the `context.tool_caller` a request already carries: the caller says who asked so
the answer comes back to the right occupant, and this says who answered so the menu can be
merged instead of overwritten. **An answer without one is the one-answerer shape** and counts
as the default answerer `"tools"` -- a tree wired before this keeps exactly one row, replaces
it on every tick, and behaves exactly as it did.

**The menu is derived, never accumulated.** The reply to the bundle sorts the rows by
`answerer` and walks them in that order, so the same rows produce the same menu on every tick
and a re-derivation is not a diff. A name two answerers both declare is taken from the
**first** of them: a menu that declared one tool twice is a menu no provider accepts, and
which of the two won has to be something a reader can predict. The self-served names of
`3.3.1` are appended after that, still only where nobody delivered them, and the result is
written with `$replace` exactly as one answer used to be.

**Both guards stand, one at each end of the round trip.** The empty-menu guard is unchanged
and still in FRONT: an answer with no usable declaration writes nothing -- and, since
`3.4.0`, **records** nothing either, so it does not overwrite that answerer's stored row with
an empty one, and the other answerers' rows stand untouched. The second guard is on the way
back: a merge that came out empty writes nothing, for the same reason the first one exists.
`$replace` over an empty menu is a revocation of the model's whole tool set.

**`hop.menu_unknown` is computed against the MERGED menu.** A declared name is a finding only
when NOBODY delivered it -- not when the answerer that happened to reply had nothing under it.
That is what makes one declared list askable of several answerers at once: a surface naming
`consult_cogny` beside its search tools asks both, the tools hive has nothing under the first
and the core nothing under the other two, and neither of those is a defect. So the `unknown[]`
of one answer travels **into its row** instead of being reported at the door, and the warn
line moved with it: it reads `collector: no answerer has a declaration for: ...`, it is
written once per **merge** rather than once per answer, and it still lands in `log.jsonl` at
warn level with `had_stderr` on the emission.

**`hop.menu_answerers`** joins `menu_count`, `menu_self` and `menu_unknown` on the `menu`
message: the sorted list of the answerers whose rows went into this menu. It is what makes a
merged menu readable at all -- `menu_count` alone cannot say whether the second answerer was
in it.

**A store refusal in a `menu` phase is a warn line and a stop, not a `degraded` answer.**
Every other phase of this cell reports a refusal on `answer`, because that is where the turn
was going anyway (see "When the store says no"). A menu has no turn beside it, so that report
would put "context assembly stopped" in front of somebody who asked nothing. A menu is durable
state, the next tick asks again, and stopping is the honest exit -- which is one more reason
the ask is a tick.

**Two answerers replying at the same time converge.** The bundle is one message and the
`store` is one task with one connection, so the two bundles run sequentially: whichever runs
second sees both rows and writes the full union. There is no lost update to guard against and
no guard row to win.

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
| 4 | the **schema** of a declaration unused for `keep_rounds` | the name and one line of description |
| 5 | the tail of an over-size `system.*` slot of this cell's own | the head, and a marker naming the cut |

Stages 4 and 5 are `collector@3.1.0`
([#451](https://github.com/mmeyerlein/meclaw/issues/451)), and they are what makes the
arithmetic honest. Until then `curate()` was handed the rounds and, for everything else, a
single integer -- `len(json.dumps(system))`, with the tool declarations in **no sum at all**.
That is the largest block in a real prompt: measured on one turn, **5267 characters of tool
declarations against 3998 characters for everything else**. A curator could therefore sit at
`hard`, out of stages, having elided every payload it was allowed to touch, while the single
biggest consumer of the window stood outside its sums. The contract is now the whole
projection -- rounds, window, `system.*` as a tree, `tools[]` as an array -- and any of it may
be left out, always against something that says how to get it back.

**Stage 4 is usage, not judgement.** A call id carries its tool name and every item carries
the iteration it belongs to, so "used recently" is a lookup over two facts the cell already
holds -- no ranking, no similarity, no model. A tool called inside the newest `keep_rounds`
iterations is one the model is working with right now, and taking its schema away mid-task is
how an agent forgets how to do the thing it is doing. `thread_recall` itself is exempt at
every budget: eliding the way back is the thrashing loop, one level up from the exemption
stage 1 already makes for a recalled result. What a stub keeps is the name, one line, and the
key: `thread_recall(call_id="tool:<name>")` -- answered straight out of the menu this cell
holds, with no store read. `parameters` becomes the empty object schema rather than
disappearing, because a declaration without it is one no provider accepts, and a curation that
produced a rejected request would have traded a large prompt for no answer at all.

**Stage 5 cuts, it does not reference.** `system.*` is upserted per slot path in the brain, so
a leaf sent from here **overwrites** the durable one -- a stub with no text would revoke
somebody's state instead of shortening one prompt. Only slots this cell re-derives from live
state every round are candidates, the cut lasts exactly one round, and the marker says so.

#### What is never touched, at any stage and any budget

- the **conversation window** -- every user and assistant turn, verbatim
- **`system.*`, the protected families** -- `instructions`, `identity`, `persona`,
  `handover`. They count towards the budget and are never candidates at any size or budget,
  which is exactly why hard constraints belong there and not in the chat: what is only in the
  conversation is compaction-mortal everywhere else. Since `collector@3.1.0` the promise is
  narrower and therefore true: not *all* of `system.*` is out of reach, but that list is, and
  it is a list read out of one constant (`SYS_KEEP`) with a test pinning it. What became
  reachable is a slot the collector itself writes and re-derives every round. Since
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
- every **tool in use** -- a declaration whose tool was called inside the newest
  `keep_rounds` iterations keeps its schema, whatever the budget says

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

#### The budget is told, not only enforced

`hop.tokens_window` is a number for the *operator*. Since `collector@3.1.0` the **generator**
is told as well: one short deterministic sentence in `system.budget`, computed out of the
numbers the report already carries.

```
Context budget: 41207 of 128000 tokens used, 86793 left.
```

No model produced it and nothing in it can drift. It is a `system` slot rather than a message
because it is a fact about the request rather than something somebody said -- and being a
`system` slot, it obeys the revocation rule: switched off (`curate_budget_line` `""` or `"0"`)
the leaf travels **empty**, because a slot that simply stops being sent stands in the prompt
forever, quoting whatever turn last wrote it. With curation itself off no leaf travels at all.

The sentence is placed in the tree, not in the order: `system_order` in the brain decides
where it lands, and a brain that wants it last names `budget` last in that list.

#### Message validity beats message count

The hard rule, and it outranks every byte this component saves: **no stage removes a row.**
Stages 1 and 2 replace a payload and leave the `tool_result` standing under its own call id,
stage 3 empties an argument string, and stages 4 and 5 do not touch `messages[]` at all -- so
every `tool_result` in the projection keeps the assistant `tool_call` that asked for it. A
body whose `tool_result` has no partner is one every provider rejects, which makes a
projection that is one row smaller and structurally invalid infinitely more expensive than the
window it was shrinking. Pinned across every budget and both menu states in
`crates/meclaw-cells/tests/w11_curator.rs`.

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
no caller could wire. A composite that carries this collector as a sub-unit and lets an OUTSIDE dispatcher
route the call has to declare the lane **at its own hive path** as well and forward it through
its door edge; `talky` does (since `talky@3.0.1`). A composite whose dispatcher is its own
does not need the door at all -- the edge never leaves the composite -- and that is what
`cogny` draws in its `params.graph`, one ordinary edge on the reserved tool name, with
its seal unchanged ([#240](https://github.com/mmeyerlein/meclaw/issues/240)).

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

`context_window` still defaults to *off* rather than to a number **in this template**, and no
longer because of the sharing edge: a curator budget is a property of the model behind the
seam, and inventing one for a template that does not know its brain would be a guess. A
composite that DOES know its brain names one -- `cogny` sets it in the
`override_params` of its collector reference, together with the `thread_recall` edge the
stubs need (GH #451).

### A cap is a preview, never a delete

Every cap in this hive is a **read-time cut**. The full tool result stays in the `round`
table, the full conversation stays in `turns`, and no cap ever removes anything -- a capped
value is a bounded preview of something the environment still holds, which is why the cut
is reported (`hop.round_dropped`, `hop.round_capped`, `hop.memory_capped`, next to the
existing `hop.window_*`) instead of happening quietly. **`hop.round_capped` is not the
iteration cap alone**: it is raised by `round_bytes` and by `tool_chars` too, on a round
that is still going. The exit is told apart by `hop.partial` (`collector@3.5.0`).

### A capped round is a partial answer (GH #570, since `collector@3.5.0`)

A round that spends its iteration budget leaves on `answer` with everything it collected.
Until `3.5.0` that was the whole of it, and the last turn of the assembly was whatever the
last tool happened to return. **The last turn is exactly what a consumer reads** -- the
shipped surfaces take the last text of an answer and put it in front of a person -- so a
core that capped mid-search handed its surface a raw `web_search` payload, and the surface
wrote it into the conversation as the reply. The better the errand was going, the more
certainly a tool got quoted at the reader.

Since `3.5.0` the seam appends one turn of its own on that branch and only on it:

```json
{"origin": "assistant", "type": "text",
 "text": "The round hit its iteration cap (max_iter=8) before an answer was written. Collected so far: 5 tool call(s) -- web_search, fetch_url. The last result began: ..."}
```

It is assembled here and never asked of a model: a round that could not finish is the one
moment another provider call is the wrong answer, and a sentence that changes with the
weather is not a marker a reader can learn. The tool names come from the `tool_call` turns
of the round in call order, deduplicated; the head of the last result is whitespace-collapsed
and cut to 200 characters. **Nothing is lost**: the raw round stays in the `round` table and
`thread_recall` still reaches it -- what changed is the last *word*.

**`hop.partial` is the marker `round_capped` could never be.** That key means two things at
once -- the round ended early, or `round_bytes`/`tool_chars` trimmed some bytes off a round
that is still going -- and only the first is an answer somebody is looking at. `partial` is
the first alone. Like every key **this message** carries it is always present (`"1"` /
`"0"`), because a CEL modifier that reads a missing key fails and a failed modifier skips
the edge. The byte caps are untouched and stamp `partial=0` on the `brain` lane.

Both keys belong to the SEAM and to nothing else: a real answer arrives on `in_answer` and
leaves on `answer` carrying neither, so a reply edge still tells a real answer from a
capped round with `!has(hop.round_capped)` -- which is what the shipped composites wire.

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

### The memory tool left this hive (`4.0.0`, [#552](https://github.com/mmeyerlein/meclaw/issues/552))

The per-turn leg above is the **free floor**: it is fired the moment a turn arrives, at a
fixed tier, before the model has read a word of it. That covers the ambient case and it
cannot cover the other one -- a question about a **time range**. The recall cell has
understood `recall_window_from` / `recall_window_to` since P15, but nothing in an agent
could ever *decide* to send them, because nobody who had seen the turn was ever the one
asking. A `memory_recall` tool call is that missing producer.

**It was served here from GH #78 to `collector@3.5.0`, and that was the wrong hive.** The
argument was that this cell is the memory specialist of its composite -- it owns the recall
port for the per-turn leg (R-OS-5) -- so the round could end where it began (R-OS-2). What
that argument left out is that the RULES a recall obeys are not this cell's: who was present
when a fact was learned, what a half-open window means, how deep a tier goes. All of them
are enforced in the memory hive. Serving the call here meant typing that hive's `in_query`
contract out by hand, in a template that answers no recall, and a second time as a seed row
in a brain -- three artefacts for one contract, each free to drift, held together by a test.

**Now the hive declares it and answers it.** `templates/memory-hive/schemas` hands out the
declaration on the hive's own `in_schemas` lane, and `templates/memory-hive/tool` turns the
call into the hive's own question and the bundle -- or the refusal -- back into one
`tool_result`. From this cell's side nothing about that is special: the dispatcher names the
tool, an edge OUTSIDE the composite knows the cell, and the result arrives on `in_tool` like
any other. The lane `in_memory_call` and the setting `memory_call_tier` are gone with it,
which is the first version digit this cell has ever spent.

**What is left here is the ambient leg**, and it is unchanged:

```jsonc
// the recall port, carrying four keys -- and no correlation, because the lane
// has ONE meaning again
{"from": "./collector", "to": "<memory hive>",
 "condition": "hop.route == 'recall'",
 "modifier": {"set_hop": {"route": "'in_query'"},
              "set_context": {"recall_query": "hop.recall_query",
                              "memory_tier": "hop.memory_tier",
                              "recall_window_from": "hop.recall_window_from",
                              "recall_window_to": "hop.recall_window_to"}}}
```

Every key is always present and empty rather than absent -- a missing hop key makes the
promoting CEL modifier fail, and a failed modifier skips the edge. The ambient bundle comes
back on `in_bundle` and reaches the brain as a SYNTHETIC `memory_recall` tool result at the
end of the round (GH #278), under the name the member's memory now really serves: a model
that reacts to it by calling the tool itself reaches a real cell instead of a void.

Discipline, unchanged in every direction:

- **The memory result counts as a normal call.** Whatever answers `memory_recall` hands
  back is a member of the round's expectation set like any other, `tool_chars` cuts it like
  any other, `round_bytes` and `max_iter` bound the round it belongs to.
- **A call nothing answers ends in the idle exit.** Without an edge from the composite to a
  memory the call is unroutable and no answer ever comes; the round then parks and is closed
  by the round idle window of GH #103 (synthetic result, `hop.round_stale=1`) -- the same
  exit a tool that died mid-flight gets. No second machinery for a memory tool, which is
  what made giving the name back cheap.
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
round-check-> complete: ROUTE brain (iter + 1)              <- the same seam
             + update round set fired=1  phase round-done   <- per ITERATION
          -> ROUTE answer (round_capped,   <- at max_iter, instead of the brain,
                          partial)             ending on a named partial answer
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

and, when an answerer answers a menu question (GH #529):

```
in_menu    -> delete menu (this answerer)  phase menu-merge <- one row per answerer,
            +  insert menu (answerer,                          keyed by
                            tools, unknown)                    context.tool_answerer
            +  select menu (every row)                      <- the #419 form: the
                                                               select sees the insert
menu-merge -> ROUTE menu                                    <- the UNION over the rows,
              (nothing else: a menu is durable                 self-served names after
               state and no turn travels beside it)            it, written with $replace
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
   `consult_cogny` in the dispatcher's `handoff_tools` makes it name the affected
   `tool_call_id`s in `hop.async_calls` **and** in `hop.handoff_calls` on the `calls` lane;
   a tool in `async_tools` alone (`remember`) is named on the first only. One
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
   | a **handoff** call took the turn with it | `hop.handoff_calls` names it (the dispatcher's `handoff_tools`) | a later turn: an advisor's event, an escalation re-entering the seam |

   **Neither, and the round stays open.** A bare fire-and-forget call -- `remember` with no
   sentence beside it -- used to be filed as fired, and the channel then got *nothing*: no
   interim, no final, no error, and `round_idle_ms` does not fire on a quiet channel.
   Measured in three of five full harness runs, and the per-turn contract (GH #298) makes
   call-only iterations common. So the acknowledgement completes the fan-in, the regular
   guard fires, and the seam re-enters the brain for the iteration the model has not spent.
   Nothing new stops it -- `params.max_iter` bounds this round like any other, and a spent
   budget leaves the same seam on route `answer` with `hop.round_capped=1` and
   `hop.partial=1`. **A round always ends in an answer**: a real one, a partial one
   (`partial`), or `degraded` (GH #343).

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
role a provider accepts mid-conversation. In the store it keeps its own role, so a batch
and a prune can still tell an event from a user's word.

**And on the wire it says so ([#540](https://github.com/mmeyerlein/meclaw/issues/540)).**
The role alone was the whole frame until then, and a role a provider knows is a role the
MODEL reads: an advisor's answer arrived byte for byte in the shape of a new sentence by
the person, and was read as one. Measured on a live colony, one "plan me a thorough
three-day trip to Athens, compare two options with numbers" turn, fresh session: the
surface answered with an interim sentence and consulted, correctly; the core's plan came
back on `in_advice`; the surface consulted a **second** time, its context quoting the core
back as *"he supplied the following plan figures, not checked live"*; and then it told the
person **"Yes. We take the cheap option."** — an answer to a question nobody had asked.
The plan, with its two options and its numbers, never left the seam. It is not the runaway
of [#539](https://github.com/mmeyerlein/meclaw/issues/539): one advice, one confusion, no
loop needed.

So the assembly frames the row for what it is, and the row alone:

```
[advice from your reasoning core, consult k-7]
cheap: flight 180, hostel 3x40 ...
```

with no id in the frame when the row carries none — a printed empty id would invite a
consult call carrying one. **Why a frame and not a role**: `tool` is the role that would
say it truly, under the `consult_cogny` call id, and it is not available here. That call
belongs to a round that has **ended**; it is not in `messages[]`, and a tool result with no
preceding call is a provider error, not a better frame. `system` mid-conversation is the
other candidate and buys nothing a text frame does not: what was missing was never the role
on its own, it was that nothing beside the text said what the text is.

**The rule travels with the ids.** Knowing a row is an advice does not yet say what to do
with one, so `system.consult.text` carries, under the open ids, the one sentence that does:
an advice is the answer to YOUR consultation, pass it on to the person in your own words,
do not consult again about it — unless the core asks YOU something back, and then you
answer the core, with its `consult_id`, never the person. It is here and not in a seed
charter for the reason [#512](https://github.com/mmeyerlein/meclaw/issues/512) and
[#525](https://github.com/mmeyerlein/meclaw/issues/525) both measured: a seed is read once
at birth, and a brain that grew — imported, rebuilt, transferred — never receives it. A
slot this cell re-derives every round reaches every `talky` standing in front of a core.
It is revoked with the ids, by the same empty rendering, for the same reason.

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
  stay untouched. Since GH #540 also the wire shape of an `advice` row: the frame and its
  correlation id, the frame without one, the person's word and the agent's own voice left
  unframed beside it, and the rule under the open ids. Since GH #541 the round budget of a
  turn-opening lane -- an `in_advice` arrival carrying `iter=9` opens its round at zero, and
  the counter-pin that a tool result still carries the round it belongs to. Since 3.5.0 the
  NAMED end of a capped round (GH #570): the last text of the answer is the digest and not
  the raw payload, the last `messages[]` entry is an assistant text turn, `hop.partial` is
  `"1"` -- and, the counter-pin, the byte cap raises `round_capped` with `partial == "0"`
  and appends nothing.
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
