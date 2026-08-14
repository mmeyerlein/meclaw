# `collector@1.0.0`

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
- **The memory bundle, through one door.** With `COLLECTOR_MEMORY_TIER` set, every turn
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
  seam that started the round (`COLLECTOR_MAX_ITER`) -- not by a TTL that dies silently,
  and not by an edge that has to know about iterations.
- **A deterministic exit for a round that cannot complete.** A result lost in flight used
  to park the fan-in forever (GH #103). Now a round whose last progress lies behind
  `COLLECTOR_ROUND_IDLE_MS` is closed at the next occasion: the missing calls get
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
  than `COLLECTOR_PRUNE_AFTER_MS`. Without a ledger row nothing is ever pruned: rather
  grow than silently lose a turn. The cut is reported on the hop, per session.

## Cells

| cell | type | what it holds |
|---|---|---|
| `assemble` | `code` | the whole state machine: ten entry lanes, the fan-in gate, the eviction policy, the seam, the round-robustness exits, the prune chain |
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
| `in_answer` | the brain, on `finish_reason == 'stop'` | writes the answer into the window and lets it out |
| `in_close` | the session keeper, on `hop.route == 'close'` | reads the whole session back and batches it out |
| `in_prune` | a timer or an operator, on `hop.route == 'prune'` | prunes delivered-and-aged sessions; the template **never fires this itself** |
| `in_round_sweep` | a timer or an operator, on `hop.route == 'sweep'` | re-checks every open tool round and closes the stale ones; equally **never fired by the template itself** |

Exits leave **from `./assemble`** on `hop.route`:

| route | to | notes |
|---|---|---|
| `brain` | the agent LLM | THE seam. Promote `hop.turn_id`, `hop.session_id` and `hop.iter` to context on this edge. `system.consult.open` carries the correlation ids of the advice turns still in the window. |
| `answer` | the reply sink | the brain's final turn, after it is in the window -- **or** a turn that reached `COLLECTOR_MAX_ITER`, marked `hop.round_capped=1` |
| `recall` | the memory hive's recall port | the per-turn leg (only when `COLLECTOR_MEMORY_TIER` is set) **and** every `memory_recall` call; promote `recall_query`, `memory_tier`, `memory_call_id`, `recall_window_from`, `recall_window_to`, `session_id`, `turn_id`, `iter` |
| `write` | wherever a closed session belongs | one batch per close: `messages[]` the whole conversation, the raw round rows in the top-level slot `rounds` |
| `prune` | a log sink or the operator surface | one report per pruned session (`hop.session_id`, `hop.pruned_turns`, `hop.pruned_rounds`, `hop.prune_boundary`) -- or a single zero report when nothing was eligible |

Wire the ports in the **same mutation** that instantiates the hive: an island without a
crossing edge derives inactive and never spawns.

## Knobs

| env var | default | meaning |
|---|---|---|
| `COLLECTOR_WINDOW_TURNS` | `12` | how many of the newest turns enter the context. The cut runs in the store (`order by id desc`, `limit`). |
| `COLLECTOR_WINDOW_BYTES` | `8000` | byte cap over the turn texts, counted from the newest turn backwards. Whole turns are dropped, never truncated. |
| `COLLECTOR_TURN_CHARS` | `4000` | per-turn character cap applied before the byte cap, so one pathological turn cannot eat the window. |
| `COLLECTOR_TOOL_CHARS` | `4000` | per-item character cap on tool **result** texts before they enter the seam. |
| `COLLECTOR_ROUND_BYTES` | `16000` | byte cap over the whole tool round, counted from the newest iteration backwards. What does not fit falls as a whole **iteration**. |
| `COLLECTOR_MEMORY_CHARS` | `8000` | character cap on the memory bundle as it enters `system.memory` -- the readable block and every `{text}` leaf of the machine-readable form. |
| `COLLECTOR_MAX_ITER` | `8` | how often a turn may re-enter the brain with a tool round. At the cap the seam leaves on `answer` instead. |
| `COLLECTOR_ROUND_IDLE_MS` | `120000` | idle window of one tool round (two minutes). A round whose last progress is older **and** whose fan-in is incomplete is closed at the next occasion with synthetic error results and fires with `hop.round_stale=1`. |
| `COLLECTOR_MEMORY_TIER` | (empty) | empty = no memory leg at all, and the assembly waits for the window leg alone. `0` / `1` / `2` request that recall tier once per turn. |
| `COLLECTOR_MEMORY_FORM` | `readable` | which form of the bundle reaches the brain: `readable` (the rendered block a model reads), `json` (the machine-readable bundle), `both`. Applies to the ambient leg and to a `memory_recall` result alike. |
| `COLLECTOR_MEMORY_CALL_TIER` | `1` | recall tier of the **memory tool** (GH #78). Configuration, never a model argument. Empty switches the tool off: a call is then answered with a typed error result instead of being asked into a void. |
| `COLLECTOR_ASYNC_TOOLS` | -- | **not a collector knob.** The async class is declared once, at the dispatcher (`DISPATCHER_ASYNC_TOOLS`), and travels as `hop.async_calls`. |
| `COLLECTOR_PRUNE_AFTER_MS` | `604800000` | age gate on the prune lane (seven days). A session is pruned only when its close batch left **and** that delivery is older than this. |

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
  older than `COLLECTOR_PRUNE_AFTER_MS` (default seven days) and cuts, per session, the
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
- A round whose progress lies behind `COLLECTOR_ROUND_IDLE_MS` **and** whose fan-in is
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
recall port's own keys, and takes the **tier** from `COLLECTOR_MEMORY_CALL_TIER`. A tier
is a cost decision of the tree, not something a model gets to raise from inside a prompt.

Discipline, unchanged in every direction:

- **It counts as a normal call.** The `memory_recall` id is a member of the round's
  expectation set like any other, `COLLECTOR_TOOL_CHARS` cuts its result like any other,
  `COLLECTOR_ROUND_BYTES` and `COLLECTOR_MAX_ITER` bound the round it belongs to.
- **A call that cannot be served is answered, never parked.** With
  `COLLECTOR_MEMORY_CALL_TIER` empty the collector answers the call itself with a typed
  error result -- the dispatcher-lid pattern, one lane further in.
- **A port that is not wired ends in the idle exit.** Without the `recall` edge the
  request is unroutable and no answer ever comes; the round then parks and is closed by
  the round idle window of GH #103 (synthetic result, `hop.round_stale=1`) -- the same
  exit a tool that died mid-flight gets. No second machinery for a memory tool.
- **The ambient tier-0 bundle stays the free floor.** It does not step aside when the
  model asks for itself: the two are different questions (what is always true about this
  person vs. what happened between these two dates), and a turn that pays for both is a
  turn whose model asked for the second one on purpose.

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
`COLLECTOR_MAX_ITER`, which is why a runaway round ends in a message on the `answer` lane
rather than in a silence. Hop table and derivation:
[`docs/store-backed-tool-loop.md`](../../../docs/store-backed-tool-loop.md).

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
          -> ROUTE answer (round_capped) <- at COLLECTOR_MAX_ITER, instead of the brain
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
fan-in that waited for it would be betting `COLLECTOR_ROUND_IDLE_MS` against thinking
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
- `crates/meclaw-cells/tests/collector_colony.rs` -- a running colony with no memory hive
  in it at all, so a turn that references an earlier turn can only have been answered from
  the window; plus a 100 KB tool result that arrives capped, a runaway round that the seam
  ends, a session that leaves as one batch, a batched session that is pruned while
  the living session keeps every byte, a lost tool result whose round a sweep closes, and
  a mid-round turn that defers and rides with the next assembly. Two more trees run the
  memory tool against the **shipped `dispatcher@1`** -- one with the recall port wired
  (both results fan in, the model's time range reaches the request) and one without it
  (the call is unroutable and the round ends in the idle exit).
