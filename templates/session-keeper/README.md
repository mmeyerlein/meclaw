# `session-keeper@2.0.2`

A session lifecycle as a hive of existing cell types -- no new cell type, no Rust. Four cells:
`stamp` (a `code` cell in the ingress path), `close` (a `code` cell for the night),
`sessions` (a `store` cell, the whole state) and `night` (a `timer` cell).

**A session is a channel generation, and the model is a phone call.** It begins on a
channel with a turn, it ends on that channel, and the fluid transition in between belongs
to it. Everything downstream -- the context window, the episode a memory hive is handed,
the answer to "what did we talk about this morning" -- hangs off ONE id, and this hive is
the only place that mints it.

## What it delivers

- **One identity per call.** Every inbound turn of a channel leaves the stamp with the
  `session_id` of the generation that channel is currently in. Turn 17 carries the same id
  as turn 1, without anyone downstream having to remember anything.
- **An end that is arithmetic.** A nightly timer plus an idle threshold: no channel is
  ended while it is talking, and no model is asked whether a conversation is over. Both
  halves are configuration -- the night is a cron, the silence is a number.
- **A lazy beginning.** Nothing pre-creates a session. Not a boot, not a close, not the
  timer. The next turn after the end of a call IS the beginning of the next one, and it
  mints its own id.
- **Exactly one close request per generation.** The seal is a guarded update; the pass that
  affected the row owns the close. A repeated firing, a missed night and a second sweep
  are all silent.

## Cells

| cell | type | what it holds |
|---|---|---|
| `stamp` | `code` | the ingress pass: look up the generation, restart the idle clock, stamp the turn |
| `close` | `code` | the night pass: which channels fell silent, seal them, ask for the close |
| `sessions` | `store` | one row per generation: `channel, session_id, opened_at, last_seen, closed, closed_at, audience_set` |
| `night` | `timer` | the firing, every thirty minutes through the local night |

## Ports

