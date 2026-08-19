# `steward@2.0.1`

The colony's control loop, as a hive of seven cells. It is what turns "the
system can improve itself" from a claim into something you can check.

| path | type | role |
|---|---|---|
| `charter` | `store` | goals, rules, thresholds, windows, budget caps -- as rows |
| `meter` | `code` | the deterministic measurement, read out of the colony's own ledger |
| `judge` | `llm` | evaluate, simulate, decide |
| `mutator` | `code` | re-checks the decision, then acts through the normal mutation lane |
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
4. **Decide and mutate.** Through the **normal** lane, with every gate the
   substrate applies to anybody. Never an operator override.
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

This is the rule that makes the difference between a control loop and an agent
with write access. An inverse mutation invented later, under a failing metric,
at whatever hour the window closed, is exactly the improvisation the loop exists
to avoid.

## Radius v1

Autonomous: **model choice** and **numeric params** (caps, tiers). Nothing else.

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
  the mutation.
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
turns a row on. For a loop that mutates the tree it runs in, that is the only
defensible default.

## Lanes

`params.ports` is empty. The address is the hive path; the lane is `hop.route`,
and it is named for what a caller asks for, never for the cell it lands on.

| lane | direction | carries |
|---|---|---|
| `in_cycle` | in → the hive | run a cycle now (a cost alert, a DLQ spike). The timer is the ordinary trigger; this is the extra one |
| `mutate` | out → the hive | `hop.msg_type == 'mutation'`, the body carrying `scope` and `diff`. A parent carries it on to `/colony/mutations` |
| `error` | out → the hive | a step of the cycle could not complete |

Which cell serves a lane is this hive's business and may change without a caller
noticing. The judge in particular is unreachable from outside on any lane: a cell
that could be fed a measurement from outside would be a cell whose numbers nobody
can vouch for.

A parent that does not draw `mutate` on to `/colony/mutations` gets a steward that
measures, judges and receipts and changes nothing. That is a legitimate way to run
it for a while, and a good one for the first weeks.

**And the steward cannot draw it itself.** `/colony/*` is a virtual endpoint
rather than a registry node, so `add_edges` refuses it by name — at any scope,
from any cell, including from this hive's own mutator. Granting the loop its
mutation lane is a boot-time act: a human puts that edge in the seed. That is a
stronger bound than any rule inside the charter, because it does not depend on
the steward behaving.

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
| `STEWARD_PROBE_WINDOW_SEC` | 120 | how far back the health check looks |
| `STEWARD_PROBE_MAX_ERRORS` | 0 | errors tolerated in that window |
| `STEWARD_JUDGE_MODEL` | `anthropic/claude-opus-4` | the thinking model |

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
- **No operator lane, ever.** If the steward's mutation is rejected by a gate,
  that is the answer. A loop that could override the gates it runs under would
  not be governed -- it would merely be polite.
