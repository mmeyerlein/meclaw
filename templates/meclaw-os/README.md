# `meclaw-os@1.8.2`

The colony shell: the outermost of the four composition levels, and the tree everything
else is grown into. It holds no cell of its own. It holds four occupants, one empty
container and the transit graph between them, and its entire job is to be the boundary
those things share.

Since 1.7.0 ([#556](https://github.com/mmeyerlein/meclaw/issues/556)) it is four and not
five. The **submitter** stopped being a hive of this level and became an occupant of the
front door: `/os/operator/submit`. One front, one place a submission lives, and a road that
can be read off the graph — a submission used to cross `./operator` and then `./submit` for
what is one job. What did NOT move is the guardrail: the drafter and the submitter are still
two nodes, they still share no edge, and `/colony/mutations` is still an endpoint no
mutation may draw at any scope (ADR-0015 § Amendment 2026-08-31).

## The rule this level was authored under

**A level owns what its siblings must share.**

All four levels — `meclaw-os`, `org`, `member`, `assistant` — repeat that sentence in the
same words, because it is the only test that decides what belongs at a level and what does
not. Ask it of anything you are tempted to add here: do *all* organisations of this colony
share it? A capability broker, yes — one broker means one answer to "may this actor do this
thing", and two would mean two. A control loop, yes — one colony, one hand on the params.
A memory, no: memory belongs to the **member** (GH #122), and a group is an audience, not a
holder. A persona, a model, a channel: no, no, no. Each of those is owned further down, and
a shell that grew one would have stopped being a boundary and become a participant.

**And one thing more: the OS hands out what is system-near.** A colony carries many
organisations and exactly **one** OS, so the OS is what allocates the resources that are
scarce and colony-wide — a TCP port, a bind address, a socket — where two holders of one
is a collision rather than a disagreement. An organisation does not hold a port band and
does not assign a port; it **asks the OS** for one
(ADR-0022,
[#543](https://github.com/mmeyerlein/meclaw/issues/543)). The form that has today is the
`builder` standing at this level: every member it grows gets a screen, and that screen's
port is `screen_port_base` plus the member's index in its organisation, counted off
`/colony/graph` before anything is rendered. The builder is part of the OS
(ADR-0015), so that is this
level's own responsibility being exercised — never a right the organisation lent it.

## What is inside

| Occupant | What it is | Why it is at THIS level |
|---|---|---|
| `access` | a `ref` to the `access` template — the capability broker, with its own interior `vault` | every organisation asks the same broker; two brokers are two answers to one question |
| `argus` | a `ref` to the `argus` template — the control loop | one colony, one loop; it ships with every goal disabled |
| `builder` | a `ref` to the `builder` template — the intake that drafts a manifest | one authoring path per colony; a second baumeister would be a second audit trail |
| `operator` | a `ref` to the `operator` template — the one front door a person addresses the OS through, and since 1.7.0 the hive the **submitter** lives in | a POST carries no sender, and only this level stands beside both the front door and the container the request has to reach. The submitter went inside it because one submission has one front and one place, and the guardrail it carries is a missing edge between the DRAFTER and the submitter — which is missing wherever the submitter stands |
| `orgs` | a real, empty, open container hive that declares nothing | the address an organisation is instantiated **at**; the shell declares where, not which — and declares the container's lanes for it (see below) |

All four occupants are pinned to an **exact** version. A bare name resolves to the newest
version present on disk, so a shell that named one would silently adopt a new broker the
day a bump landed — which is exactly the drift `registry.template_chain` exists to make
visible, not to excuse. Whatever the reference form, the resolved exact version is what
gets stamped into a grown node's chain, outermost first.

## Lanes

The hive path is the address; `hop.route` is the lane. `params.ports` is **absent** — the
open state — so the shell draws no boundary of its own around its occupants.

| In | Demands | Reaches |
|---|---|---|
| `in_request` | `context.requester` | the broker: may this actor do this thing |
| `in_invoke` | `context.requester` | the broker: spend a grant it already issued |
| `in_cycle` | — | the control loop: run a cycle now rather than at the next tick |
| `in_turn` | — | the `orgs` container, and through it whichever organisation stands inside |
| `in_recall` | — | a question against a member's memory, asked from outside the organisation |
| `in_brief` | — | a read against a member's curated record |
| `in_propose` | — | a write against that record: a correction, a trust decision, a subscription |
| `in_build` | — | the baumeister: a build somebody wants **drafted**. The lane an operator uses, because the first build of a fresh colony has no assistant to raise `build` — `./orgs` is empty by construction. The door stamps `context.build_caller = 'operator'`, which is what sends the draft to the front door instead of back into that empty container |
| `in_submit` | — | the front door: a manifest somebody wants applied. Two callers — a person at the rim, and an assistant inside the colony, whose apply this level re-stamps onto the same lane |
| `in_dump` | — | the front door: produce a dump of a member's memory, triggered from the OS side |
| `in_lifecycle` | — | the front door: birth, sleep or wake for a node somewhere in the colony |
| `in_export` | — | the `orgs` container: hand a member's memory out as a versioned document. The usual sender is the front door, whose `export` lane this level re-stamps onto it |
| `in_import` | — | the `orgs` container: one part of such a document, on its way back into the memory of the member it belongs to. The return leg of `in_export`, crossing untouched — the shell reads no part and decides nothing about what may enter |

| Out | What it carries |
|---|---|
| `grant` | the broker's verdict — a handle, never a credential |
| `ack` | an outcome that was **decided**: the broker spending a grant, or an organisation acknowledging a proposed change. Two senders, one lane |
| `mutate` | the change the loop decided on, as an ordinary params update |
| `alert` | a symptom the loop **watched** for and counted -- the metric, the goal, the count and the window. Deterministic, no model asked, and deliberately not a decision. Nothing here can act on it, so the shell hands it out |
| `answer` | what an organisation produced for whoever asked — a turn answered, a brief read |
| `bundle` | the answer to a question asked at a member's own `in_recall` door, carried out of the organisation and out of the shell. The lane arrived with [#533](https://github.com/mmeyerlein/meclaw/issues/533): the question has crossed this level since it was written, and the answer had no exit at any of the three levels it has to leave |
| `reject` | a refusal from inside an organisation that is a **verdict**, not a failure |
| `error` | a lane of the broker, of the loop or of an organisation failed and it was not a verdict. **The colony that instantiates this shell must drain it, and `reject` with it.** |
| `receipt` | what the front door answered — a submission's counts, an export's result, a refusal, or a lane nobody wired. **Drain it too:** a manifest has no rollback, so an operator who learns nothing is the one outcome there is no recovery from. |
| `write` | an assistant's batched conversation write, handed straight out. The shell owns no archive |
| `turn_write` | one finished turn, offered for archiving as it is produced. A **copy**: since [#527](https://github.com/mmeyerlein/meclaw/issues/527) the member that produced it also fans it into its own memory hive, so this lane is an archive offer and no longer the only place the turn could go. A distribution that wires it nowhere dead-letters it here — `examples/meclaw-os/` routes it to `./sink` |
| `prune` | a housekeeping report raised inside an organisation. Nothing here schedules it |
| `close_report` | what one close pass did to an ended session. Nothing here triggers the pass and nothing here reads the receipt |
| `pack_ack` | the receipt of one identity pack a member's record pushed into one of its generations. Two travel per pack, one per occupant |
| `catalogue` | what one reconciliation of the baumeister's corpus against the colony's own template registry did: how many names the registry holds, how many the corpus already carried, how many rows were written and which. Nothing here reads it — the shell drives the nudge, so the shell hands out the report |

Both broker lanes demand a promoted `context.requester`, and the shell demands it too. A
level forwards a requirement; it cannot satisfy one. An edge that carries `in_request` into
this shell without having promoted the requester somewhere upstream is refused with
`hive_contract` before anything is staged — a grant issued to whoever asked loudest is the
one failure the broker cannot recover from afterwards.

## The forty-nine edges

Thirty-six of them are a door or an exit, and every declared lane has at least one. The
broker knows nothing about the loop, the loop asks the colony rather than the broker, and
neither of them knows an organisation exists.

**Thirteen wire two occupants to each other, and that is what owning a baumeister and a
front door looks like.**
Until R6 every edge here touched the rim, and it was tempting to read that as a rule. It
was a coincidence of who lived at this level: `assistant` wires `./cogny -> ./tools` and
`member` wires `./assistants -> ./firewall`, because a level owns what its siblings must
share and sharing means being wired to it. The container reaches the builder on `build`
with `hop.build_op == 'draft'` and the FRONT DOOR with `'apply'`, and both answer it
back on `in_build_result`. Since GH #435 the submitter asks the broker as well, and that
pair can only be drawn here for the same reason: the two are siblings at this level and
nowhere else — `./operator -> ./access` on `ask` and `./access -> ./operator` back on
`grant` since 1.7.0, where they read `./submit -> ./access` and `./access -> ./submit`
before (#556).

The two edges that carried a manifest out to `./submit` and a receipt back are **gone from
this file**: they are `./intake -> ./submit` and `./submit -> ./intake` inside the front
door now, and they cross nothing. What is left of that round at this level is the pair the
submitter needs from OUTSIDE the hive it lives in — the broker pair above, `./operator -> .`
on `mutate`, and the two nudges into `./builder` on `sub_receipt`. Two further edges carry
the export trigger down into the container and its answer back up.

**An operator can drive the baumeister, and since 1.5.0 that is a lane rather than a
trick.** `in_build` at the rim reaches `./builder`, and the door stamps
`context.build_caller = 'operator'` on the way in — which door a round came in at is a fact
of this level, never a caller's claim. On the way back the marker decides the destination:
`./builder -> ./operator` carries the draft to the one submission front door, re-stamped
`in_submit`, and `./builder -> .` hands a failure out on `error`. The two older
`./builder -> ./orgs` edges carry the counter-guard `context.build_caller != 'operator'`,
so an assistant's round still goes home to its own organisation and an operator's is never
delivered twice — edges fan out, and a lane with two destinations and no discriminator is
two deliveries.

**And since GH #474 the draft STOPS at the front door.** The rim door reads one more word off
the wish, `hop.auto_submit`, and the default is the halt: `./builder -> ./operator` re-stamps an
operator-asked draft onto `in_draft`, where it is parked under its own digest and answered with a
receipt naming that digest — nothing is applied. The operator's second act is an ordinary
`in_submit` quoting the digest and carrying no manifest. `auto_submit: true` keeps the one-act road
for the caller that wants it (a rebuild script replaying wishes somebody already read), and it is
a second guarded edge rather than a branch inside a cell: which road a round takes is a fact of
this level, told by the same discriminator pattern the two `./orgs -> ./builder` / `./orgs ->
./operator` edges have used since R6. The builder's own contract calls what it emits a PROPOSAL —
"a sentence a human can read before saying yes" — and between 1.5.0 and this change that sentence
travelled past the human.

This retracts half of what this file said through 1.4.0. The builder lane pair was
described as deliberately absent from the rim, because `./orgs` raises it and `./builder`
takes it, so it crosses nothing. That is still true of `build` and `in_build_result` — an
organisation's names for the same round. It was wrong about the case that matters most:
the FIRST build of a colony, where the container is empty and no assistant exists to raise
anything, so the road the argument rested on has no traveller and the draft died as
`hive_no_route`. What the lane buys is an answer instead of that silence; whether the
answer is a yes is the broker's business, not the wiring's. The shipped
`colony.mutate.default` policy row scopes an agent's mutations to `/os/orgs`, so a first
ORGANISATION — whose scope is necessarily `/os` — comes back as a named
`requester_not_permitted`, and growing the shell itself stays an operator act at
`/colony/mutations`.

**There is one road to the mutation door, and it starts at the front door.** The container
used to reach `./submit` directly on `build`/`build_op == 'apply'`, so a colony had two
submission fronts: an assistant's and an operator's. R-Zielfluss (a) collapsed them. The
edge is now `./orgs -> ./operator`, re-stamped `in_submit` and marked
`context.operator_caller = 'agent'`; `./orgs -> ./submit` and the `./submit -> ./orgs`
receipt edge that answered it are gone — and since 1.7.0 `./submit` is not a node of this
level at all, so the road runs through the front door by construction rather than by
discipline (#556). The front door writes the same fact into the
correlation id (`op:agent:<id>` rather than `op:<id>`), reads BOTH carriers off the
returning receipt, and puts `hop.submitter_kind` on what it emits. This level routes on
that and on nothing else: an agent's receipt goes back DOWN as `./operator -> ./orgs` with
`in_build_result`, an operator's leaves on the rim. The marker never crosses either edge —
the id on the receipt is the one the caller used, because an assistant's fan-in waits for
the id its own tool call carried.

**Two carriers, because neither survives every road.** A manifest the gate refuses BEFORE
it parks anything leaves no flight row, so the receipt comes back with an EMPTY
`tool_call_id` — and context is what survives that. A manifest that reaches the mutation
door comes back on a fresh trace that carries no promoted context — and the marked id off
the flight row is what survives that. The two failures are disjoint, so the front door
reads both; the details are in `templates/operator/README.md` § Wiring.

**The front door gives an identity, never an authentication.** It exists because
`envelope.reply_to` is stamped on a CELL's emission and a person with a shell is not a
cell — so an operator's submission used to be refused as anonymous, or to walk past the
gate entirely through `/colony/mutations`. What it supplies is a path. It supplies no
token, checks no header and keeps no secret: that is a reverse proxy's job in front of the
API, and the broker's behind this level.

**Since 1.6.0 the submitter also nudges the corpus, and this is the only level that can
draw that edge either.** The baumeister's librarian holds its corpus as a **seed**: it is
loaded once, when the store's `cell.db` is created, so it describes the library of the
moment the colony was born. A class registered afterwards by an `add_templates` is
resolvable at the mutation door and invisible to the composer — measured as seven rounds
spent looking for a template that had been in the library for an hour. GH #496 built the
reconciliation and named the wiring that should fire it: *an edge from `submit` after a
committed submission whose diff carried `add_templates` — the one cell that knows both
facts.* The form it named, `./submit/gate -> ./builder/librarian`, is undrawable by
anybody: both endpoints are interior nodes of sealed hives, and the refusal
(`hive_port_boundary`, twice) is right on both counts. The drawable form is the one
between the two **hive paths**, and it needed a lane at each end: `builder` accepts
`in_ingest` and forwards it to its librarian, `submit` publishes
`hop.registers_class` on the receipt, and the edge here reads both that key and the
absence of `hop.error_code`. Since 1.7.0 the edge starts at `./operator` and reads the
lane `sub_receipt` (#556): the submitter is an occupant of the front door, and its own
receipt leaves that hive on a lane of its own precisely so that a submission is not
answered twice on the lane a caller subscribes to. Both guards are load-bearing — the key alone would nudge
after a manifest that registered a class and was refused at the door, and `committed`
alone after every submission a colony ever makes. `./builder -> .` carries the report back
out on `catalogue`, because the baumeister pairs the two in its own `required_drains`: the
counts are the only thing that tells *nothing was missing* from *the nudge never ran*.

```json
{"add_edges": [
  {"from": "<shell>",           "to": "<shell>/access",  "condition": "has(hop.route) && hop.route == 'in_request'"},
  {"from": "<shell>",           "to": "<shell>/access",  "condition": "has(hop.route) && hop.route == 'in_invoke'"},
  {"from": "<shell>/access",    "to": "<shell>",         "condition": "!has(context.sub_ask) && has(hop.route) && hop.route == 'grant'"},
  {"from": "<shell>/access",    "to": "<shell>",         "condition": "has(hop.route) && hop.route == 'ack'"},
  {"from": "<shell>/access",    "to": "<shell>",         "condition": "has(hop.route) && hop.route == 'error'"},
  {"from": "<shell>",           "to": "<shell>/argus",   "condition": "has(hop.route) && hop.route == 'in_cycle'"},
  {"from": "<shell>/argus",     "to": "<shell>",         "condition": "has(hop.route) && hop.route == 'mutate'"},
  {"from": "<shell>/argus",     "to": "<shell>",         "condition": "has(hop.route) && hop.route == 'alert'"},
  {"from": "<shell>/argus",     "to": "<shell>",         "condition": "has(hop.route) && hop.route == 'error'"},
  {"from": "<shell>",           "to": "<shell>/orgs",    "condition": "has(hop.route) && hop.route == 'in_turn'"},
  {"from": "<shell>",           "to": "<shell>/orgs",    "condition": "has(hop.route) && hop.route == 'in_recall'"},
  {"from": "<shell>",           "to": "<shell>/orgs",    "condition": "has(hop.route) && hop.route == 'in_brief'"},
  {"from": "<shell>",           "to": "<shell>/orgs",    "condition": "has(hop.route) && hop.route == 'in_propose'"},
  {"from": "<shell>/orgs",      "to": "<shell>",         "condition": "has(hop.route) && hop.route == 'answer'"},
  {"from": "<shell>/orgs",      "to": "<shell>",         "condition": "has(hop.route) && hop.route == 'bundle'"},
  {"from": "<shell>/orgs",      "to": "<shell>",         "condition": "has(hop.route) && hop.route == 'ack'"},
  {"from": "<shell>/orgs",      "to": "<shell>",         "condition": "has(hop.route) && hop.route == 'reject'"},
  {"from": "<shell>/orgs",      "to": "<shell>",         "condition": "has(hop.route) && hop.route == 'error'"},
  {"from": "<shell>/orgs",      "to": "<shell>",         "condition": "has(hop.route) && hop.route == 'write'"},
  {"from": "<shell>/orgs",      "to": "<shell>",         "condition": "has(hop.route) && hop.route == 'turn_write'"},
  {"from": "<shell>/orgs",      "to": "<shell>",         "condition": "has(hop.route) && hop.route == 'prune'"},
  {"from": "<shell>",           "to": "<shell>/orgs",    "condition": "has(hop.route) && hop.route == 'in_export'"},
  {"from": "<shell>",           "to": "<shell>/orgs",    "condition": "has(hop.route) && hop.route == 'in_import'"},
  {"from": "<shell>/orgs",      "to": "<shell>",         "condition": "has(hop.route) && hop.route == 'close_report'"},
  {"from": "<shell>/orgs",      "to": "<shell>",         "condition": "has(hop.route) && hop.route == 'pack_ack'"},

  {"from": "<shell>/orgs",      "to": "<shell>/builder", "condition": "has(hop.route) && hop.route == 'build' && has(hop.build_op) && hop.build_op == 'draft'"},
  {"from": "<shell>/orgs",      "to": "<shell>/operator","condition": "has(hop.route) && hop.route == 'build' && has(hop.build_op) && hop.build_op == 'apply'"},
  {"from": "<shell>/builder",   "to": "<shell>/orgs",    "condition": "has(hop.route) && hop.route == 'manifest'"},
  {"from": "<shell>/builder",   "to": "<shell>/orgs",    "condition": "has(hop.route) && hop.route == 'error'"},
  {"from": "<shell>/operator",  "to": "<shell>/orgs",    "condition": "has(hop.route) && hop.route == 'receipt' && has(hop.submitter_kind) && hop.submitter_kind == 'agent'"},
  {"from": "<shell>/builder",   "to": "<shell>",         "condition": "has(hop.route) && hop.route == 'catalogue'"},
  {"from": "<shell>/operator",  "to": "<shell>/builder", "condition": "has(hop.route) && hop.route == 'sub_receipt' && !has(hop.error_code) && has(hop.registers_class) && hop.registers_class == true",
   "modifier": {"set_hop": {"route": "'in_ingest'"}}},
  {"from": "<shell>/operator",  "to": "<shell>",         "condition": "has(hop.route) && hop.route == 'mutate'"},

  {"from": "<shell>/operator",  "to": "<shell>/access",  "condition": "has(hop.route) && hop.route == 'ask'",
   "modifier": {"set_hop": {"route": "'in_request'"},
                "set_context": {"requester": "'/os/operator/submit'", "sub_ask": "'1'",
                                "sub_sha": "hop.manifest_sha256"}}},
  {"from": "<shell>/access",    "to": "<shell>/operator","condition": "context.sub_ask == '1' && has(hop.route) && hop.route == 'grant'",
   "modifier": {"set_hop": {"route": "'in_verdict'"}}}
]}
```

**The submitter asks the broker, and only the shell can draw that pair** — since 1.7.0
across the front door's rim rather than the submitter's own, because the two hives are
siblings here and nowhere else. The submitter
holds no policy of its own: it parks a manifest under its digest and asks *may this
identity have a manifest applied under this scope root*, once, in the check-only form.
Three things about the outward edge are load-bearing and none of them is decoration.

`context.requester` is stamped to the submit hive's own path — `/os/operator/submit` since
1.7.0 — because the broker reads the requester from the **edge** and never from a body
(R-AC-1); the identity on whose behalf it asks travels as `subject` *inside* the question,
so the delegation is visible in the rule ("submit may mutate on behalf of S under P")
instead of implicit in a script. It names the **occupant** and not the hive around it: the
reach the rule is about belongs to the submitter, and `/os/operator` would grant an export
cell and a lifecycle composer the same thing. The edge may say so honestly because `ask` has
exactly one sender inside that hive, which the `operator` template declares.

`context.sub_sha` carries the digest, because `hop.*` lives for exactly one hop and the
verdict has to be matched back against the manifest it was asked about.

The marker is `sub_ask`, in the **submitter's** key space. `access` overwrites `ac_phase`
and `ac_carry` on its own internal edges, so a marker under those names would not come
back. And the rim edge for `grant` now excludes it: edges **fan out**, so without that
guard one check-only question would answer the submitter *and* hand a grant to whoever
wired the shell's `grant` lane — an answer to a question the outside never asked.

**`mutate` carries two senders now**, the control loop and the submitter, and that is this
file's own precedent applied a third time: `ack` and `error` each carry two already, and a
caller subscribes to a lane rather than to whoever raised it. One lane means one privileged
edge out of the shell, and one privileged edge means one audit trail — which is the whole
of R6's guardrail. The builder gets no such edge, and cannot be given one later:
`/colony/mutations` is not an endpoint a mutation may draw, on any scope.

Those edges travel with the template; they are shown here so a reader can see the shape,
not so anyone has to draw them. The last three arrived when the level below them started
re-emitting what a member's assistants raise and nothing at member level consumes — nobody
re-derived the list by hand, `gh302_meclaw_os_shell.rs` read `templates/org/config.json`
off the tree and went red until this level moved with it.

## The container

`orgs` is a **real** hive with no children, no edges and **no `params` block at all**.

- **Real**, because #303 forces the same shape one level down — `channels` has to be a node
  to carry its fan-in edges once — and four levels with one shape beat four levels with
  three. The live tree already grew this way.
- **Empty**, because an organisation is instantiated *into* it. It ships as an address, not
  as a tenant.
- **Open** — `params.ports` absent, not `[]` — because the mutation that instantiates an
  organisation draws edges to that organisation, and a sealed hive refuses exactly those
  endpoints with `hive_port_boundary`.
- **Silent**, and that is the part worth explaining.

### Why the container declares nothing and the level declares everything

The obvious shape is the wrong one: give the container its own `params.contract` naming the
transit lanes, so a reader can wire against it. That declaration is a trap, and it is a trap
that hides itself.

A hive contract is checked inward: every accepted lane must route from the hive path **to a
cell inside it**, every emitted lane must route from inside back out. The container is empty
by construction, so it has no inside — no lane it declared could ever have a door. The check
does not fire while nothing addresses the hive (`hive_path_is_wired` treats an unaddressed
contract as dormant), which means such a declaration ships **green**. It stops being green
the moment somebody draws the first edge to the container — an operator, an example, the
mutation that instantiates the first organisation — and from that moment **every** mutation
of the colony is refused with `hive_contract` until an organisation actually stands inside.
A declaration that is green only because nobody is looking is the same defect class as the
slot this wave removed from all four levels, and it is caught the same way: by asking what
the code does rather than what the field is named.

So the rule, and all four levels follow it: **the level declares the transit lanes, and the
level's own edges satisfy them from birth.** `in_turn` has a door because the shell routes
it into `./orgs`, and `orgs` lies inside the shell; `answer`, `ack`, `reject`, `error`,
`write`, `turn_write`, `prune`, `close_report` and `pack_ack` have exits because the shell
routes them back out of `./orgs`. Below the container there is nothing to route to
until an organisation is instantiated, and the mutation that grows one draws its own edges —
but the level's promise is already true and already checkable on the day it ships.

## What is deliberately not here

**No second vault.** GH #302's original sketch listed `vault` beside `access` at this level.
It is not here, and that is a ruling (Q20), not an omission: `access` already carries
its own interior `vault`, reachable from nowhere outside, and the standalone `vault`
template attests its inbound edges against `params.broker` and `params.sealed_neighbors`.
With no broker at this level, a second one would boot locked and inert — a credential store
that answers nobody, which reads like a working component right up to the moment somebody
needs it.

**No swallowing sink.** No `terminal`, and no refusal lane that ends inside the shell. The
dead-letter queue is the record (GH #284, ruling Q2). All three `error` exits leave the level;
what happens to them is the colony's decision, made where the colony is wired.

**No `turn` lane outward, and that is the union rule doing its job.** A level declares the
union of what its occupants accept and emit, **minus the lanes a sibling inside the level
consumes itself** — explicitly written out, with the versions each entry was derived from in
its `because`. `turn` is the one subtraction on this level: a member consumes its own turn
behind its screen (`./assistants -> ./firewall`), so nothing ever reaches an organisation's
boundary on that lane, and an exit for it here could only ever have been an edge that never
fires. What does come back out is `answer`, `ack`, `reject` and `error`. Two of those,
`ack` and `error`, have **two senders and one declaration**: a caller subscribes to a lane,
not to whoever raised it.

**No `connect` lane outward.** The broker emits one, and this shell does not pass it on.
`access` requires `connect` to be the *only* edge that reaches a connector cell, and a
connector lives four levels down — so that edge is drawn where the connector stands, in the
same mutation that instantiates it. Re-emitting the lane here would offer a second way in
and quietly end the broker's monopoly, which the substrate has no permission layer to
restore.

**The unbound behaviour of `orgs` is undeclared, and that is measured rather than
overlooked.** The substrate's slot declaration governs an address that does **not** exist;
a container hive that exists but has no children counts as *bound*, so a declared `unbound`
word could never fire for it — and writing `params.ports` for the slot's sake would have
**sealed** this shell, turning a silent declaration into a harmful one. So a message that
reaches `orgs` before an organisation is instantiated into it takes the ordinary path. The
full finding was read out of the slot resolution itself:
`unbound_slot_behaviour` in `crates/meclaw-colony/src/colony.rs`, which steps aside as soon as the target is a registered hive scope.

**This shell's argus talks to the colony directly.** Since GH #267 the loop's meter and
probe ask `/colony/ledger` for their numbers instead of opening a database they do not own
— `ARGUS_COLONY_DB` was retracted with that change. The two absolute lanes that carry
those asks travel with the `ref`, so this level writes no edge for them; it also must not
seal them away, which is one more reason `params.ports` stays absent here. The virtual
endpoints are in bounds at every scope, so wrapping the loop one level deeper changes
nothing about them. Their two settings, the meter's `max_ledger_rows` and the
probe's `probe_ledger_tries`, both carry defaults inside the loop's own cells and need no
pass-through from this level; since `argus@1.1.0` they are PARAMS of those cells
(GH #138), so an operator who wants other values sets them with `override_params`
in the manifest that grows the loop, not in an environment every occupant shares.

**No `requires` block.** This template substitutes nothing in its own config values, so it
declares nothing. The requirements of `access` and `argus` are **not** repeated here:
W3's validator resolves each node's ref chain back to the template it came from and
collects that template's own declaration, so a requirement of a referenced template is
already a requirement of this composite.

## Stage one: one declaration

A built colony arrives in two stages. **Stage one** puts this shell there. **Stage two** is the
authoring path inside it growing everything else — an organisation, a member, an assistant — as
manifests the baumeister drafts and the submitter carries to the door. The two are different
kinds of act, and stage one is deliberately the small one: it takes a root tree with **no cell
in it at all**.

```
seed-ref/
├── colony.json            substrate defaults. two lines.
├── main/config.json       type: "hive", one edge, and not one cell
└── main/os/config.json    {"cell": {"type": "ref", "template": "meclaw-os@1.8.2"}}
```

```bash
meclaw --root ./examples/meclaw-os/seed-ref --templates ./templates \
       --daemon --api 127.0.0.1:7777
```

That third file is a **declaration, not a cell**. The FIRST start resolves it against the
template library and grows it — through the very resolution and staging a mutation takes, which
is why the result is byte-identical to what the equivalent `add_nodes` builds. Then the marker
is **gone**: what stands at its address is the shell it named. Two consequences follow from that
rather than being bookkept — a second boot finds nothing to grow, and a node you later remove
with `remove_nodes` cannot be re-declared into existence by a restart.

**The one edge is the whole birth topology.** `./os -> /colony/mutations`, conditioned on the
`mutate` lane, is the line the next section explains: it cannot be added by a mutation on any
scope, so it lives in the root tree or it does not exist. A `ref` marker declares a **node** and
never an **edge**, so the seed writes it by hand — one line, and the only one.

**It is read, not generated.** A colony root somebody's script writes before the first boot is a
tree nobody can diff, and the two things that decide whether the colony can ever change itself —
the marker and that edge — are exactly the two a generator gets wrong in silence. So the root
tree is checked in, beside the example that uses it, and pinned:
[`examples/meclaw-os/seed-ref/`](../../examples/meclaw-os/seed-ref/) with
`crates/meclaw-cells/tests/gh465_one_declaration_boots_the_os.rs`. That test is the only proof
stage one may cite.

What stage one does **not** do is grow the organism. A marker names one node; everything under
`./orgs` is stage two's business, and `examples/organism/` is where the whole stack from one
file is written out.

## What it needs before it runs

`template.json` § `requires.env` is this level's environment surface, rolled up from its
occupants and derived from them by test: every `${VAR}` any config value under this shell
substitutes is named there, with a sentence saying what it binds and why.

**The surface shrank with the knob migration, and it is finished.** Since
`1.8.1` the three `DISPATCHER_*` names and the four `BUILDER_LIBRARIAN_*` ones
are gone from the rollup. Since `dispatcher@1.2.0` that cell reads its call
budget and its two tool classes from its own `params`,
and since `builder-librarian@2.2.0` the library reads its four windows from
its own ([#138](https://github.com/mmeyerlein/meclaw/issues/138)), so a key set
for either in the environment would go nowhere. What tunes them now is an
`override_params` entry addressed at the cell --
`{"builder/dispatcher": {"max_calls": 8}}` -- and what stays here is the
provider lane and nothing else.

Exactly **one** of those keys is required, and it is the only value in the whole shell written
with no default: `OPENROUTER_API_KEY`, the credential of the control loop's judge. A colony that
grows this shell without it is refused with `requirement_missing` **before a single byte is
staged** — the marker is still a marker afterwards, there is nothing to clean up, and the
refusal quotes the declaration's own sentence so a reader learns what the key is for. The
alternative, and the state before GH #465, was a shell that boots, looks healthy, and fails at
the first cycle the loop runs.

Everything else has a default in the template it configures and is declared anyway, so that a
builder learns this shell's environment surface by reading it rather than by watching a cell
answer nothing. Since `meclaw-os@1.8.2` that surface is seven keys instead of
twenty-nine, and every one of them is the provider lane: the authoring dispatcher's
three knobs, the control loop's seven, the capability broker's six, the two source
names of the vault inside it and the library's four windows became params
of the cells that read them (GH #138), so they are set in the manifest that grows the
shell and are no longer asked for here. A declared key that binds nothing is a
leaflet, and the roll-up is derived by test rather than transcribed, so it went out
with them.
Two of the remaining keys are worth knowing before you start:
[`MODEL_BUILDER`](../builder/) and `LOCAL_LLM_BASE_URL` are what the composer asks its model
through, they default to empty, and unset they leave the authoring path inert — the shell boots,
the front door answers, the submitter gates, and no manifest is ever drafted. **Stage two cannot
run until they are set.** A copy-ready file with every name and no value stands beside the seed:
[`examples/meclaw-os/seed-ref/.env.example`](../../examples/meclaw-os/seed-ref/.env.example).

## Growing a colony out of it

The other way in — into a colony that is already up, from an operator's shell rather than from a
root tree:

```json
{"scope": "/",
 "diff": {"add_nodes": [{"name": "os", "template": "meclaw-os@1.8.2"}],
          "add_edges": []}}
```

One node and no edges at all: the shell is the outermost boundary, so what reaches it comes
from outside the colony and what leaves it leaves the colony. The broker, the control loop,
the `orgs` container, the baumeister, the submitter, the front door and every edge between them
came with the template.

**One line is still missing, and it has to be written by hand:** the edge from the shell to
`/colony/mutations`. It cannot be added by a mutation on any scope, so it lives in the
birth topology of the root or it does not exist. A colony without it is one where the
control loop measures and reports and changes nothing, and where a submission ends as a
`no_route` in the dead-letter queue — loudly, and localising itself. That is the right
default: a colony nobody privileged applies nothing.

Then one organisation at a time, into the container, each with its transit edges in the
**same** mutation — a hive is an island until an edge crosses into it:

```json
{"scope": "/os",
 "diff": {"add_nodes": [{"name": "orgs/acme", "template": "org@1.4.0"}],
          "add_edges": [{"from": "./orgs", "to": "./orgs/acme",
                         "condition": "has(hop.route) && hop.route == 'in_turn'"},
                        {"from": "./orgs/acme", "to": "./orgs",
                         "condition": "has(hop.route) && hop.route == 'answer'"}]}}
```

**The spelling is worth copying exactly.** A node is addressed by its `name` plus the
mutation's `scope`, and the scope is the level *above* the container — the node's `name`
carries the `/`. Writing `"scope": "/os/orgs"` and then `"from": "."` does not work: an
`add_edges` endpoint is resolved as a node name, and `.` names none, so the whole diff is
rejected with `edge_schema`. An **absolute** endpoint does not work either — `"from":
"/os/orgs/acme"` is refused with `scope_out_of_bounds` before anything else is looked at,
whatever the scope. Endpoints are scope-relative, always.

A fully wired organisation costs one such edge per lane the [`org`](../org/) level
declares — its accepts down into the node, its emits back up into the container.
[`examples/organism`](../../examples/organism/) is the whole stack written out that way:
five declarations, one per level, and its organisation step draws the seventeen its
walkthrough exercises.

The broker starts inert by design — every seeded policy row ships disabled — and so does
the control loop: every charter goal ships disabled too. A fresh shell therefore grants
nothing and changes nothing until an operator turns on exactly what they mean.