**This hive is sealed.** `config.json` declares `params.ports: []` (GH #228), which is the
SEALED state: the hive path is the only address, and a mutation naming a cell inside it --
`./stamp`, `./close`, `./sessions`, `./night`, any of them -- is refused with
`hive_port_boundary`. What a caller wants rides on `hop.route`, and the lanes it may use
are the two `params.contract` declares.

The entry lane therefore addresses the **hive path** and names itself on the hop. The
parent edge names the lane with `set_hop: {"route": "'in_turn'"}` and promotes the channel
identity to `context.channel`
(a Telegram/Slack `hop.chat_id`, a room, a phone number -- whatever a surface calls "the
same conversation partner"). Without it every turn of the colony lands on the channel
`default`, which is the right answer for a single-surface colony and the wrong one for
a bot with many chats.

The same edge is also where `context.audience_set` belongs -- the round the conversation
is spoken in, as a JSON list in affinity vocabulary (`["member:alex","agent:scribe"]`).
It is a **constant of the generation**: a change of the participant set ends the
generation and a new one takes over (ADR-0002 E8), so the turn that OPENS a generation is
the only place it can be recorded, and the keeper records it on the row right there. A
door that declares nothing leaves the column empty; nothing here derives a round from the
`session_id` prefix, and nothing defaults it to `["*"]`.

| lane | who sends it | what it does |
|---|---|---|
| `in_turn` | the inbound surface (proxy, intake) | stamps the turn and restarts the idle clock |
| `in_sweep` | an operator, a second schedule | forces a sweep outside the night (optional) |

Exits leave **from the hive path** on `hop.route`. The "written by" column says which cell
inside produces the emission -- it is a fact about this hive, never an endpoint: an edge
that names it is refused (see above).

| route | written by | to | notes |
|---|---|---|---|
| `turn` | `stamp` | the context assembly | the inbound turn, unchanged. **Promote `hop.session_id` to context on this edge** -- that promotion IS the stamp. |
| `close` | `close` | the consumer of a finished session | one request per generation; promote `hop.session_id`, `hop.channel` and `hop.audience_set` -- all three, and the third is the one a caller wiring from `template.json` used to miss (see below). |

**The close lane, as a port convention (Track K/E):** the keeper emits `hop.route == 'close'`
carrying `hop.session_id`, `hop.channel` and `hop.audience_set`, and a body with no turns
at all (`messages: []`) -- a close request is a question about a session, not a
conversation.

**All three come off the row, and that is the point.** A close is fired by a TIMER, and a
firing carries no context of its own: it knows the moment, not the conversation. So the
room and the round of a swept-closed generation cannot come from the message being
handled -- they are read out of the `sessions` row the opening turn wrote, carried down
that generation's own seal chain, and put on the hop. All three keys are always PRESENT,
empty when unknown: a missing hop key makes a CEL modifier fail, and a failed modifier
skips the edge, so a close that could not name its round would vanish instead of being
refused (GH #273).

**Three keys, and the third was missing from the recipe.** Until `session-keeper@2.0.2` the
`PORTS` slot of `template.json` named only `hop.session_id` and `hop.channel` on this port,
against this section and against `contract.emits[close]`. A caller who wired from that slot
closed generations that `memory-drain` then refused with `missing_audience` -- the drain
does exactly what the last bullet of "Known limits" promises, and the promotion it needs was
never in the recipe ([#311](https://github.com/mmeyerlein/meclaw/issues/311)).

The consumer is the collector's `in_close` lane, so the parent edge renames the route the
way it renames every collector lane:

```json
{"from": "./session-keeper", "to": "./collector",
 "condition": "hop.route == 'close'",
 "modifier": {"set_hop": {"route": "'in_close'"},
              "set_context": {"session_id": "hop.session_id",
                              "channel": "hop.channel",
                              "audience_set": "hop.audience_set"}}}
```

The collector then reads the session out of **its own** store and emits the batch. The
keeper never sees a turn of it.

Wire the ports in the **same mutation** that instantiates the hive: an island without a
crossing edge derives inactive, and its timer never spawns.

## Knobs

**Env knobs are an experimental surface.** Until this template's knobs move onto the `params`
block of the cells that read them, their names carry no compatibility promise and may change in
any `0.x` release; provider credentials keep living in `.env` either way. The migration is
tracked in [#138](https://github.com/mmeyerlein/meclaw/issues/138), with the
`collector@1.2.0` migration ([#136](https://github.com/mmeyerlein/meclaw/issues/136)) as the
reference pattern.

| env var | default | meaning |
|---|---|---|
| `KEEPER_IDLE_MS` | `7200000` | how long a channel has to be silent before a firing ends its generation. Two hours. |
| `KEEPER_NIGHT_CRON` | `0 0,30 22-23,0-3 * * *` | 6-field Quartz cron of the sweep, **in UTC** (see below). |
| `KEEPER_CLOSE_LIMIT` | `50` | how many generations one firing may seal. A store `select` has no implicit limit. |

## The timer computes in UTC. Do the sum.

The `timer` cell resolves cron expressions against UTC -- always, everywhere; there is no
zone parameter and there will not be one. A schedule that is meant to run through the
LOCAL night is therefore written as the UTC **image** of that night, and the image moves
with daylight saving time.

For `Europe/Berlin`, 00:00 until 06:00 local:

| period | local offset | local window | UTC window | cron |
|---|---|---|---|---|
| summer (CEST) | UTC+2 | 00:00 – 05:30 | 22:00 – 03:30 | `0 0,30 22-23,0-3 * * *` (**shipped default**) |
| winter (CET) | UTC+1 | 00:00 – 05:30 | 23:00 – 04:30 | `0 0,30 23,0-4 * * *` |

Read the default field by field: seconds `0`, minutes `0,30` (twice an hour), hours
`22-23,0-3` (22, 23, 0, 1, 2, 3 — six hours across midnight UTC), every day. Twelve
firings, the first at 22:00 UTC = **00:00 CEST**, the last at 03:30 UTC = **05:30 CEST**.
Rule of thumb: `UTC = local − offset`, and an hour that goes below zero wraps into the
previous day (00:00 CEST − 2 h = 22:00 UTC **of the day before**), which is why the hour
list is written as two ranges and not as `22-3`.

Running the summer cron in winter is not a defect, it is an hour of drift: the sweep opens
at 23:00 local instead of midnight. Since the idle threshold is what actually ends a
session, an hour of drift costs nothing -- but if you want the boundary exact, set
`KEEPER_NIGHT_CRON` per season, or point it at a window wide enough for both
(`0 0,30 22-23,0-4 * * *`).

**Missed firings expire.** The timer never catches up (`docs/cell-types.md` § timer), and
that is exactly right here: the next firing thirty minutes later finds the same idle
channel, and a night the colony spent switched off is caught by the first firing of the
next one.

## The protocol, row by row

The ingress pass, once per turn:

```
in_turn  -> select sessions (channel, closed 0, limit 1)   phase look
look     -> a row?  update last_seen                       phase touch
         -> no row? insert a new generation                phase open
         -> AND the turn onward on route 'turn' with hop.session_id
```

The night pass, once per firing:

```
firing   -> select sessions (closed 0, last_seen < now - IDLE)   phase sweep
sweep    -> per candidate: update closed=1 where closed=0        phase seal   <- the guard
seal     -> rows_affected 1: ROUTE close (one per generation)
         -> rows_affected 0: nothing (someone else sealed it)
```

A sweep without a candidate and a lost guard race emit **nothing** (empty multi-send,
terminal by design) -- the same discipline as the collector this sits in front of.

The idle comparison never parses a timestamp on the row side: `last_seen` is a fixed-width
UTC stamp, so it orders lexicographically, and "older than the cutoff" is a store-side
`lt` on a string the close pass computed once.

## What it is not

- **Not a summarizer.** The close request names a session; what the day was worth is
  decided by whoever consumes it. There is no LLM in this hive and no route to one.
- **Not a counselor.** "Is this conversation over?" is a model question and a later
  experiment. v1 answers "has this channel been silent for two hours, and is it night?"
- **Not a context assembler.** The turns, the window, the tool round and the memory bundle
  belong to the collector. The keeper adds an id to the envelope and keeps its hands off
  the body.
- **Not a deleter.** A sealed generation keeps its row. That is what makes a repeated
  firing silent and the history readable.

## Known limits

- **The turn rides through the lookup on the hop.** The stamp needs a store round trip
  before it knows the id, and a store reply carries the row rather than the conversation
  that asked for it -- so the inbound body travels as `hop.keeper_body` (a JSON string,
  promoted to context by the hive edge) and is restored one hop later. A pathological turn
  is therefore copied once into the header of two internal messages. Bodies that must not
  be copied belong behind a blob pointer.
- **A cold channel can mint two generations under a burst.** Two first turns arriving at
  once both find no open row and both insert one. The store has no unique constraint to
  lean on (constraints are deferred), so the next turn converges on the newest row
  (`order by opened_at desc, limit 1`) and the older one is closed by the next night. A
  duplicate generation costs an extra close request, never a lost turn.
- **The night is one cron for the whole colony.** Per-channel time zones are not modelled;
  a keeper serves one local night.
- **Instance-per-day is not this.** v1 runs the logical generation (same cells, new id).
  A talky that IS one day (R-OS-3's target picture) is the experiment after the wave.
- **A generation opened before its door declared a round has none, forever.** The column
  is written once, at the open, and never rewritten -- provenance is not a roster
  (ADR-0002 E12). Wiring `context.audience_set` onto the ingress edge therefore takes
  effect on the NEXT generation of a channel, not on the one that is currently running.
  What is already open closes with an empty round, and a consumer that needs one refuses
  that batch visibly rather than writing a row that claims everyone was present.

## Pins

- `crates/meclaw-cells/tests/session_keeper.rs` -- the shipped `script_inline` against real
  stdin documents (the lookup, the lazy reopen, the idle cut, the guard) plus the cron
  arithmetic of the shipped default, computed rather than asserted.
- the same file, colony level -- a running colony with a real store and a real timer: three
  turns with one id, a firing that closes nothing, a firing that closes exactly once, a
  repeated firing that stays silent, and the next turn that opens the next generation.
- the same file, the round of a generation (GH #273) -- the row records the set the door
  declared, a running generation never has it rewritten, every seal carries its own, and a
  generation without one says so instead of inventing one.
- `crates/meclaw-cells/tests/gh273_a_swept_close_reaches_the_memory.rs` -- the whole way:
  a conversation, a SWEEP that ends it, the shipped drain, and the episode row that lands
  with the room and the round the conversation was spoken in.
