# `argus@1.0.0`

The colony's watcher and its control loop, as a hive of seven cells. It is what
turns "the system can improve itself" from a claim into something you can check.

> **Renamed from `steward` in [#462](https://github.com/mmeyerlein/meclaw/issues/462),
> and the name is the point.** A steward tends; an argus *sees*. This hive
> leaves a receipt for every tick of its clock — including the tick that found
> nothing to do and the cycle that died at a store reject — so the receipts
> table is an unbroken chain rather than a list of the moments something was
> decided. `steward` still ships for one release and takes no further work; a
> new control loop is an `argus`.

| path | type | role |
|---|---|---|
| `charter` | `store` | goals, rules, thresholds, windows, budget caps -- as rows |
| `meter` | `code` | the deterministic measurement, asked of the colony's own ledger |
| `judge` | `llm` | evaluate, simulate, decide |
| `mutator` | `code` | re-checks the decision, then sends it to the named cell as a params update |
| `probe` | `code` | the immediate health check |
| `receipts` | `store` | one append-only row per cycle |
| `clock` | `timer` | the tick (six-field Quartz cron, **UTC**) |

## The loop

1. **Charter.** What it shall achieve, what it may do, under which conditions --
   plus everything a judge needs: thresholds, measurement windows, significance
   floors so noise never triggers action, and abort criteria. Rows, not code.
2. **Measure.** `meter` **asks** the colony for counts over one window --
   `/colony/ledger`, an ordinary message -- and gets **aggregates** back: sums
   and counts over the message log, the token columns, the dead-letter queue,
   grouped by model. No rows and no header contents; nothing in this hive opens
   a database it does not own. The property that matters is unchanged: the
   numbers are arithmetic over what already happened. No model runs in this
   path and none can -- a measurement a model produced is an opinion.
3. **Judge.** The one model in the hive evaluates the numbers against the rules,
   and may first **simulate**: the ledger is append-only, so the counterfactual
   is arithmetic rather than a rerun. *Had model X served yesterday's calls at
   these token counts, the cost would have been Y.* Its charter — role, method,
   radius, the revert-plan rule, the quality gate — and the single tool it may
   answer with are **seeded** into its `system` tree
   (`judge/seed/system.jsonl`), which is the only route a template has into it.
   Until [#342](https://github.com/mmeyerlein/meclaw/issues/342) they sat in
   `params` instead, where the `llm` cell has no such fields and dropped both in
   silence: the charter that says *answer with exactly one tool_call to
   `argus_change`* was addressed to a model that had been shown neither the
   tool nor the charter.
4. **Decide and update params.** The decided change leaves as an **ordinary
   params update** — body `{system:{}, params:{…}}`, `hop.target` naming the
   cell — which the target merges into its live params and answers with nothing
   at all. No inference, no provider call, no `config.json` rewrite. Never an
   operator override, and never a mutation: nothing in this hive authors a diff.
5. **Health check.** `probe` looks immediately, in seconds.
6. **Measure the effect.** Over the charter's window. Improvement proven by more
   than the significance floor → keep. Not proven → **revert**, by the explicit
   inverse the judge authored beforehand.
7. **Receipt.** Every cycle appends what was measured, judged, simulated,
   changed, verified, and kept or reverted -- including the cycles where nothing
   happened.

## The charter rule

> **A cycle without a pre-authored revert plan is invalid.**

The argus has to know the way back *before* it moves, while the colony is
still healthy and the thinking is still clear. `mutator` enforces it and does
not settle for a well-formed one: the plan must name the same target and the
same key, and it must restore the **original** value. A "revert" pointing at the
value we are moving to would pass every structural check and undo nothing.

And if a plan is missing anyway — the row predates the rule, a hand-edited
receipt lost it — the way back does not get improvised, and just as importantly
the row does not get to claim one happened: the cycle closes as
`revert_refused` / `revert_plan_missing_at_revert_time` and goes back to
`status: applied`, because that is what the colony is. `reverted` is a word
about the colony, not about the loop's intentions.

This is the rule that makes the difference between a control loop and an agent
with write access. An inverse change invented later, under a failing metric,
at whatever hour the window closed, is exactly the improvisation the loop exists
to avoid.

## Radius v1

Autonomous: **model choice** and **numeric params** (caps, tiers). Nothing else,
and the mechanism is the reason rather than the rule: a params update can only
move a param of one cell. It cannot add a cell, remove one, or move an edge,
because nothing in this hive authors a mutation diff at all.

The radius has a second, harder edge: **the loop reaches only the cells a human
wired it to.** `hop.target` names the cell, but an edge cannot be computed from
a body, so the parent draws one edge per cell the argus may touch. Which cells
are in range is therefore a fact about the seed, not about what the judge wrote
down — the same bound `llm-registry` puts on its own push lane.

### It is `llm` params, and that is a limit rather than a description

A params update is merged by the cell types that **have** a params lane — `llm`,
`store`, `timer`, `mcp`, `proxy`, `subcolony`, `harness`. A **`code` cell has
none**: an overlay addressed there is not merged, not persisted, and not refused
either — the cell would take the body as ordinary input and run its script on
it. So the radius in practice is the `llm` cell's runtime-mutable params: the
model, and the numeric knobs beside it (`temperature`, `max_tokens`,
`external_timeout_ms`, `attachment_timeout_ms`).

**Numeric caps that live on a `code` cell are out of radius today.** The
collector's `max_iter` is the shipped example, and it is exactly the case that
has to be refused rather than sent: a cycle that pushed an overlay nobody merges
and then receipted `applied` would be a loop reporting improvements over a
colony where nothing moved. The mutator cannot see a target's cell type — it has
a path and a key — so it checks the half it can check honestly, the KEY, against
`ARGUS_NUMERIC_PARAM_KEYS`. A key outside that set comes back as
`key_outside_radius_<key>` with a receipt, and nothing leaves.

Widening the set is an operator declaration: point it at a key whose target
really can receive one. It is not a wish — a set that names a key with no
receiver puts the silent failure back. (The reason code says
`key_outside_radius` rather than "no receiver" on purpose: this cell cannot see
a target's cell type, so it does not know whether the key it refused has a
receiver somewhere. `system_max_slots` has one and is immutable at runtime;
`max_iter` has none at all. What is true of both is that they are outside the
declared set.)

**The check binds the way back as well.** The revert path runs the same
change-shaped validation as the decide path — absolute target, radius, a value
that is actually there and actually numeric — because a way back waved through a
bound the change had to pass is not a way back, it is a second door. So a stored
plan whose key has since left the set (an operator narrowed it while the window
was open, or the row predates it), and equally one that is degenerate in its own
right (no value, a relative target, no target at all), is **refused: the applied
value stays standing, and the receipt says so** — `outcome: revert_refused` with
the reason code, nothing on the `mutate` lane. The cycle is put back to
`status: applied`, which is the state it is genuinely in and the one the meter
scans for, so the loop starts nothing new until a human resolves it. A revert
nobody took must never read as taken.

A topology change is not executed -- the judge may raise one, and it is recorded
as a `proposed` cycle for a human to read. A change outside the radius comes
back as `outside_radius` with a receipt, which is the normal case for a model in
this position rather than an exception.

The radius widens by **editing the charter row**, never by editing code. That is
the property worth having: what the loop may do is legible to somebody who does
not read Python.

A numeric step is additionally capped (50% by default). One cycle's mistake has
to stay small enough that the next cycle can measure its way back out.

## Quality is a gate, not a wish

A cheaper colony that answers worse has not improved; it has been degraded.
`quality_floor_pct` says how far the quality metric may move against us before a
change is reverted, and zero is the default. This makes the loop directly
dependent on the quality metric being trustworthy -- which is a real dependency
and named as one, not a footnote.

## Two clocks, on purpose

- The **probe** answers *is the colony still working* -- in seconds, right after
  the change went out.
- The **window** answers *was that a good idea* -- in hours, over real traffic.

They fail on different timescales. A loop with only the second one leaves a
broken colony broken while it patiently collects evidence.

The probe asks three mechanical questions of the colony's own ledger: **did this
cycle's params update reach the cell it names**, has the colony produced errors
since, and did anything land in the dead-letter queue. It also counts how many
messages the named cell exchanged in the window and writes that count into the
receipt -- but it does **not** judge it: silence is not a verdict here, and a
number the reader can see is worth more than a threshold nobody chose. The
first question is about the params update
rather than about a committed mutation on purpose -- this hive authors no diff at
all, so a `mutation_log` answer would be a verdict on a mechanism the loop does
not use, and it read `unhealthy` for every healthy cycle
([#338](https://github.com/mmeyerlein/meclaw/issues/338)). Because that ledger
row can trail the probe by milliseconds -- the update and the probe order leave
in one batch -- the question is **asked again** a bounded number of times
(`ARGUS_PROBE_LEDGER_TRIES`, one round trip per try, 100 ms apart) before the
update counts as missing.

The probe fails **closed**, and that verdict is read before the three: a probe
that cannot look at the ledger at all reports `unhealthy` with
`probe_unavailable`, because "found nothing" and "found it healthy" must never
read the same. And it never
invents a revert -- it fetches the plan from the receipt, one select.

## Significance, and the honesty of an empty cycle

Every goal carries `min_samples` and `min_delta_pct`. Below the sample floor the
cycle closes as `skipped`, **with the count in its reason code**: *we did not
look* and *we looked and saw nothing* are different facts, and only one of them
is a reason to try again. An observe-only goal (the DLQ watch) never reaches the
judge at all -- an error rate is a symptom, and a loop that reacts to symptoms
without a hypothesis is a random walk with receipts.

That path is deterministic all the way through, and since #462 it says what it
saw: the reason code carries the metric and its count (`observed_dlq_rate_3`,
`observed_dlq_rate_clean`), and a non-zero count also leaves the hive on the
`alert` lane. Both halves matter. A single `observe_only` code for every
observation made a colony losing letters and a colony losing none read alike;
a watcher that sees something and tells nobody is a log file with extra steps.
**No model is reached anywhere on this path** — the judge is asked about
optimisation goals and about nothing else, which is also why a colony in trouble
does not start paying for opinions about it.

## The resting state

**A freshly grown argus changes nothing.** Every goal in the seed ships
`enabled: 0`, so it measures nothing and proposes nothing until an operator
turns a row on. For a loop that reaches into the tree it runs in, that is the
only defensible default.

**It is not, however, a silent one.** A tick against an inert charter writes a
receipt with `outcome: idle` and `reason_code: no_enabled_goal`. Before #462 it
wrote nothing at all, which meant a watcher whose clock had stopped and a
watcher with an empty charter produced the same empty table, and no reader could
tell them apart. The evidence that the loop is running must not be the same
evidence as that it decided something.

## The chain has no holes

Three receipts exist so that every tick lands in the table:

| `outcome` | when |
|---|---|
| `idle` | the charter had no enabled goal |
| `store_error` | a store refused an operation of this cycle; the reason code names the operation and the code |
| every other status | the cycle it belongs to — `applied`, `kept`, `reverted`, `skipped`, `observed`, `refused`, `no_action`, `proposed`, `revert_refused`, `unhealthy_no_plan` |

The `store_error` write is bounded by its own phase: it goes out on `serr`, so a
refusal *of the receipt* leaves on the error lane and writes no second row. One
step, never two — the meter has no memory to keep a counter in, so the phase is
the counter.

## Lanes

`params.ports` is empty. The address is the hive path; the lane is `hop.route`,
and it is named for what a caller asks for, never for the cell it lands on.

| lane | direction | carries |
|---|---|---|
| `in_cycle` | in → the hive | run a cycle now (a cost alert, a DLQ spike). The timer is the ordinary trigger; this is the extra one |
| `mutate` | out → the hive | a params update — body `{system:{}, params:{…}}`, `hop.target` naming the cell it is for. A parent draws one edge per cell the loop may reach |
| `alert` | out → the hive | a watched symptom crossed zero, counted deterministically and with no model asked: the metric, the goal that watched for it, the count and the window |
| `error` | out → the hive | a step of the cycle could not complete — including the one state this loop cannot leave on its own: an unhealthy colony whose applied change carries no revert plan |

Which cell serves a lane is this hive's business and may change without a caller
noticing. The judge in particular is unreachable from outside on any lane: a cell
that could be fed a measurement from outside would be a cell whose numbers nobody
can vouch for.

A parent that draws no `mutate` edge at all gets an argus that measures, judges
and receipts and changes nothing. That is a legitimate way to run it for a while,
and a good one for the first weeks.

**And the argus cannot draw those edges itself.** An edge is a mutation, and
this hive authors none — the whole loop reaches the outside world through one
ordinary message. Granting it a target is a boot-time act: a human puts the edge
in the seed. That is a stronger bound than any rule inside the charter, because
it does not depend on the argus behaving, and it is stricter than the old
one: an edge on to `/colony/mutations` gave the loop *every* cell at once, and
`/colony/mutations` refusing to be a mutation-drawn edge endpoint was the only
thing between it and the whole tree. That refusal is specific to the mutation
endpoint, not to `/colony/*` as a class -- the read-only `/colony/graph` has
been drawable by a mutation since
[#163](https://github.com/mmeyerlein/meclaw/issues/163).

`/colony/ledger` is the second endpoint in that class, and the argus's two
edges on to it (`./meter` and `./probe`) are the reason this hive no longer
opens `colony.db`
([#267](https://github.com/mmeyerlein/meclaw/issues/267)). It is drawable by a
mutation for the same reason `/colony/graph` is: it grants **counts rather than
reach**. An edge on to it lets a cell ask how much happened in a window; it does
not let it read a row, a header or another cell's state, and it moves nothing.
The two of them are the whole list -- a `/colony/*` path is not sanctioned by
being one, and the argus's own edge test asserts against the literal pair.

## Relationship to `llm-registry`

**The registry is the book, the argus is the brain.** The registry stays a
passive catalogue -- what models exist, what they cost, which tier means what --
plus the assignment truth. The control loop lives here. The registry's own
runtime pin, which asserts that it contains no control loop, stays true.

Its write-hand cell is called `hand` for that reason: the name `argus` belongs
to this hive.

## Configuration

| variable | default | meaning |
|---|---|---|
| `ARGUS_CYCLE_CRON` | `0 0 */6 * * *` | the tick, UTC. A loop that ticks faster than its window can fill measures only noise |
| `ARGUS_MAX_LEDGER_ROWS` | 200000 | the `scan_budget` the meter **asks** `/colony/ledger` for: the hard bound on the rows each windowed sub-query may read. Since [#385](https://github.com/mmeyerlein/meclaw/issues/385) an answer that hit the bound is **discarded** -- the cycle is receipted as not measured rather than ruled on a part of the window -- so a budget smaller than the colony's traffic in one window means the meter never measures at all and the loop stands still. Generous is the safe direction |
| `ARGUS_MAX_NUMERIC_STEP_PCT` | 50 | how far one cycle may move a numeric param |
| `ARGUS_NUMERIC_PARAM_KEYS` | `temperature,max_tokens,external_timeout_ms,attachment_timeout_ms` | the numeric half of the radius, as a key set. The default is the `llm` cell's runtime-mutable numeric params; a key outside it is refused with `key_outside_radius_<key>` rather than receipted as applied |
| `ARGUS_PROBE_WINDOW_SEC` | 120 | how far back the health check looks |
| `ARGUS_PROBE_MAX_ERRORS` | 0 | errors tolerated in that window |
| `ARGUS_PROBE_LEDGER_TRIES` | 3 | how often the health check re-**asks** the ledger -- one round trip per try, 100 ms apart -- before it calls the cycle's params update missing. Closes the write-lag race against a row that is still being written |
| `ARGUS_JUDGE_MODEL` | `anthropic/claude-opus-4` | the thinking model. The one cell in the hive where a weaker model is a false economy: it decides what the colony does to itself |
| `ARGUS_JUDGE_PROVIDER` | `openai` | provider adapter of the judge. `openai` is the only value `LlmParams` accepts today; it names the Chat-Completions **wire**, not the vendor, and the endpoint it talks to is `ARGUS_JUDGE_BASE_URL` ([#387](https://github.com/mmeyerlein/meclaw/issues/387)) |
| `ARGUS_JUDGE_BASE_URL` | `https://openrouter.ai/api/v1` | provider endpoint of the judge |
| `OPENROUTER_API_KEY` | — (required) | the judge's key. Bound late, never stored in the tree |

**Retracted:** `ARGUS_COLONY_DB` is gone
([#267](https://github.com/mmeyerlein/meclaw/issues/267)). The meter and the
probe no longer open `colony.db` -- nothing in this hive opens a database it
does not own -- and the counts arrive over `/colony/ledger` instead. Setting it
does nothing; a tree that still sets it is not broken, it is merely talking to
a version that has passed.

Prices live in the charter as a `price_per_mtok` rule
(`model=in/out,model=in/out`), because a colony that has to reach the network to
know what it spent cannot measure itself while the network is what broke.

## Honest limits

- **The receipts are the claim.** "Recursive" is only as good as the rows, and
  the rows are only as good as the quality metric behind the gate. An argus run
  against a metric nobody trusts produces confident nonsense with excellent
  bookkeeping.
- **One change at a time.** The loop refuses to open a second cycle while an
  applied one is still unmeasured -- otherwise neither change could be
  attributed. That makes it deliberately slow.
- **The push is eventual.** A model swap is a params update; calls already in
  flight finish on the old model.
- **No operator lane, ever.** The change is an ordinary message, so a cell that
  refuses the overlay -- an immutable key, an unknown one -- refuses it, and that
  is the answer. A loop that could override the gates it runs under would not be
  governed; it would merely be polite.
- **And the loop never learns whether the change landed.** A params update is
  fire-and-forget into a cell that answers nothing, exactly as in `llm-registry`.
  What the argus knows is what it *sent*; what it measures afterwards is the
  colony's behaviour, which is the only evidence it needs and also the only one
  it has.
- **An answer that never arrives leaves the cycle unverified.** Since the counts
  are asked for rather than read, the probe has to wait for them -- and it
  cannot hear its own silence: it has no `cell.db` and no timer, so a reply that
  is simply never sent leaves the cycle sitting where it was. What bounds that
  is only that the endpoint always answers *something*. There are **three** ways
  an answer can fail to be one, and all three read as *could not look*, never as
  *looked and saw nothing*: `unavailable` when the read itself failed,
  `invalid_query` when a filter could not be read
  ([#341](https://github.com/mmeyerlein/meclaw/issues/341)/[#359](https://github.com/mmeyerlein/meclaw/issues/359)),
  and `scan_truncated` when a windowed sub-query hit its budget and the counts
  beside it cover only a part of the window
  ([#385](https://github.com/mmeyerlein/meclaw/issues/385)). Each of them is
  fail-closed: the probe answers `unhealthy` / `probe_unavailable`, the meter
  receipts `unmeasured`.
- **A colony busier than the scan budget reverts everything.** Fail-closed has a
  price, and this is where it is paid. If one window holds more ledger rows than
  the budget (200000 at the ceiling), every answer comes back
  `scan_truncated` -- so every probe reads `probe_unavailable`
  (`scan_truncated: partial counts`), every change is reverted, and the meter
  measures nothing at all, until the window is made shorter or the traffic
  falls. The read is not free either: `/colony/ledger` **stalls the colony's
  inbox loop for its duration**. Since #462 the `mutation_log` half of it reads
  a `created_at` index like the other two windowed tables, where it used to be a
  full table scan whose only bound was that same budget — but an argus pointed
  at a busy colony with a long window is still asking the substrate to stop and
  count.
- **A refusal cannot be attributed to a cycle.** An `invalid_query` refusal
  carries no `query` echo at all, and the echo is the probe's entire memory --
  it has no `cell.db` to hold an ask in. So for that one ask the verdict **and
  the revert are lost**: nothing is written against the cycle, and nobody is
  told which cycle it was. The direction stays safe (a lost verdict is never a
  healthy one), and the trigger can only ever be a defect of the probe itself,
  because the query is built entirely from values the probe already holds. But
  a broken probe fails *quietly per cycle* rather than loudly once.
- **"The params update was seen" is a count, not a row.** It now means *a
  message of this cycle arrived under this path prefix inside the window*,
  counted by the substrate. That is narrower than the old row-level test in one
  way -- the `hop.route == "mutate"` fallback for rows without a cycle id is
  gone, on the evidence that the mutator stamps one on every update it emits --
  and wider in another, because `path_prefix` is a **prefix**: a target of
  `/main/talky/brain` counts `/main/talky/brainstem`'s traffic too. It can
  therefore mask a cell that went silent. It cannot invent a healthy verdict on
  its own: errors and dead letters are counted independently of it. And the
  re-read is now a re-**ask**: up to `ARGUS_PROBE_LEDGER_TRIES` round trips
  100 ms apart, so a colony under load fails the check later than it used to,
  never earlier.
- **The window ends at the current second, and that had to be said out loud.**
  `/colony/ledger` bounds a window as `since <= created_at < until`, in whole
  seconds, and `until` defaults to the endpoint's own `now` — so a window that
  names no upper bound cannot see anything the colony logged during the second
  it is asked in. Harmless for a cost goal measured over a day; not harmless for
  the probe, which asks milliseconds after the update it is looking for was
  emitted. Both asks name `until` explicitly since #462, and both name `now + 1`,
  because against an exclusive bound that is how *up to and including this
  second* is spelled.
- **A target that refuses the params update is caught but not named.** A cell
  that answers `consumes_violation` or `invalid_input` to the overlay replies
  straight to `./mutator`, and that reply is a fresh message with no `context`
  of ours — so the mutator can tell that something was refused but not WHICH
  cycle it belonged to. It therefore writes nothing (a refusal is not a ruling),
  and the cycle is caught one hop later by the probe instead: the refusal is a
  colony error inside the health window, the verdict is `unhealthy`, and the
  revert plan is taken. Safe, and coarser than it should be. Attributing it
  needs a correlation the substrate does not hand a `code` cell today.
