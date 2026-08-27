# `affinity@3.0.0`

The curated record of the people and agents a colony knows -- as one hive of existing
cell types. No new cell type, no Rust, and no model: every judgement in here is a
comparison, so a brief costs nothing and a push tick over unchanged data costs two
selects.

Five cells:

| path | type | role |
|---|---|---|
| `store` | `store` | the domain: entities, relations, trust, disclosure, subscribers, proposals, audit |
| `brief` | `code` | the only reader of the domain -- audience filter and pack rendering. Appends its own `audit` row per brief |
| `gate` | `code` | the only writer of the domain -- AIeOS validation, minting, audit |
| `push` | `code` | push-on-change: hashes what a subscriber would get, and stays silent when it did not move. Writes that hash and `sent_at` back onto the subscriber row, plus an `audit` row |
| `clock` | `timer` | the push tick (6-field Quartz cron, **UTC**) |

**This is the first hive with a domain of its own.** Memory produces the raw material
(extracted, unkuratiert, with a confidence); affinity decides. The way from there to
here is the `proposals` table and nothing else, and for a question like *who is X to me*
only affinity is quoted. Memory is allowed to be wrong; affinity has to have decided.

## What it delivers

- **Persons and agents in ONE vocabulary.** An entity row carries a whole
  [AIeOS](https://aieos.org) 1.1.0 document in its `aieos` column -- unchanged, exactly as
  the spec describes it. A member, a channel voice and an agent core are the same shape,
  which is what lets `brief` answer *who am I* and *who am I talking to* out of one table.
- **Relations as a table, not as prose.** AIeOS describes a character, never the
  relationship of two entities: `history.family` has four free-text fields, which is prose
  about a person and not an addressable edge. So relations live in their own table, cut
  **exactly on the `traverse` signature** (`src`/`dst`/`kind`/`weight`). *Who is Sam to
  Alex* is therefore ONE store op over two hops, not a graph walk in a script.
- **Visibility as rows, and fail-closed.** A field reaches an audience because a
  `disclosure` row named it -- never because no rule forbade it. An audience with no row at
  all does not even cause the document to be read: the denial happens one read earlier, and
  the only thing that leaves is an audit line.
- **An audience is a SET, not a name** (GH #154). A fact that surfaced in a conversation is
  implicitly released to exactly the people who were there, and never beyond. So a row
  carries `audience_set`, and `brief` uses it iff the CURRENT round is a **subset** of it:

  | origin | later round | verdict |
  |---|---|---|
  | `{e,a,b}` | `{e,a,b,c}` | not a subset -- **must not be used** |
  | `{e,a,b}` | `{e,a}` | subset -- may be used |
  | `{e,a}` (a told the agent about b) | `{e,b}` | not a subset -- a confidence stays one |

  Two things fall out of the rule rather than out of extra code: hearsay never surfaces in
  front of its subject, and a four-person round can only use what all four were present
  for. Strict and fail-closed, including for a participant's own 1:1 self-disclosures --
  the obvious softening (`subject == speaker == present`) is a decision that has not been
  taken, not an oversight. A row addressed to `*` stays universal; a row from before the
  rule is read as its addressee alone, the narrowest reading that cannot widen anything by
  accident.
- **And the round is DECLARED, or there is no answer** (GH #306). The round is the other
  half of the rule, and it is edge truth exactly like the asker: the caller carries it as
  `hop.audience_set` (a JSON array of participant ids) and the door edge promotes it to
  `context.audience_set`, or the caller's own edge promotes that key and the door leaves it
  standing. Inside the lane it then rides in the carry through every store round trip,
  because a `code` cell has no `cell.db`.

  **A round nobody declared is refused** -- `no_round`, its own reason code in the `audit`
  table -- and the previous fallback of reading it as "the asker alone" is gone. That
  fallback called itself the safe reading and was the opposite: for a SUBSET test, `{asker}`
  is the WIDEST set that can be handed in, because every release naming the asker passes it
  whoever else is in the room. Until this issue no modifier in the whole library wrote the
  round under the name this hive read, so `{asker}` was not merely the default, it was the
  only value that ever reached the gate -- the refusal half of the rule was dead code in
  every colony that ever booted, and a round of `{a,b,c,d}` read a row released to
  `{a,b,c}` with the fourth participant unreportable. A 1:1 lane says so out loud (`["a"]`,
  or a declared empty `[]`) rather than being assumed.

  **The round has ONE name, and it is `audience_set`**
  ([#330](https://github.com/mmeyerlein/meclaw/issues/330)). It was not always so: this hive
  read the round under an internal name of its own while every round producer that ships
  writes `context.audience_set` -- `session-keeper`, `memory-drain`, and the receptionist's
  ingress edge per ADR-0002 E8 -- and *nothing bridged the two*. That divergence is why the
  gate could stand open unnoticed: nobody wired the key because the rest of the colony spells
  it differently. The colony's spelling won and the hive's was withdrawn. `context.participants`
  was this hive's internal name for the round until GH #330; it is retired, not aliased -- a
  request that declares its round only under the old name is refused `no_round`.
  **No template may ever introduce a second name for it**: a second spelling is not a
  convenience, it is a second gate that can stand open while the first one reads shut.
- **Identity in the subscriber's own prompt.** The pack leaves as `system.identity`,
  `system.peer`, `system.relationship` and `system.channel`, and an `llm` cell accumulates
  those four per path in its own `cell.db`: an attune at birth and every later change
  update only the slot that moved.
- **And every slot carries its own rendering** (GH #258). The right *place* is not yet the
  right *shape*. An `llm` cell flattens what arrives in `system.*` into leaves, stopping at
  the first object that carries a `text` key, and concatenates exactly those `text` values
  into the prompt. The pack had none anywhere, so it produced no leaf at all -- nothing
  persisted, nothing rendered, and the subscriber's model answered from whatever else it
  had while the audit table said `ok`. Each slot therefore ships its structure and, at the
  top of the same object, a `text` rendering of it: `path: value` lines under a heading
  that names the subject, because a leaf reaches the prompt on its own and has to say what
  it is about. The `text` sits at the top so the leaf walk stops there and the four slots
  stay exactly four -- which is also what keeps a subscriber that pins `system_writable` to
  them accepting the write.
- **And a push costs a write, not a call** (GH #263). An `llm` cell stays silent for a
  body with **no** `messages[]` at all -- it persists the slots and returns. So the push
  lane carries `system` and nothing else, which is the shape that silence rule is written
  for. It used to carry the tool lane's `tool_result` as well, because both lanes left
  through one `answer()`: every changed subject then bought the subscriber an inference,
  on a tool result answering a call it never opened -- an orphan a real provider answers
  with a 400 rather than with silence. A push with nothing to disclose now says nothing
  at all, because an empty body is not silence to an `llm` cell, it is a parse error.
- **And the same pack on the tool lane** (GH #242). A sealed agent hive delivers a tool
  answer as `messages[]` alone -- the `system` slot does not survive the boundary -- so an
  answer that travelled only there reached the asking model as a receipt line with nothing
  behind it. The `tool_result` therefore carries the receipt line and, below it, the same
  pack as JSON -- the very object the push lane hands its subscriber, renderings included.
  One disclosure decision, one document, two lanes: which one a caller reads is a fact
  about its wiring, not about what it is allowed to see.
- **The answer names the call it answers** (GH #241). A `code` cell has no `cell.db`, so
  the id of the tool call rides in the lane's carry through every store round trip. Before
  that it was read from a context key nothing wrote, and every brief served after a store
  read left with an empty id -- which closes no fan-in, so the asking turn ended in its
  idle window while the audit table said `ok`.
- **A correction never deletes.** A new document supersedes the active row by status; a
  withdrawn relation gets `status: retracted`; a narrowed release is a newer `disclosure`
  row that `brief` prefers. No branch of `gate` emits `delete` -- see the honest note below
  about what that promise is worth.

## Cells and lanes

```
affinity/                     hive  -- scope marker, eight internal edges
  store/                      store -- the domain (7 tables + 2 store-owned alias tables)
    seed/{entities,relations,trust,disclosure}.jsonl
  brief/                      code  -- read port: trust -> disclosure -> traverse -> entity
  gate/                       code  -- write port: validate -> store ops -> audit -> ack
  push/                       code  -- change detector, renders nothing itself
  clock/                      timer -- the tick
  aieos.schema.json           the VENDORED AIeOS 1.1.0 skeleton (see below)
```

A `code` cell has no `cell.db`, so a lane that needs several reads keeps its state on the
wire: `brief` and `push` emit their phase and their carry on the **hop**, the internal edge
promotes both to **context**, and the store's answer brings them back. The store round trip
*is* the cell's memory. That is why this hive has eight edges and not four.

### Ports

| port | direction | lane |
|---|---|---|
| `in_brief` | in -> the **hive path** | the request as a `tool_call` turn (`{subject, channel, slots}`), plus TWO facts a body may never carry: `hop.audience` (who asks) and `hop.audience_set` (the round, a JSON array of participant ids). The door edge promotes both to `context.asker` and `context.audience_set` -- a caller that promotes those keys on its own edge is served the same way, and **wins** where both exist (see § Identity comes from the edge). `participants` is **retired, not aliased** ([#330](https://github.com/mmeyerlein/meclaw/issues/330); see the retraction note under the audience-SET rule) -- a request that spells the round that way declared no round at all. **Both facts are required** -- no asker is `no_audience`, no round is `no_round`, and either is a denial with an `audit` row and no `system` slot at all |
| `out_brief` | `./brief` -> the asking `llm` cell, or an agent hive's tool lane | `hop.route == 'answer' && hop.subscriber == ''`: the four `system.*` slots **and** the same pack as JSON in the `tool_result`, under the id of the call being answered |
| `in_propose` | in -> the **hive path** | the proposal as a `tool_call` turn (`{op, ...}`); the edge **MUST** promote the writer to `context.actor` and, for `subscribe`, the subscribing cell's address to `context.subscriber` |
| `out_ack` | `./gate` -> the proposer | `hop.route == 'ack'`, `accepted` or `rejected` plus a `reason_code` |
| `out_push` | `./brief` -> each subscribed `llm` cell | `hop.route == 'answer' && hop.subscriber == '<cell path>'`: the four `system.*` slots and **no** turn beside them, so the update costs a write and not an inference (GH #263; the `llm` cell returns without calling when a body carries no `messages[]`) |
| `out_error` | `./gate`, `./brief`, `./push` -> a drain | `hop.route == 'error'` -- the parent MUST wire it |

Both in-lanes are asserted at the hive path and not at a cell behind it: `params.ports`
is empty, so a wiring that names `./brief` or `./gate` is refused with
`hive_port_boundary`. Which cell serves a lane is the template's business, which is what
makes it replaceable.

`out_push` leaves `./brief` and not `./push` on purpose: `push` decides **whether** a
subscriber hears anything, `brief` decides **what** it hears. One renderer means the
audience filter can only be wrong in one place. Both lanes leave on `answer`, so
`hop.subscriber` is what tells them apart -- an `out_brief` edge written without that
condition also collects every push, and the asking cell then gets slot writes meant for
somebody else.

### Wiring `out_push` for a subscribing brain

A subscription has two halves and the `subscribe` op is only one of them. The op says
**what** -- which subject, which channel, which slots -- and the edge that carried it says
**who** (`context.subscriber`, see § Identity comes from the edge). The **graph** says where
the answer runs, and the parent draws it at instantiation. Two edges, no new construct:

```json
{"from": "<affinity>", "to": "<the subscribing cell>",
 "condition": "has(hop.route) && hop.route == 'answer' && hop.subscriber == '<the subscribing cell>'"},
{"from": "<affinity>", "to": "<the asking cell>",
 "condition": "has(hop.route) && hop.route == 'answer' && hop.subscriber == ''"}
```

**One edge per subscribing cell.** `hop.subscriber` carries the address `./push` read out
of its own subscription row, so the condition compares it against the very path the edge
ends at. Two subscribers are two edges. There is no fan-out form and there is nothing to
add inside the hive -- the subscriber list lives in the store, the delivery list lives in
the graph, and the push only travels where both agree.

**The second edge is not optional.** An edge conditioned on `hop.route == 'answer'` alone
matches *every* answer this hive speaks, so a brief edge written without
`hop.subscriber == ''` also collects every push: the asking cell gets slot writes meant for
somebody else, under no call id it ever opened, and the subscriber's own lane is missing
nothing it can notice. That comparison is safe because `./brief` sets `subscriber` on
**every** emission of the answer route -- the tool lane sets it to the empty string rather
than leaving the key off, so the emptiness is a value and not an absence. Both facts are
asserted together in `the_two_answer_lanes_are_told_apart_by_the_subscriber_key`
(`crates/meclaw-cells/tests/affinity_template.rs`), which provokes both lanes in one colony
and drains each sink to the end to prove neither saw the other's message.

**A `subscribe` cannot draw its own edge.** Writing an edge is a mutation, and mutation
authority belongs to the colony alone -- a cell has none, a hive has none, and a `subscribe`
op that could wire its own delivery would be a cell granting itself a route. So the
composite wires the lane at instantiation, before anybody subscribes, and the op that
arrives later only fills a row the graph already has an address for. A subscription with no
edge behind it is accepted, written and silently undeliverable; that is the parent's bug to
avoid and not something the hive can refuse for it
([#289](https://github.com/mmeyerlein/meclaw/issues/289)).

### The persona is a projection, not a copy

The seed under `seed/entities.jsonl` is the **birth state** and nothing more. After birth
the agent's own AIeOS document lives in this hive as one entity, and `out_push` delivers it
into the subscribing brain's `system.identity` slot whenever it changes. Changing the
person is one write here; every brain that subscribed hears it on the next tick, and no
file anywhere is edited. The brain holds a projection of a record, not a copy of a text.

The counter-example is the persona **cell**: a `code` cell that carries the persona in its
`script_inline` as a literal and re-injects `system.identity.soul.text` on **every turn**,
so the sentence exists once per cell that needs it and changing it means editing each of
them -- the N-transcriptions shape. That is the right call for a two-cell demo with no hive
behind it, and the wrong one the moment two brains have to agree about the same person.
The shipped library no longer carries an example of it: the demo bots that did have given
way to the seed route (`templates/talky/brain/seed/system.jsonl`), which writes the
identity once at birth instead of once per turn.

**This template only offers the lane.** Which brains subscribe to what by default, and
which cell in an assistant tree owns the subscription, is a composite decision and lands
with [#302](https://github.com/mmeyerlein/meclaw/issues/302). Until then a parent wires the
two edges above by hand.

### Identity comes from the edge, never from the body

`brief` takes its audience from `context.asker` and its round from
`context.audience_set`, `gate` takes its actor from `context.actor` -- and `gate`'s
`subscribe` takes the subscriber's **address** from `context.subscriber` and the
disclosure **audience** from `context.actor`, keeping only the subject from the body
([#288](https://github.com/mmeyerlein/meclaw/issues/288)). A standing disclosure whose
target the body chose would keep delivering to an address the topology may not even have
an edge to. None of them reads a
name out of the body. A cell knows no sender, and the only trustworthy origin in this
substrate is a `set_context` value on an **edge** -- edges are written by the colony, bodies
are written by whatever produced the message, up to and including a model. An audit row
whose actor came from the body would not be worth the write.

The door edge (`. -> ./brief`) is where the read lane's two keys are promoted, and the
precedence is **edge-pinned first**:

```
asker:  context.asker        →  hop.audience      →  ''
round:  context.audience_set →  hop.audience_set  →  ''
```

A non-empty context value **wins**, and the hop only fills the gap. That order is the rule
of this section applied to itself: an edge is written by the colony, a hop key by whatever
cell the message passed through -- so a cell downstream of the pinning edge must not be able
to shrink its own room by stamping `hop.audience_set`.

`context.participants` was this hive's internal name for the round until
[#330](https://github.com/mmeyerlein/meclaw/issues/330); it is retired, not aliased -- a
request that declares its round only under the old name is refused `no_round`.

The `has(...)` guards are not decoration either: a `set_context` expression that cannot
resolve makes the modifier fail, and a failed modifier **skips the edge**, so a caller who
stamped nothing would have its message vanish instead of being told no.

The door also **resets** `aff_phase`, `aff_carry` and `aff_subscriber`. `context` travels
colony-wide -- every cell emission carries the context it was handed -- so without the reset
a fresh `in_brief` could arrive carrying a phase from an earlier lane and be read as a
mid-lane echo, with a stale carry, or leave on the push lane because an inherited
`aff_subscriber` still named somebody. The internal `./push -> ./brief` edge has always done
this; the door had not.

That internal edge promotes the same identity keys, and `push` declares the round explicitly
(`[audience]`) rather than leaving it to a default -- a push is not a conversation, so its
room is the one audience its subscription was written for.

### This hive is the SOLE source of truth for an identity reference

The round is a set of identity **references** (`member:alex`, `agent:aiden`), and the rule
above only says who may *stamp* one. This is the other half, and it is a contract rule of
the same standing (GH #330):

> **Affinity alone mints and maps identity references.** Internally they are backed by this
> hive's entity UUIDs, so renaming a person is one mapping edit and the history stays
> attached to the same entity. On the wire travels the affinity-minted vocabulary, and it
> travels **byte-identically**: every other cell -- `receptionist`, the memory hive, `talky`
> -- only transports the string it was handed. None of them resolves it, normalises it,
> re-spells it or looks it up.

Two things follow, and both are the point. A reference that no cell rewrites means the
`audience_set` a turn was stored under and the `audience_set` a later read declares are
comparable at all -- a subset test over two vocabularies is not a test, it is a coincidence.
And a resolution step anywhere else would be a second minting authority: two places deciding
what `member:alex` means is exactly the divergence #330 closed for the *key*, one level down
at the *value*.

The memory hive's writer states its half of this in its own scope note -- it *never looks up
an identity*, `context.speaker` arrives already in affinity vocabulary because translating a
connector's own user id is the talky edge's job (ADR-0002 E8) -- and that is asserted, not
merely written: `a_present_speaker_is_written_exactly_as_it_arrived`
(`crates/meclaw-cells/tests/gh272_identity_travels_per_message.rs`) hands the writer a
reference and reads the stored column back unchanged.

## The AIeOS schema is vendored, and only vendored

`aieos.schema.json` in this directory is a **copy** of the AIeOS 1.1.0 skeleton, pinned at
that version. Nothing in this template opens it at runtime, and nothing in this template
reaches `aieos.org` -- there is no network call, no API binding and no update path other
than a human replacing the file and moving the version with it.

What `gate` enforces is a literal list of mandatory paths (ruling L4, schema-only):

1. `standard.protocol == "AIEOS"`,
2. a `1.x` `standard.version`,
3. a non-empty `metadata.instance_id`,
4. at least one non-empty name in `identity.names`,
5. and no top-level section the vendored 1.1.0 skeleton does not know.

Everything else the spec describes is let **through untouched**. The value of AIeOS is that
it is a foreign, stable vocabulary; that value is destroyed the moment we write into it. So
everything meclaw needs and AIeOS does not have lives in a namespace of its own -- the `mx`
column (`relations_summary`, `trust_default`, `channel_personas`, `provenance`,
`redaction_rules`) and the `relations` / `trust` / `disclosure` tables. An upstream release
of the spec is a file swap plus a version bump; our half never collides with it.

The store enforces **no** schema of any kind -- a column is `text`, `int` or `json` and
nothing more. So `gate` is not the first line of validation, it is the only one.

## Data sovereignty, honestly

The contract is one sentence:

> **Only `affinity/gate` writes the DOMAIN of `affinity/store`. Everything else proposes.**

The word *domain* is doing work there, and it was missing until now. `gate` is the only
cell that writes `entities`, `relations`, `subscribers`' membership and `proposals` --
the rows a reader takes as truth. Two bookkeeping writes stand beside it and are not
proposals:

- **`brief` inserts `audit`.** Every brief, granted or denied, files its own row (`actor`,
  `action: "brief"`, `subject`, `outcome`, `reason_code`). A disclosure ledger that only
  the writer may append to would record nothing about who read what.
- **`push` updates `subscribers` and inserts `audit`.** The push tick writes back
  `pack_hash` and `sent_at` on the row it just served -- that hash **is** the
  push-on-change mechanism: without persisting it there is nothing to compare the next
  tick against, and every tick would resend. Neither column is domain content and neither
  is reachable from a proposal.

So the sentence holds for what a reader believes and does not hold for the two ledgers the
lane keeps about itself.

How far that is *enforced* rather than agreed:

| layer | mechanism | does it hold? |
|---|---|---|
| internal edges | only `gate`, `brief` and `push` have an edge to `./store`. `brief` reads with `select`/`traverse` and appends its own `audit` row; `push` reads, appends `audit`, and updates `pack_hash`/`sent_at` on the subscriber row it served. Neither touches domain content | template convention. The store does not check which op came from whom. |
| external access | a parent scope can no longer wire a deep endpoint into `affinity/store` **by mutation**: `params.ports` is empty, so every path inside the hive is rejected with `hive_port_boundary` (GH #133) and the two lanes are asserted at the hive path itself (`in_brief`, `in_propose`). A **bootstrap** `params.graph` of a parent still can — the birth topology is the colony author's sovereign design; the seal guards against runtime mutation. | **prevented for mutations, by design not for boot.** |
| writing | the store declares `write_surface: "internal"` (GH #132): a write op from a sender outside `/…/affinity` is refused with `write_denied` before it reaches the database, whatever the wiring path. Reads stay free from anywhere — which is what keeps a debug probe straight into `./store` a legitimate move. | **prevented.** |
| `capabilities` | a discovery hint | no. There is no permission layer: whoever can route, may. |
| hard boundary | a sub-colony (own process, own `.env`, own DBs, facade contract) | yes -- and expensive. Not what v1 buys. |

The `writing` row has **two** halves since GH #260, and they are two separate keys on
purpose. `params.write_surface` bounds the ops the store's own `handle()` runs; the
`transfer` body slot is answered by the **substrate**, before `handle()` is ever reached,
so without a second declaration an `import` would write rows straight past the one
sentence this hive is built on. `store/config.json` therefore also carries
`"write_surface": "internal"` in its **`contract`** block. Both halves compute the same
owning scope, so the store has exactly one boundary; an `export` is a read and neither
half bounds it. `clock` carries the contract half as well: its `cell.db` is where the
schedules live, and a planted schedule fires into `./push` with an `emit_to` of the
writer's choosing. `brief`, `gate` and `push` do not -- a `code` cell keeps this lane's
state on the wire (the store round trip *is* its memory), so their `cell.db` holds
nothing to protect and a boundary around it would be decoration.

So v1 is **soft sovereignty**: one port, documented edges, an `audit` table and this README.
A bypass is possible and it is *visible afterwards* -- it is not prevented. The boundary
already stands in the right place (one port, one writing cell), so promoting this to a
sub-colony later is a facade swap and not a re-cut.

The same honesty applies to No-Delete: the substrate's `store` has a `delete` op and would
happily run it. That this hive never deletes is a promise of the **template** (no branch of
`gate` emits one), not a guarantee of the substrate. And the audience filter runs in
`brief`, **after** the read: whoever holds an edge to `./store` reads unfiltered, because
the substrate has no row-level visibility.

Three more limits worth stating plainly: `cell.db` is not encrypted at rest, so the person
data is exactly as safe as the filesystem; the system-write gate of an `llm` cell is
slot-based, not sender-based -- it can say *only `identity`, `peer`, `relationship`,
`channel` are writable*, it cannot say *`identity` only from the affinity*, so which writer
owns which slot stays topological; and a push is free for the subscriber only because the
lane carries no turn (GH #263) -- the slot write itself still costs whatever the
subscriber's next inference pays for a longer system prompt.

## The write ops

All of them arrive as one `tool_call` turn whose `text` is the JSON below.

| op | writes | refused when |
|---|---|---|
| `upsert_entity` | supersede the active row, insert the new one | a mandatory AIeOS path is missing, `kind` is not one of person/agent/org/group/pet, `display_name` is empty |
| `add_relation` | one `relations` row | an endpoint is empty, `kind` is not canonical `snake_case`, `weight` outside 0..100 |
| `retract_relation` | `status: retracted` plus `valid_until` | the id is empty |
| `set_trust` | one `trust` row (append-only, newest wins) | `entity_id` or `audience` is empty (`trust_target_empty`), or the level is not stranger/known/trusted/intimate (`trust_level_unknown`) |
| `set_disclosure` | one `disclosure` row (append-only, newest wins) with the `audience_set` it was released in -- default the addressee alone | `entity_id`, `audience` or `field_path` is empty (`disclosure_target_empty`), the mode is not share/summarize/redact (`disclosure_mode_unknown`), the set is empty (`audience_set_empty`) |
| `subscribe` | deactivate the old row, insert the new one | in check order: `subject` is empty (`subscription_target_empty`), the body carries `cell_path` or `audience` at all (`identity_from_body`), the edge named no subscriber (`subscriber_not_on_edge`) or no actor (`actor_not_on_edge`) |
| `propose` | one `proposals` row, **`status: accepted`** -- pass `auto_accept: false` for a row that waits | source/entity/field reference incomplete |
| `decide_proposal` | marks the judged row `superseded` and **appends** the verdict with `supersedes` | the id is empty (`proposal_id_empty`), the status is neither (`proposal_status_unknown`), or the new row would carry no content (`proposal_incomplete`) |

**The proposal lane has no human gate** (R-AF-1). The system may and shall extend its
picture of a person on its own -- good models judge people well, and the safety net is the
substrate rather than a queue somebody has to work through: nothing is ever deleted and
every addition is traceable. A counselor working deliberately passes `auto_accept: false`;
that is the exception, not the default.

**A verdict appends** (R-AF-4). Deciding a proposal does not overwrite it: the judged row
is marked `superseded` and the decision arrives as a new row pointing back at it. That is
what keeps the agent **answerable** when somebody asks why its picture of them changed --
the timeline is a read, not an archaeology problem. Conflicts between canon and episode are
resolved by time: the temporally latest assertion counts.

Every one of them writes an `audit` row, refusals included -- a refusal is the more
interesting half of the log.

Relation kinds are **canonical English keys** in `snake_case` (`parent_of`, `child_of`,
`works_with`), the same discipline the memory hive holds for predicates and for the same
reason: a relation is a key, not prose, whatever language the turn was in.

## The seed

`store/seed/*.jsonl` ships four **generic placeholder** entities -- three persons (`Alex
Kern`, `Robin Kern`, `Sam Kern`) and one agent (`Aiden`) -- plus their relations, trust and
disclosure rows. No real person appears anywhere in this template. Every AIeOS document in
the seed is built **from the vendored skeleton**, so a seed row cannot carry a section the
pinned schema does not know.

The seed is also the shape a caller can copy: `entity:alex` is a filled-in person, and
`entity:aiden` is a filled-in agent, and they are the same document type. The relation
chain `alex -parent_of-> robin -parent_of-> sam` is there so the two-hop question has an
answer on the first boot, and the disclosure rows are deliberately **narrow**: `agent:aiden`
sees names, text style, idiolect and a summary of the favourites, and nothing else. An
audience that is not in that file is told nothing about anybody, which is the whole point.

Seed applies only on `OpenStatus::Created`; a re-open never overwrites it.

## Settings

**Env knobs are an experimental surface.** Until this template's knobs move onto the `params`
block of the cells that read them, their names carry no compatibility promise and may change in
any `0.x` release; provider credentials keep living in `.env` either way. The migration is
tracked in [#138](https://github.com/mmeyerlein/meclaw/issues/138), with the
`collector@1.2.0` migration ([#136](https://github.com/mmeyerlein/meclaw/issues/136)) as the
reference pattern.

| env | default | what it bounds |
|---|---|---|
| `AFFINITY_DISCLOSURE_ROWS` | 200 | the visibility read for one subject and audience |
| `AFFINITY_TRAVERSE_DEPTH` | 2 | hops the relationship slot walks (store cap 5) |
| `AFFINITY_TRAVERSE_NODES` | 200 | node bound of that walk (store cap 5000) |
| `AFFINITY_SUBSCRIBER_ROWS` | 200 | subscriptions and subjects read per push tick |
| `AFFINITY_PUSH_CRON` | `0 */5 * * * *` | the tick. **Cron runs in UTC.** |

## What is deliberately not here

- **No curator.** A proposal is not *read* by a model in order to be judged: `gate` holds
  no model, and every judgement in it is a comparison. This is the one place where the
  north star *no question may be answered wrongly* and person data meet, and a model that
  decides on its own what is stored about a family member is not worth it. What is
  decided is deterministic and it is decided here: a proposal is accepted as it arrives
  (R-AF-1), and `auto_accept: false` is the one way to get a row that waits.
- **No name resolution.** A `subject` is an entity_id. The store carries the canonical name
  binding (`display_name` -> `canonical_name`, normalising) and an FTS index over it, so a
  lookup lane is a small addition -- it is just not v1.
- **No per-turn recall.** Affinity is never in the turn hot path. The per-turn wire stays
  collector -> memory (`system.memory`); affinity owns `identity`, `peer`, `relationship`
  and `channel`, one writer per system path.
- **No export lane.** The substrate does have a counterpart to the seed -- the `transfer`
  body slot, `export` and `import`, answered before `handle()` for every cell with a
  `cell.db`. This template declares no lane for it and bounds the `import` half to the
  hive scope (`contract.write_surface`, see "The writing row has two halves" above), so a
  backup from outside is a read: an `export`, or a `select` plus a file.
