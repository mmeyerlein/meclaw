# `clock@1.0.1`

A periodic tick, as one `timer` cell. It exists because of a gap the library had rather
than the substrate: seven shipped templates carry a `timer` inside them -- `access/clock`,
`argus/clock`, `affinity/clock`, `memory-hive/clock`, `session-keeper/night`, `canvy/clock`,
`daily-digest/clock` -- and **not one of them was instantiable on its own**. There were nine
until [#553](https://github.com/mmeyerlein/meclaw/issues/553): `collector/menu-clock` and
`colony-view/refresh` polled for a change the mutation door can simply announce, and both
are gone. An `add_nodes` entry requires a
`name` and a `template`, there is no form for a bare cell, and so a running colony could
not be given a periodic tick by any manifest, by any caller, through any door
([#484](https://github.com/mmeyerlein/meclaw/issues/484)).

That is the whole template: one cell, one schedule, and no decision.

## What it delivers

- **A tick a declaration can ask for.** `{"name": "sweeper", "template": "clock"}` and
  one edge, in the same mutation, and a hive that measured time only when somebody
  knocked measures it on the minute.
- **A cadence that is a parameter two ways.** `override_params` on the node at
  instantiation (a flat params object -- a single-cell template has no inner cell to
  address), and an `op` message at runtime --
  `{"op": "modify", "schedule_id": "...", "cron": "0 0 * * * *"}` retunes a running clock
  without a restart. There was a third, `CLOCK_CRON`, and it is gone since 1.0.1: an
  environment knob is colony-wide, which is precisely what a clock must not be.
- **A firing that says which schedule fired and when.** Six auto headers ride every tick:
  `event_id`, `schedule_id`, `schedule_name`, `scheduled_at`, `fired_at` and
  `iteration_n`.
- **No opinion whatsoever.** The body is empty and the lane is unnamed. What a tick means
  is the parent edge's `set_hop`, which is what lets the same template drive a sweep in
  one place and a trim in another.

## The cell

| cell | type | what it holds |
|---|---|---|
| the template root | `timer` | one repeating schedule in its own `cell.db`. No content, no condition, no knowledge of what it drives. |

Single-cell template (one cell of one cell type, the smallest `config.json` that starts
it, and a README that explains its declarations): instantiate it under a name that says
what it drives, and the instance IS the cell.

## Ports and wiring

A clock has no entry lane -- it is a producer, and the only messages it consumes are its
own schedule ops. Exactly one thing leaves it, and **the edge is what gives it a
destination**: `emit_to` is `"."`, so a tick nobody wired dead-letters as `no_route`
naming the clock itself rather than vanishing.

```json
[
  { "from": "./sweeper", "to": "./firewall",
    "condition": "has(hop.schedule_name) && hop.schedule_name == 'tick'",
    "modifier": {"set_hop": {"route": "'in_sweep'"}} }
]
```

| field | meaning |
|---|---|
| `hop.schedule_name` | always `'tick'` -- the one schedule this template ships |
| `hop.schedule_id` | the key the runtime ops address, a literal UUID v7 |
| `hop.scheduled_at` / `hop.fired_at` | planned and actual firing time, RFC-3339-Z in UTC |
| `hop.iteration_n` | `0`, `1`, `2`, … for a repeating schedule |

The `condition` is worth drawing even with one schedule: an `add` op can put a second one
into the same cell later, and an unconditional edge would carry both.

## Knobs

**Since 1.0.1 both knobs are literals in `params.schedules`, not environment tokens**
([#138](https://github.com/mmeyerlein/meclaw/issues/138), ruling R-0904-6). `CLOCK_CRON`
and `CLOCK_SCHEDULE_ID` are gone, and nothing reads them any more. That is not tidiness:
an environment knob is colony-wide, so under the old form every clock in a colony ticked
on the same expression and carried the same schedule key -- in a template whose entire
purpose is to be instantiated several times under different names.

| field of `params.schedules[0]` | default | effect |
|---|---|---|
| `cron` | `0 */5 * * * *` | 6-field Quartz cron (`Second Minute Hour DayOfMonth Month DayOfWeek`), evaluated in **UTC** |
| `schedule_id` | `0190a3f2-0000-7000-8000-000000000484` | the schedule key the runtime ops address. A literal UUID v7, never a `${uuid7:...}` token: such a token is resolved at instantiation and has no filesystem-side producer, so a tree written straight to disk would refuse to boot on it. Two clocks may carry the same key -- a timer's key space is its own `cell.db` -- and neither ever meets the other. |

`schedules` is ONE params key holding the whole list, so an override replaces the list,
last-write-wins. On a single-cell template it is a **flat** params object -- there is no
inner cell to address, and the path-keyed form (`{"": …}`) is refused with `schema`:

```json
{"name": "sweeper", "template": "clock@1.0.1",
 "override_params": {"schedules": [{"schedule_id": "0190a3f2-0000-7000-8000-000000000484",
                                    "schedule_name": "tick", "cron": "0 0 * * * *",
                                    "emit_to": ".", "emit_body": {"messages": []}}]}}
```

**The schedule id is a literal and not a `${uuid7}` token.** That token is an
instantiation substitution with no filesystem-side producer, so a tree written straight
to disk -- which is how a hand-built colony is written and how this template is read all
over the test corpus -- refuses to boot on it (`unsupported_substitution`). A timer's key
space is its own `cell.db` and this template ships exactly one schedule, so the token
would buy nothing a constant does not, and the constant costs no boot.

## What does NOT live here

- **The meaning of the tick.** No condition, no filter, no store. A clock that knew what
  it was ticking for would be a second scheduler.
- **Catch-up.** A missed firing is dropped rather than replayed -- the next tick asks the
  same question, and a timer has no relevance classification that could decide otherwise
  ([`docs/cell-types.md`](../../docs/cell-types.md) § `timer`).
- **Local time.** Cron is evaluated in UTC. Whoever wants an office hour computes the
  offset into the expression and carries the daylight-saving shift themselves.
- **A second clock per lane.** One cell can hold several schedules; the edge tells them
  apart by `hop.schedule_name`.

Pinned by
[`crates/meclaw-cells/tests/gh484_a_manifest_can_give_a_colony_a_clock.rs`](../../crates/meclaw-cells/tests/gh484_a_manifest_can_give_a_colony_a_clock.rs),
which boots a colony without a clock, instantiates this template by mutation, and waits
for the tick to land on the lane the parent's edge stamped.
