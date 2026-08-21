# `steward@2.0.3`

The colony's control loop, as a hive of seven cells. It is what turns "the
system can improve itself" from a claim into something you can check.

| path | type | role |
|---|---|---|
| `charter` | `store` | goals, rules, thresholds, windows, budget caps -- as rows |
| `meter` | `code` | the deterministic measurement, read out of the colony's own ledger |
| `judge` | `llm` | evaluate, simulate, decide |
| `mutator` | `code` | re-checks the decision, then sends it to the named cell as a params update |
| `probe` | `code` | the immediate health check |
| `receipts` | `store` | one append-only row per cycle |
| `clock` | `timer` | the tick (six-field Quartz cron, **UTC**) |

## The loop

1. **Charter.** What it shall achieve, what it may do, under which conditions --
   plus everything a judge needs: thresholds, measurement windows, significance
   floors so noise never triggers action, and abort criteria. Rows, not code.
2. **Measure.** `meter` queries the ledgers the substrate already keeps --
   message log, token counts, dead letters -- read-only. No model runs in this
   path and none can: a measurement a model produced is an opinion.
3. **Judge.** The one model in the hive evaluates the numbers against the rules,
   and may first **simulate**: the ledger is append-only, so the counterfactual
   is arithmetic rather than a rerun. *Had model X served yesterday's calls at
   these token counts, the cost would have been Y.*
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

The steward has to know the way back *before* it moves, while the colony is
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
a body, so the parent draws one edge per cell the steward may touch. Which cells
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
`STEWARD_NUMERIC_PARAM_KEYS`. A key outside that set comes back as
`key_outside_radius_<key>` with a receipt, and nothing leaves.

Widening the set is an operator declaration: point it at a key whose target
really can receive one. It is not a wish — a set that names a key with no
receiver puts the silent failure back. (The reason code says
`key_outside_radius` rather than "no receiver" on purpose: this cell cannot see
a target's cell type, so it does not know whether the key it refused has a
receiver somewhere. `system_max_slots` has one and is immutable at runtime;
`max_iter` has none at all. What is true of both is that they are outside the
declared set.)

**The radius binds the way back as well.** The revert path runs the same guard as
the decide path, because a way back waved through a bound the change had to pass
is not a way back — it is a second door. So if the stored plan's key has since
left the set (an operator narrowed it while the window was open, or the row
predates it), **the revert is refused: the applied value stays standing, and the
receipt says so** — `outcome: revert_refused` with the reason code, nothing on
the `mutate` lane. The cycle is put back to `status: applied`, which is the state
it is genuinely in and the one the meter scans for, so the loop starts nothing
new until a human resolves it. A revert nobody took must never read as taken.

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

The probe fails **closed**: a probe that cannot look reports `unhealthy`, because
"found nothing" and "found it healthy" must never read the same. And it never
invents a revert -- it fetches the plan from the receipt, one select.

## Significance, and the honesty of an empty cycle

Every goal carries `min_samples` and `min_delta_pct`. Below the sample floor the
cycle closes as `skipped`, **with the count in its reason code**: *we did not
look* and *we looked and saw nothing* are different facts, and only one of them
is a reason to try again. An observe-only goal (the DLQ watch) never reaches the
judge at all -- an error rate is a symptom, and a loop that reacts to symptoms
without a hypothesis is a random walk with receipts.

## The resting state

**A freshly grown steward changes nothing.** Every goal in the seed ships
`enabled: 0`, so it measures nothing and proposes nothing until an operator
turns a row on. For a loop that reaches into the tree it runs in, that is the
only defensible default.

## Lanes

`params.ports` is empty. The address is the hive path; the lane is `hop.route`,
and it is named for what a caller asks for, never for the cell it lands on.

| lane | direction | carries |
|---|---|---|
| `in_cycle` | in → the hive | run a cycle now (a cost alert, a DLQ spike). The timer is the ordinary trigger; this is the extra one |
| `mutate` | out → the hive | a params update — body `{system:{}, params:{…}}`, `hop.target` naming the cell it is for. A parent draws one edge per cell the loop may reach |
| `error` | out → the hive | a step of the cycle could not complete |

Which cell serves a lane is this hive's business and may change without a caller
noticing. The judge in particular is unreachable from outside on any lane: a cell
that could be fed a measurement from outside would be a cell whose numbers nobody
can vouch for.

A parent that draws no `mutate` edge at all gets a steward that measures, judges
and receipts and changes nothing. That is a legitimate way to run it for a while,
and a good one for the first weeks.

**And the steward cannot draw those edges itself.** An edge is a mutation, and
this hive authors none — the whole loop reaches the outside world through one
ordinary message. Granting it a target is a boot-time act: a human puts the edge
in the seed. That is a stronger bound than any rule inside the charter, because
it does not depend on the steward behaving, and it is stricter than the old
one: an edge on to `/colony/mutations` gave the loop *every* cell at once, and
`/colony/mutations` refusing to be a mutation-drawn edge endpoint was the only
thing between it and the whole tree. That refusal is specific to the mutation
endpoint, not to `/colony/*` as a class -- the read-only `/colony/graph` has
been drawable by a mutation since
[#163](https://github.com/mmeyerlein/meclaw/issues/163).

## Relationship to `llm-registry`

**The registry is the book, the steward is the brain.** The registry stays a
passive catalogue -- what models exist, what they cost, which tier means what --
plus the assignment truth. The control loop lives here. The registry's own
runtime pin, which asserts that it contains no control loop, stays true.

Its write-hand cell is called `hand` for that reason: the name `steward` belongs
to this hive.

## Configuration

| variable | default | meaning |
|---|---|---|
| `STEWARD_CYCLE_CRON` | `0 0 */6 * * *` | the tick, UTC. A loop that ticks faster than its window can fill measures only noise |
| `STEWARD_COLONY_DB` | `colony.db` | the colony's database, opened **read-only** |
| `STEWARD_MAX_LEDGER_ROWS` | 200000 | page bound of the ledger read |
| `STEWARD_MAX_NUMERIC_STEP_PCT` | 50 | how far one cycle may move a numeric param |
| `STEWARD_NUMERIC_PARAM_KEYS` | `temperature,max_tokens,external_timeout_ms,attachment_timeout_ms` | the numeric half of the radius, as a key set. The default is the `llm` cell's runtime-mutable numeric params; a key outside it is refused with `key_outside_radius_<key>` rather than receipted as applied |
| `STEWARD_PROBE_WINDOW_SEC` | 120 | how far back the health check looks |
| `STEWARD_PROBE_MAX_ERRORS` | 0 | errors tolerated in that window |
| `STEWARD_JUDGE_MODEL` | `anthropic/claude-opus-4` | the thinking model. The one cell in the hive where a weaker model is a false economy: it decides what the colony does to itself |
| `STEWARD_JUDGE_PROVIDER` | `openrouter` | provider adapter of the judge |
| `STEWARD_JUDGE_BASE_URL` | `https://openrouter.ai/api/v1` | provider endpoint of the judge |
| `OPENROUTER_API_KEY` | — (required) | the judge's key. Bound late, never stored in the tree |

Prices live in the charter as a `price_per_mtok` rule
(`model=in/out,model=in/out`), because a colony that has to reach the network to
know what it spent cannot measure itself while the network is what broke.

## Honest limits

- **The receipts are the claim.** "Recursive" is only as good as the rows, and
  the rows are only as good as the quality metric behind the gate. A steward run
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
  What the steward knows is what it *sent*; what it measures afterwards is the
  colony's behaviour, which is the only evidence it needs and also the only one
  it has.
