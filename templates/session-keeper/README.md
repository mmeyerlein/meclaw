# `session-keeper@2.2.0`

A session lifecycle as a hive of existing cell types -- no new cell type, no Rust. Five cells:
`stamp` (a `code` cell in the ingress path), `close` (a `code` cell for the night),
`sessions` (a `store` cell, the whole state), `night` (a `timer` cell) and, since 2.1.0,
`porter` (a `code` cell that opens and closes nothing -- it carries the ledger to another
keeper and back, § *Carrying the sessions to another keeper*).

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
| `sessions` | `store` | one row per generation: `channel, session_id, opened_at, last_seen, closed, closed_at, audience_set`. Since 2.1.0 one more table stands beside it and is not domain at all: `port_scratch`, the transfer lane's notepad — written and read by `porter` alone, and it never travels. |
| `night` | `timer` | the firing, every thirty minutes through the local night |
| `porter` | `code` | the transfer lane: it walks `sessions` for an `in_export` and writes one part back on `in_import`. It opens no generation and closes none — stateless like the other two, which is why its notepad is a table in `sessions`. |

## Ports

**This hive is sealed.** `config.json` declares `params.ports: []` (GH #228), which is the
SEALED state: the hive path is the only address, and a mutation naming a cell inside it --
`./stamp`, `./close`, `./sessions`, `./night`, `./porter`, any of them -- is refused with
`hive_port_boundary`. What a caller wants rides on `hop.route`, and the lanes it may use
are the four `params.contract` declares.

The entry lane therefore addresses the **hive path** and names itself on the hop. The
parent edge names the lane with `set_hop: {"route": "'in_turn'"}` and promotes the channel
identity to `context.channel`
(a Telegram/Slack `hop.chat_id`, a room, a phone number -- whatever a surface calls "the
same conversation partner"). Without it every turn of the colony lands on the channel
`default`, which is the right answer for a single-surface colony and the wrong one for
a bot with many chats.

**It is the chat and not the node the connector stands at** -- those were one word
until [#522](https://github.com/mmeyerlein/meclaw/issues/522), and the word had to be
the node, because the answer was routed back by it. A generation was then opened per
CONNECTOR: one idle clock, one nightly close and one session id for every chat of a
bot, and the channel-local clause of an audience gate could never match a row written
under a chat id. The address moved to `context.channel_node`, which nothing in this
hive reads, and this key now means what this section always said it meant
(`templates/member/README.md` § *The two channel keys*).

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
| `in_export` | whoever moves this keeper | demands the whole session ledger as a versioned document. Empty `context` -- an export is about the hive, not about a round. |
| `in_import` | the same, on the receiving side | feeds ONE part of such a document into a keeper that is already running. Empty `context`. |

Exits leave **from the hive path** on `hop.route`. The "written by" column says which cell
inside produces the emission -- it is a fact about this hive, never an endpoint: an edge
that names it is refused (see above).

| route | written by | to | notes |
|---|---|---|---|
| `turn` | `stamp` | the context assembly | the inbound turn, unchanged. **Promote `hop.session_id` to context on this edge** -- that promotion IS the stamp. |
| `close` | `close` | the consumer of a finished session | one request per generation; promote `hop.session_id`, `hop.channel` and `hop.audience_set` -- all three, and the third is the one a caller wiring from `template.json` used to miss (see below). |
| `export_done` | `porter` | whoever asked for the export | this keeper's own store wrote the whole ledger into `<fence>/<dir>/seed/` and says where: `hop.seed_dir` (relative to `params.transfer.base_path`), `hop.export_hive`, `hop.export_of`, `hop.rows_written` ([#555](https://github.com/mmeyerlein/meclaw/issues/555)). |
| `dump` | `porter` | whoever fed the import | the receipt of one applied import part (`hop.rows_written`, `hop.export_part` of `hop.export_of`, `hop.export_final == "1"` on the last). Since #555 that is all this lane carries. **Drain it with a PLAIN `hop.route == 'dump'` test** -- an edge that also tests a second hop key reads as no drain under the `required_drains` probe and the mutation is refused. |
| `reject` | `stamp`, `close`, `porter` | wherever a broken keeper is read | the session store did not answer a step of this keeper ([#343](https://github.com/mmeyerlein/meclaw/issues/343)). `hop.reject_reason` is `store_refused`, `hop.store_error` carries the store's own `error_code` (a free string -- the store's code list is open) and `hop.store_operation` the refused op. **Drain it.** Every one of these failures reads as a correct run: an unanswered lookup looks like a channel with no open session -- and used to **open a second generation** for one that had one -- and an unanswered nightly sweep looks like a night with no idle channel. The body names the **step**, because they do not leave the same thing standing: at `touch` and `open` the stamped turn has already left in the same emission, and the `open` case is the sharp one -- the turn travels with a session id whose row was never written, so the next turn opens yet another generation. Since 2.1.0 a **third producer** takes the same lane: `porter` refuses a transfer it will not carry out and names the case in the same `hop.reject_reason` (`export_write_failed`, `import_format`, `import_unknown_table`, `import_schema_drift`, `missing_audience`, `import_probe_failed`, `import_write_failed`). One drain takes all of them; the reason code tells them apart. |

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

**Since `2.2.0` the three knobs of this hive are params, not environment
variables** ([#138](https://github.com/mmeyerlein/meclaw/issues/138); the
`collector@1.2.0` migration
([#136](https://github.com/mmeyerlein/meclaw/issues/136)) is the reference
pattern). Each one lives in the `params` block of the cell that reads it, is
declared in that cell's `contract.settings`, and is the same value in both places
plus as the fallback literal inside the script -- a test pins the three against
each other. Defaults are bit-identical to the environment form they replace.

**What this buys.** A substitution token resolved out of `.env` was
colony-GLOBAL: two keepers in one colony could not end their sessions on
different rules, and an `override_params` entry could not name the knob at all,
because only a key a cell carries under `params` may be named
([#294](https://github.com/mmeyerlein/meclaw/issues/294)). Now it can.

| cell | param | default | meaning |
|---|---|---|---|
| `./close` | `idle_ms` | `7200000` | how long a channel has to be silent before a firing ends its generation. Two hours. |
| `./close` | `close_limit` | `50` | how many generations one firing may seal. A store `select` has no implicit limit. |
| `./night` | `schedules[0].cron` | `0 0,30 22-23,0-3 * * *` | 6-field Quartz cron of the sweep, **in UTC** (see below). |

A `timer` cell has no top-level `cron` param and never will: `TimerParams::parse`
reads `schedules` and `query_timeout_ms`, and would ignore any other key in
silence. So the nightly cron is a literal *inside* the schedule, and moving it
means naming `schedules` -- the key that does exist:

```json
"override_params": {
  "session-keeper/close": {"idle_ms": 600000, "close_limit": 20},
  "session-keeper/night": {"schedules": [{
    "schedule_id": "<a real uuid>",
    "schedule_name": "night-close",
    "cron": "0 0,30 23,0-4 * * *",
    "emit_to": "../close",
    "emit_body": {"messages": [{"origin": "user", "type": "text", "text": "night-close"}]},
    "emit_headers": {}
  }]}
}
```

The whole `schedules` array is replaced, not merged, so every field has to be
there -- including `schedule_id` as a real UUID, because the template's
`uuid7` token is minted at instantiation and an override takes that moment over.

**A standing instance is untouched.** Instantiation is a COPY: a colony grown
from an earlier version keeps its own `templates/` copy with the old tokens in it
and goes on reading its `.env`. What stops working is the reverse -- an old
environment line in a colony grown from `2.2.0` is read by nothing at all, and
says so nowhere. Move such a line into `override_params` when you regrow.

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
session, an hour of drift costs nothing -- but if you want the boundary exact,
override `./night`'s `schedules` per season, or point it at a window wide enough
for both
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

## Carrying the sessions to another keeper (#471)

A session row decides whether a turn continues the conversation that is open or starts a
new one. A keeper reborn empty therefore greets somebody it has been talking to for a
year as a stranger — and nothing anywhere reports it, because opening a generation is a
perfectly ordinary event. That is why the ledger has to travel. Two lanes since 2.1.0:
`in_export` writes it out as a versioned document, `in_import` takes one part back into a
keeper that is already **running**.

**The document.** One content table means one part, and that part is also the last one —
which is what lets the completeness marker exist at all:

```json
{"format": "meclaw-session-export/1", "hive_template": "session-keeper",
 "export_id": "…", "exported_at": "…",
 "table": "sessions", "part": 1, "of": 1, "final": true, "absent": false,
 "key": ["channel", "session_id"],
 "schema": {"channel": "text", "…": "…", "audience_set": "text"},
 "rows": [ {"channel": "tg:42", "session_id": "tg:42-…", "…": "…"} ]}
```

`schema` is the store's own declaration, so `{"schema": …}` as line 1 plus one row per
line after it **is** a `sessions/seed/sessions.jsonl` — birth path and transfer path speak
one format. Since [#555](https://github.com/mmeyerlein/meclaw/issues/555) the EXPORT half
does not travel as messages at all: this keeper's own store writes that file itself,
through the substrate's `transfer` slot and inside the fence it declares in
`params.transfer.base_path`, with `seed/export_final.json` beside it as the completeness
marker — a directory without it is a prefix, and nothing else in it would say so. The
object above is therefore what an IMPORT part looks like, which is that file read back the
other way round.

**Redirect that fence before the first export.** The shipped default is
`/tmp/meclaw-member-export`, which is world-readable on most hosts, and what lands under it is
the whole SESSION LEDGER — every turn of every session this keeper holds. Nothing about the
fence is a secret and nothing about it is checked: the store writes where `params` say, so name
a directory of your own with `override_params` on `"talky/session-keeper/sessions"` (or the path
the keeper stands at) before anything is exported.

**Wiring.** `params.required_drains` pairs each ingress lane with its exit
(`in_export → export_done`, `in_export → reject`, `in_import → dump`,
`in_import → reject`), and the mutation is refused unless all of them are drawn **in the
same mutation** as the ingress edge. An export nobody drains writes the whole ledger and
tells nobody where, and an undrained refusal makes a transfer that did not happen look
exactly like one that did. Keeper to keeper, the move goes through a DIRECTORY since #555
and no edge carries the document at all:

```json
[
  { "from": "./keeper-old", "to": "./transfer-drain",
    "condition": "has(hop.route) && (hop.route == 'export_done' || hop.route == 'dump' || hop.route == 'reject')" },
  { "from": "./keeper-new", "to": "./transfer-drain",
    "condition": "has(hop.route) && (hop.route == 'export_done' || hop.route == 'dump' || hop.route == 'reject')" }
]
```

`in_export` at the old keeper writes `<fence>/<dir>/seed/sessions.jsonl`; the file read
back the other way round — header line = schema, rest = rows — is the `in_import` part the
new one takes, which is exactly what
[`../../examples/memory-import/build_import.py`](../../examples/memory-import/build_import.py)
writes with `--after-boot`. Every drain stays a plain route test: that is what the
plainness rule above is about.
`crates/meclaw-cells/tests/gh471_a_keeper_carries_its_sessions.rs` drives exactly this
shape and then asks the third question a row count cannot: a turn on the transferred
channel is stamped with the session the **source** keeper had open. A ledger that arrives
and is not read is a table, not a session.

**Applying the same part twice leaves the same state.** `params.schema` cannot express a
key, so a repeated `insert` would duplicate the row. The importer probes first: it parks
the part and the `(channel, session_id)` pairs the target already holds under one
`port_scratch` key, reads both back in a single `select`, and inserts only what is
missing. The target wins every collision — an import never updates and never overwrites,
so a generation this keeper already sealed is not reopened by a document from elsewhere.
That is what makes "send it again" the repair for any failure.

**A part that lost its round is refused whole.** A transfer is exactly where a
disclosure-governing column falls off quietly, so a `sessions` part whose declared
`schema` does not carry `audience_set` is refused with nothing written and
`hop.reject_reason = "missing_audience"`: a row whose round did not survive is a row that
may be told to anyone, and nobody downstream can reconstruct one honestly. A round that
is present but **empty** travels as it stands — empty means invisible, which is the
honest fate of a generation opened before its door declared one, and inventing one would
*be* the laundering. Nothing is recomputed on the way in. The firewall's porter has no
such gate and needs none: a rule names no audience.

**Through a member, the lane is the same lane four levels up (#475).** Nothing above this
hive used to forward one, so the sessions were the one table a rebuilt member came back
without. [`talky`](../talky/README.md) and [`assistant`](../assistant/README.md) pass
`in_export` and `in_import` straight through — no modifier, because the lane is named the
same on both sides of every one of those boundaries — and carry `export_done` and `dump`
back out, past [`member`](../member/README.md) to whoever asked. Since #555 this keeper's
own store writes the ledger beside the three holders' documents
(`<fence>/<dir>/session-keeper/seed/`) rather than handing parts up to a cell of the member
that files them. Two things are asked of the caller and neither is this hive's:

```jsonc
// the export, at the member's own path
{"target": "<member>", "header": {"hop": {"route": "in_export"},
                                  "context": {"assistant": "<generation>"}}}
// one part back, at the same path
{"target": "<member>", "header": {"hop": {"route": "in_import",
                                          "import_hive": "session-keeper"},
                                  "context": {"assistant": "<generation>"}}}
```

`context.assistant` is the key a turn is addressed with, and it addresses a transfer for
the same reason: it is the member's container that knows which generation is which. A part
that names `session-keeper` and no generation has no address at all, and the container says
so as `no_route` rather than delivering it to a holder that would refuse it under some
other name. The refusal path is the one difference from the hive-to-hive shape above: a
porter refusal is normalised inside the talky and leaves the generation on `error`, beside
every other failure of that unit, rather than on `reject`.

**Birth is the other half, and it is a file.** A seed is read **once**, when the `cell.db`
is created, and inert for ever after. To give a keeper a past before it exists, write the
part as `sessions/seed/sessions.jsonl` and instantiate. `in_import` is the only way into
one that is already running — the two are complements, not a choice.

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
- **An export reaches this keeper only when the caller names the generation.** The lane
  crosses `member -> assistants -> assistant -> surface -> session-keeper` since
  [#475](https://github.com/mmeyerlein/meclaw/issues/475), but the member's fan-out edge
  is guarded on `context.assistant`: a member with two generations holds two ledgers, and
  they are not one document — nor could a directory hold both, since a document is filed
  under the hive it came out of and both would claim the same name. So an export that
  names no generation is the export the member always did, three holders and no sessions.
  That is a property of the address, not a gap in this hive.
- **A session is keyed on a channel, and a channel belongs to the member.** Since
  [#454](https://github.com/mmeyerlein/meclaw/issues/454) the channel names are the
  member's, not this hive's. A transfer into a colony whose channels are named differently
  therefore carries history whose referent is gone: the rows arrive, they are correct, and
  no turn will ever match them. Move a keeper with the channel names it was written
  against, or expect a ledger that reads as a full one and behaves as an empty one.
- **A part is a whole table.** There is no paging: a truncated part would lie about being
  a table. A ledger that outgrows one message has no shipped answer yet.

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
- `crates/meclaw-cells/tests/gh471_a_keeper_carries_its_sessions.rs` -- two shipped
  keepers in one colony, wired with the single carrying edge: the walk leaves as ONE final
  part, the same document applied twice leaves the same state, and a turn on the
  transferred channel is stamped with the session the source keeper had open.
- `crates/meclaw-cells/tests/gh475_a_member_reaches_the_keeper_it_holds.rs` -- the four
  levels in between: the shipped `talky`, `assistant` and `member` files carry the two
  lanes and the `dump` back out, the member names the generation, and a colony run drives
  one part from the member's own door into the keeper's store and back out again.
- `crates/meclaw-cells/tests/gh471_the_porters_mirror_their_stores.rs` -- the porter's
  schema mirror against `sessions/config.json`, column for column, plus the tables that
  stay behind by name and the document format that is this hive's alone. A mirror is the
  thing that rots silently: a column added to the store and not to the walk simply stops
  travelling, and the loss surfaces one colony later as an empty field nobody can trace.
