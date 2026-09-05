# `cogny@5.0.0`

The agent core as one template. Four units under one hive:
[`collector`](../collector/) and [`dispatcher`](../dispatcher/) -- each carrying its
template's own name -- plus ONE `llm` `brain` and one `code` cell, `schemas`, which
hands out the schema of the errand this core takes. No new cell type, no Rust.

**One brain, since 4.4.0** ([#528](https://github.com/mmeyerlein/meclaw/issues/528)).
Until then the seam had two lanes and the core carried a fast one for memory lookups. The
right owner of a fast memory question is the **conversation surface** -- it already holds
the window, and asking it there costs one tool call instead of an advisor round trip, which
is the very thing [#124](https://github.com/mmeyerlein/meclaw/issues/124) measured. What is
left here is one class of question -- synthesis, a development over time, multi-step work,
research -- and one class needs one lane. `brain_fast`, `escalate_to_deep`,
`context.consult_class` and `ctx.model_fast` went with it.

**Structurally a talky without a channel.** The advisor split (GH #28, R-CG-1) gives an
agent two brains: a fast [`talky`](../talky/) that owns the channel, and this one, which
owns the thinking. The core therefore carries no session keeper, no summarizer and no
proxy -- it has no channel, no sessions and no night. Its "conversation" is the errands
the channel voices send it, and the memory it reads is the member's central hive rather
than a window over one chat.

**One core, N channel voices.** Cogny is a *sibling* hive of the talkies at agent level
(`<agent>/{talky…, cogny}`), never a cell inside one and never one per talky (R-CG-2). Two
talkies consulting the same core is the normal shape. The memory and its archive are not in
that set: a `memory-hive` is the source of truth of a **member**, not of an agent, so it
sits beside the agent rather than inside it
(`<member>/{assistants/<agent>/{talky…, cogny}, memory, archive}`). Every agent the member
runs is a lens on the same hive, and a second one inherits what the member already knows.

## What it delivers

- **The seam, already bounded, and its own.** The collector hands the assembled errand to
  the brain over ONE edge carrying the iteration counter and `restore_ttl` -- a second
  copy of the mechanism the talky has, with its own bound, because a consultation is a
  longer round than a chat turn.
- **A tool round that only needs its tools.** `brain -> dispatcher -> (your tools) ->
  collector -> brain` is pre-wired except for the one lane that is genuinely
  per-instance: which cell answers to `web_search`. Adding a tool is one edge pair.
- **A consultation that looks like a turn.** The errand arrives on the collector's
  `in_turn` lane and is filed as the turn it is: the talky IS the core's user. Nothing in
  here knows that its user is a machine.
- **An answer that is an event.** The advice leaves on the ordinary `answer` route and
  becomes the asking talky's `in_advice` event -- which is why the same lane carries a
  *question back* without a second mechanism.
- **Nobody waits.** The consult is classified at the asking dispatcher
  (the asking dispatcher's `handoff_tools` names `consult_cogny` -- a handoff is async and says in
  the same breath that the answer comes from a later turn, GH #372), so the asker's fan-in
  opens no expectation for it and the round it leaves behind is over. Thinking time never
  races an idle window. That property lives on the *asking* side; this template is the half
  that is allowed to be slow.
- **A memory it asks on purpose (4.4.0, moved in 5.0.0).** `memory_tier` is empty: the core
  has no ambient bundle handed to it and a `memory_recall` tool instead, with a time range
  and a session it chose itself. A problem solver asks; it is not read to. Since 5.0.0 the
  tool is not this composite's to answer ([#552](https://github.com/mmeyerlein/meclaw/issues/552)):
  the member's own memory hive declares the schema and serves the call, and the call leaves
  here on the ordinary tool exit like any other name.
- **One answer per consultation (4.4.0, #539).** *No channel* is enforced here rather than only
  stated: the `./dispatcher` ref marker carries `override_params {"": {"interim": ""}}`, so
  the sentence a thinking model puts next to its tool bundle -- "I am checking the official
  fares now" -- does not leave the cell. It used to leave on the `answer` lane, which for
  this composite is the asking voice's `in_advice`: the voice was handed an advisor's answer
  that was not one, said it in the channel, and sometimes consulted again, which the core
  answered with its next interim sentence. Measured on a live colony: 11 of 26 answers on
  that lane were interim, and one user turn produced thirteen messages
  ([#539](https://github.com/mmeyerlein/meclaw/issues/539)). See
  [Knobs](#knobs).
- **And an errand nobody has to type (4.4.0).** The core answers `in_schemas` with the
  schema of `consult_cogny`, in the tools hive's own shape, on `tool_schemas`. Whoever is
  reached declares themselves -- see [The core declares its own errand](#the-core-declares-its-own-errand-528).

## Cells

| path | type | from |
|---|---|---|
| `collector/{assemble,window}` | `code`, `store` | `collector` **(sealed)** |
| `dispatcher` | `code` | `dispatcher` (a single-cell template) |
| `brain` | `llm` | this template -- the one inference |
| `schemas` | `code` | this template -- the errand schema (4.4.0; named `declare` until 4.5.0) |

**The braces are an inventory, not an address list.** `collector` declares
`params.ports: []`, so `./collector` is the only address an edge from outside may name and
`./collector/assemble` is refused with `hive_port_boundary`; which cell inside takes the
message is decided by the `in_` lane the edge sets.

### How the sub-units are referenced: by name and version (GH #277)

The two sub-units are **references**, not copies. Each of the two directories holds one
`config.json` and nothing else:

```json
{"cell": {"type": "ref", "template": "collector@4.0.0"},
 "override_params": {"assemble": {"context_window": 128000,
                                  "curate_soft": 0.5,
                                  "curate_hard": 0.75,
                                  "tools": ["*"]}}}
```

Since `4.1.0` the collector reference carries an `override_params` block, and that is where
the curator is switched **on** -- see [The curator, live](#the-curator-live) below.

**And where the memory TOOL went** (`5.0.0`,
[#552](https://github.com/mmeyerlein/meclaw/issues/552)). It was here from `4.4.0` to
`4.6.1`: `memory_call_tier` decided whether a `memory_recall` call was answered out of the
collector's own recall port or refused with a typed error, one ordinary
`./dispatcher -> ./collector` edge kept the call inside, and the schema the model read was
typed by hand -- in a template that answers no recall -- as a projection of the memory hive's
own `in_query` contract. Three copies of one contract, each able to drift on its own. The
hive declares and answers the name now, so this composite does with `memory_recall` what it
does with `web_search`: the dispatcher names it, the guarded default carries it out, and an
edge the PARENT draws knows the cell. Nothing here has to be switched on for it -- the
declared list is `["*"]`, so whatever answerers the level wires are asked, the memory among
them.

**The ambient leg goes the other way.** `memory_tier` stays empty, and that is the whole
shape of the ruling: the core is the problem solver, so it asks about a time range or a
session **on purpose** and is not handed a bundle before it has read the question. The
conversation surface is the one that wants the free floor, because it is the one with a
person waiting. `thread_recall` is unchanged and stays on (GH #451), and
since `collector@3.3.1` (GH #512) both names ride out on every menu the collector
writes -- they are the two tools this hive serves itself, and no tools hive has a
declaration for either.

At instantiation the referenced template's tree takes that position, so the instance is
byte-for-byte the tree the copies used to produce -- and every cell inside it now records
the template it really came from: `collector/assemble` is stamped with the `collector` version it was grown from, with
`cogny@5.0.0` above it in its provenance chain.

**The library has to carry both.** A reference resolves against the colony's template
registry, so `collector` and `dispatcher` have to sit in the same `templates/` directory
as `cogny` -- as they do in the shipped library. A tree that copied `cogny` alone gets
`template not found` at the mutation, not at boot.

**The version is pinned on purpose.** A bare `collector` would resolve to whatever the
highest version on disk happens to be, so a standalone bump would silently re-point this
composite. The pin makes the composite say which version it was built against; moving it
is a `cogny` bump, in the same commit.

Until GH #277 the sub-units lived here as byte copies of their `config.json` files, held
against their sources by a byte-identity pin. Its successor is
`crates/meclaw-colony/tests/gh277_composite_instantiation_is_byte_identical.rs`: the two
golden manifests prove the instantiated bytes did not move, and
`a_cell_inside_talky_is_stamped_with_its_own_template_and_names_talky_above_it` proves
the origin is recorded.

## Ports

**Four external ports in two pairs, and the parent wires each pair in the SAME mutation
that instantiates the composite** -- an island without a crossing edge derives inactive.
All four meet at the hive path; five further lanes (`in_tool`, `in_bundle`, `tool`,
`recall`, `error`) meet there too and are wired per instance, see [Lanes](#lanes).

| port | endpoint | direction | what travels |
|---|---|---|---|
| consult ingress | `./cogny` | in | the errand on lane `in_turn`, carrying `context.consult_id` **and `context.session_id`** |
| advice exit | `./cogny` | out | `hop.route == 'answer'` -- the advice, a question back, **or** a store refusal marked `hop.degraded` (see Lanes) |
| declaration ingress | `./cogny` | in | `in_schemas` with `{"tools": [...]}` -- what does your errand look like? |
| declaration exit | `./cogny` | out | `hop.route == 'tool_schemas'` -- the `consult_cogny` schema, provider-neutral |

```json
{"from": "<front>/surface", "to": "./cogny",
 "condition": "has(hop.route) && hop.route == 'tool' && has(hop.tool_name) && hop.tool_name == 'consult_cogny'",
 "modifier": {"set_hop": {"route": "'in_turn'"},
              "set_context": {"consult_id": "hop.consult_id",
                              "session_id": "context.session_id", "col_phase": "''"},
              "restore_ttl": true}},
{"from": "./cogny", "to": "<front>/surface",
 "condition": "has(hop.route) && hop.route == 'answer'",
 "modifier": {"set_hop": {"route": "'in_advice'"},
              "set_context": {"col_phase": "''"},
              "restore_ttl": true}}
```

**The ingress is ONE edge since 4.4.0.** The second one carried `ask_memory` and set
`context.consult_class` to `'lookup'`; the class, the lane and the tool name are all gone
([#528](https://github.com/mmeyerlein/meclaw/issues/528)). A caller that still promotes
`consult_class` is not refused -- nothing in here reads it any more.

Four things in that pair are load-bearing, and none of them is decoration:

- **`col_phase` must be cleared, on BOTH edges.** Each message leaves *another*
  collector's chain and carries whatever step that chain was in. A collector's `in_turn` /
  `in_advice` refuses a message that arrives mid-assembly, so the port edge resets the
  key. Everything else in the context rides along on purpose.
- **`consult_id` becomes context**, because the hop decays at the next cell and the
  correlation has to survive the core's whole chain and come home with the answer. A
  *fresh* consult is named by the call that opened it; a reply to a question the core
  asked back passes the id it was shown -- the dispatcher decides which, and both arrive
  here as the same key.
- **`session_id` becomes context, and the lane DEMANDS it.** `accepts[].context` names it
  (GH #291 makes that requirement checkable by a backwards walk, so a mutation that draws
  an ingress without it is refused rather than discovered at runtime). The reason is the
  memory tool of 4.4.0: a core that may ask its member's memory about *this conversation*
  has to be able to say which one, and a consultation belongs inside the session that
  raised it. The shipped form reads it out of the CALLER's own context
  (`"session_id": "context.session_id"`) and not off the hop: a talky's session keeper puts
  it there on the first edge of every turn, and no cell between there and here emits it as a
  hop key -- `dispatcher`'s contract has no `session_id` in `emits.hop`, so an edge promoting
  `hop.session_id` would fail its modifier and be skipped, which loses the errand in
  silence. Re-setting a key that already rides along is not redundancy: it is what makes the
  requirement local to the edge that owes it, and what the checker reads.
- **`restore_ttl` on both**, with the condition they already carry: an errand is a
  fresh journey, not the tail of the turn that started it, and the advice home is another.
- **The errand arrives as a `tool_call` turn.** Its text is the raw arguments the model
  wrote -- `question` and `context`, both required. The core's collector files that as the
  turn.

### The core declares its own errand (#528)

The second pair, and it is the same lane pair a tools hive answers on
(`templates/tools/README.md` § *Asking for the declarations*). That is the whole point: an
asking collector can put the question to two answerers and cut the two menus together
without knowing which of them was a hive of tools.

```json
{"from": "<front>/surface", "to": "./cogny",
 "condition": "has(hop.route) && hop.route == 'schemas'",
 "modifier": {"set_hop": {"route": "'in_schemas'"}}},
{"from": "./cogny", "to": "<front>/surface",
 "condition": "has(hop.route) && hop.route == 'tool_schemas'",
 "modifier": {"set_hop": {"route": "'in_menu'"}}}
```

`params.required_drains` refuses a mutation that draws only half of it, for the reason that
pair always carries: a caller that asks and does not subscribe offers its model a menu
without `consult_cogny` in it, and the round then looks like a model that chose not to
consult -- the one failure nobody can see from outside.

**What a caller sends** is `{"tools": ["consult_cogny"]}`, or `["*"]` for everything this
core declares, which is one schema. **What comes back** on `tool_schemas` is
`schemas[]` / `unknown[]` / `messages[]` in the body and `operation` / `schema_count` /
`unknown_count` / `error_code` (`tool_unknown`, `tools_missing`) on the hop -- byte for
byte the tools hive's answer shape, and provider-neutral for the same reason: wrapping the
envelope is the caller's job, because the caller is the one that knows its provider.

The single schema:

| field | type | |
|---|---|---|
| `question` | string | **required** -- the one thing the core has to answer |
| `context` | string | **required** -- everything the core needs: what the person wants, what was already said, what is excluded |
| `eta` | string | optional -- the asker's own coarse guess, said to the person in the same reply |
| `consult_id` | string | optional -- names a consultation that is already open |

**`context` is required, and it is required to be redundant.** The asking model must not
filter it against what it thinks the core already knows: the core's curator discards what
it does not need at assembly time, and it cannot recover a sentence that was never sent.
That instruction is in the schema's own `description`, which is also where the **class
boundary** lives -- synthesis, time series, multi-step work and research come here; a quick
fact the asker looks up in its own memory. One sentence in one file beats a paragraph copied
into every persona, and the copies are what drift.

**The cell is called `schemas`, and it was called `declare` until 4.5.0**
([#548](https://github.com/mmeyerlein/meclaw/issues/548)). One lane, one cell name: the
tools hive answers `in_schemas` with an occupant called `schemas`, this core answered the
same lane with a cell called `declare`, and the two scripts differ in their schema table and
almost nowhere else. Two words for one job is cheap on the day it is written and expensive
the moment a third hive picks a third word -- every `override_params` path, every mutation
that adds a schema and every reader following the declaration round had to know which word
this particular hive chose. `schemas` survived because it names what comes back and it
matches the lane. Nothing else moved: the lane pair, the body, the answer shape and the
`required_drains` entry are what they were.

**Why the core and not a tools hive.** A tools hive declares the cells it contains, and this
core is not one of them. Before 4.4.0 the `consult_cogny` schema was typed by hand into every
calling brain's `system.tools`; when GH #464 replaced typed menus with asked-for ones, every
caller stopped typing and the schema had no owner left at all -- so a grown assistant offered
its model a menu without the one tool the core exists for. Whoever is reached declares
themselves.

**The hive path is the address, and the lanes are the contract.** Since the seal
(GH #228) `params.ports` is empty: no edge from outside may name `./cogny/collector` or
any other cell in here, and every port above and every lane below meets at `./cogny`
itself. Which cells sit behind that path may be rearranged in a version bump without a
caller noticing -- that is what the seal bought. What may NOT change silently is the set
of lanes: dropping one or renaming one is a breaking change to every parent that wired it,
a CHANGELOG Breaking entry and a new major version, never a patch. Adding a lane nothing
ever promised is additive and takes the minor digit; giving a hive that shipped sealed the
contract it already implied is a repair and takes the third, which is what 3.0.1 was.

### Per-instance lanes (not ports of this template)

**Tools stay outside.** The tool set is the per-agent choice, so the composite carries no
tool cells and no map of them. Wiring a tool is one edge pair:

```json
{"from": "./cogny", "to": "./search",
 "condition": "has(hop.route) && hop.route == 'tool' && has(hop.tool_name) && hop.tool_name == 'web_search'"},
{"from": "./search", "to": "./cogny",
 "modifier": {"set_hop": {"route": "'in_tool'"}}}
```

`hop.route == 'tool'` is the lane; the `has()` guards are not decoration either, because
the `calls`, `result` and `answer` emissions carry no `tool_name` at all and an unguarded
comparison **errors** in CEL, which skips the edge with a log line per lane per message.

**One tool name is reserved inside this composite and never leaves**: `thread_recall`
(GH #451), which reads the collector's own slate -- a table in that cell's own `cell.db`,
which no other cell may read. `memory_recall` stood beside it from 4.4.0 to 4.6.1 and left
with 5.0.0 ([#552](https://github.com/mmeyerlein/meclaw/issues/552)): a memory belongs to the
MEMBER, and the hive that enforces the rules a recall obeys is the hive that declares it.
The reserved name is served by this hive's own collector and costs exactly one ordinary
`./dispatcher -> ./collector` edge -- which is what the **guarded default edge** of `4.0.2`
was designed for ([#283](https://github.com/mmeyerlein/meclaw/issues/283), ruling Q1). The
exit is `{"from": "./dispatcher", "to": ".", "default": true, "condition": "has(hop.route)
&& hop.route == 'tool'"}`, consulted only when no ordinary edge out of `./dispatcher` fired
for the message, so a reserved name silences it by claiming the message and no exclusion
term is written anywhere. `escalate_to_deep` was the third reserved name until 4.4.0 and
went with the lane it escalated to.

The guard on that default is not decoration: `./dispatcher` emits four sorts (`calls`,
`result`, `answer`, `tool`) and default suppression is **sender-wide**, so an unguarded
default would try to carry `calls`/`result`/`answer` outward whenever nothing ordinary
fired for them. For the same reason there is **no unconditional tee** from `./dispatcher`
here -- five out-edges, each conditioned on its own lane. A tee added later, at
`./cogny/dispatcher`, would silence this default for every tool call and the parent's tool
cells would go dark.

**The memory leg is the second pair**, and since 4.4.0 it carries a TOOL call rather than
an ambient one:

```json
{"from": "./cogny", "to": "<member>/memory",
 "condition": "has(hop.route) && hop.route == 'recall'",
 "modifier": {"set_hop": {"route": "'in_query'"},
              "set_context": {"recall_query": "hop.recall_query",
                              "memory_tier": "hop.memory_tier",
                              "memory_call_id": "hop.memory_call_id",
                              "recall_window_from": "hop.recall_window_from",
                              "recall_window_to": "hop.recall_window_to"}}},
{"from": "<member>/memory", "to": "./cogny",
 "condition": "has(hop.route) && hop.route == 'bundle'",
 "modifier": {"set_hop": {"route": "'in_bundle'"}}}
```

**The recall port has ONE meaning since 5.0.0** ([#552](https://github.com/mmeyerlein/meclaw/issues/552)):
the ambient leg of a turn. It carried two until then, told apart by a `memory_call_id` the
request carried out -- set meant "this answers a `memory_recall` call", empty meant "this is
the turn's own memory". Both halves of the tool live in the memory hive now, so the key is
gone from this road and the parent carries no correlation on it at all. Every key that IS on
it is always present and empty rather than absent, because a missing hop key makes the
promoting CEL modifier fail and a failed modifier skips the edge.

`params.memory_tier` on `./collector/assemble` stays **empty** here: this core takes no
ambient bundle. It asks on purpose, and since 5.0.0 it asks the member's memory the way it
asks any other tool. The tier of such a call is read off the hive's own `tool` cell and never
off the model's arguments -- a tier is a cost decision of the tree, not something a prompt
gets to raise.

**The error drain is one edge**, because the brain is normalised onto the lane by
the exit edge itself rather than by a cell:

```json
{"from": "./cogny", "to": "<parent>/drain",
 "condition": "has(hop.route) && hop.route == 'error'"}
```

**Wire it.** `talky` says the same thing about its own error lane and for the same reason:
undrained, a failed inference dead-letters as `no_route` and nothing upstream ever learns
the consultation died.

**What this composite still does NOT declare**, and it is a limit rather than an omission:

| wanted | state |
|---|---|
| housekeeping (`in_prune`, `in_round_sweep`) | not declared |
| a normalising `errors` cell | R-CG-2 names "collector + dispatcher + llm, and nothing else"; an `errors` cell is not among them, so the brain is put on the lane by the exit edge's `set_hop.route` instead. That is enough to make the failure reachable; it is not enough to give it a body a reader can grep, which is what `talky/errors` adds |

**The one tool this composite serves ITSELF is wired since 4.1.0.** `thread_recall` landed
with the curator, as a `./dispatcher -> ./collector` edge on the tool name in **this
template's** `params.graph`, because a parent cannot draw it: the seal refuses an outside
edge naming `./cogny/dispatcher`. It never touched the tool exit, which is exactly what the
guarded default of `4.0.2` promised a reserved name would cost. That is the shape `talky`
ships for its own served tool ([#55](https://github.com/mmeyerlein/meclaw/issues/55)).
`memory_recall` was the second from 4.4.0 to 4.6.1 and is an ORDINARY name since 5.0.0
([#552](https://github.com/mmeyerlein/meclaw/issues/552)) -- one edge deleted, and the call
leaves on the default like every other.

The tool SCHEMAS of the tools a PARENT wires are a different thing again: they live in the
brain's `system.tools`, asked for on the `schemas` lane since 4.3.0 and written there by the
collector. **This composite declares none of them.** What it does declare, since 4.4.0, is
its OWN errand -- see [The core declares its own errand](#the-core-declares-its-own-errand-528).
That is the same rule read from the other side, and it is the rule `talky` stated for itself
([#55](https://github.com/mmeyerlein/meclaw/issues/55)): a tool the composite *implements* is
topology and ships with it, schema and edge together; a tool the parent wires is the agent.
`consult_cogny` is not a tool this core implements -- it is the door this core IS -- and the
answer is the same, for the sharper reason that nobody else can hold it.

## One brain (#528)

`1.1.0` split the seam in two, because a memory lookup arriving six seconds into a
twenty-two-second research answer waited **15.5 s** in an `llm` cell's serial mailbox for a
call it finished in 2.5 s. The measurement was right and the topology answered the wrong
question: the lookup should never have been an errand at all. It is the **conversation
surface** that holds the window and the person, and a `memory_recall` tool answers in
one call what an advisor round trip answered in two hops and a queue.

So `4.4.0` takes the split out and moves the class boundary into the one place a caller
reads before it decides -- the `consult_cogny` description this core now hands out itself:

```
        collector ══(brain, iter < 12, restore_ttl)══> brain
                                                         │
                     dispatcher <──(stop | tool_calls)───┘
                            │
     thread_recall ─────────┴──> collector
```

**One class, one lane, one mailbox.** Synthesis, a development over time, multi-step work
and research come here; everything a question's own asker can look up stays there. A
misfiled errand no longer costs a lane change and an extra assembly -- it costs a consult
that should not have been made, which is visible in the transcript and fixable in one
sentence.

`hop.consult_eta` -- the asker's own coarse duration guess, GH #123 -- stays
**observe-only** and routes nothing. It is now a field of the declared schema (`eta`), which
is the first time anything says out loud who fills it in and what for: it is said to the
person in the same reply, so nobody waits without knowing why.

What went with the lane: `brain_fast` and its `brevity` seed, `ctx.model_fast`,
`context.consult_class` on the seam edge, the `ask_memory` ingress edge, and
`escalate_to_deep` -- the reserved tool name, its edge, and the paragraph that told every
instance to put it in its dispatcher's handoff list. **Drop it from that list**: a name in the
handoff list that no cell serves is a call the dispatcher marks as answered-elsewhere and
nothing ever answers.

## The internal wiring, edge by edge

Twenty edges in this hive's `params.graph`, plus the four the sealed collector brings
with it -- those four are its own door and store edges and are neither drawn nor wireable
from here. Every edge below names `collector` by its HIVE path; the lane in the third
column is what the door behind it reads:

```
collector ==(brain, iter < 12, restore_ttl)==========> brain       <- THE SEAM
collector --(pack)-----------------------------------> brain       <- THE DOOR IN
                                                                      THE WALL, #458
collector --(menu)-----------------------------------> brain       <- the answered
                                                                      menu, #464
brain      --(stop | tool_calls)--> dispatcher
brain      --(length)-------------> collector  in_answer

dispatcher --(calls)---> collector  in_calls
dispatcher --(result)--> collector  in_tool
dispatcher --(answer)--> collector  in_answer     -> and out of the advice port
dispatcher --(tool_name == thread_recall)--> collector  in_thread_call

.          --(in_turn)-----------> collector         THE DOORS
.          --(in_tool|in_bundle|in_pack|in_menu)-> collector
.          --(in_schemas)--------> schemas           <- #528
collector  --(answer)-----------> .                  THE EXITS
collector  --(recall)-----------> .
collector  --(pack_ack)---------> .
collector  --(schemas)----------> .
schemas    --(operation == schemas)--> .  route := 'tool_schemas'
dispatcher ==(tool, DEFAULT)==============> .
brain      --(error|content_filter)--> .  route := 'error'
```

**The seam is one edge again since 4.4.0.** Until then it was two complementary
conditions, and complementary was a correctness property rather than tidiness: fan-out
copies a message to *every* matching edge, so two overlapping seam conditions would have run
both brains on one errand and answered twice. With one brain there is nothing to overlap.

**Two reserved tool names, and the exit is untouched by either.** The `==` on the exit marks
the **default** edge (`4.0.2`, [#283](https://github.com/mmeyerlein/meclaw/issues/283)): it is
consulted only after every ordinary edge out of `dispatcher` has declined, which is precisely
how `thread_recall` silences it without being named there. A per-instance
tool of either name would be swallowed by the lane that claims it.

**The loopback bound is an edge literal, on purpose.** `int(hop.iter) < 12` is a safety
belt, not the policy: the round is bounded by `max_iter`, which ends a runaway
round with a message on the `answer` lane instead of a silence. The edge number only has
to be larger. Env substitution does not reach edge conditions -- a `${VAR}` there would be
registered verbatim and fail to parse as CEL -- so raising it is a mutation:
`remove_edges` first, `add_edges` second, in **two** mutations.

**`max_iter` is a knob, and a thorough errand can reach it.** The shipped default is
`8` -- the collector's own, and generous for a question that takes two or three lookups.
A core told to research something in depth spends an iteration per search, and one that
reaches the bound does not fail: the seam leaves on `answer` with
`hop.round_capped == "1"` and, since `collector@3.5.0`, `hop.partial == "1"` beside it,
**carrying a named partial answer as its last turn** -- one sentence saying which bound it
hit, how many calls it made, which tools it called and the head of the last result
([#570](https://github.com/mmeyerlein/meclaw/issues/570)).

Until `cogny@4.6.1` it carried the raw end of the round instead -- the turn as it was
assembled, with whatever the tools had just returned. On the advice lane that is what the
asking voice receives, and a surface reads the LAST text of an answer, so the reply changed
shape on exactly the errands that were going best: a core that capped mid-search handed its
surface a raw `web_search` payload and the person was shown a search dump. Nothing was ever
lost -- the raw round is in the `round` table and `thread_recall` reaches it -- but what
reached a reader was not a sentence. Now it is.

An operator who runs this core on research-sized work still raises the knob per instance
(`override_params` on the `./collector` copy) rather than living with the cap. The default
stays `8`: a colony that has not measured its own rounds is better served by a bound that
ends a runaway early than by one that pays for it.

**`restore_ttl` sits on the seam, once per round.** `iter` counts brain answers, and a
bundle of fifteen calls is one answer, one iteration, one restore.

## Knobs

The collector's knobs are **params of `./collector`** (since `collector@1.2.0`):
they ship with their defaults inside the sub-unit copy and are retuned in the instantiated
tree, per core. Three of the dispatcher's are still `${VAR:-default}` env literals that travel into the
instance and bind **late**, at every read -- and therefore move every unit in the colony at
once. Its fourth, `interim`, is a param like the collector's, and this template sets it
([#539](https://github.com/mmeyerlein/meclaw/issues/539)).

| knob | where | default | unit |
|---|---|---|---|
| `window_turns` | param | `12` | collector -- newest errands entering the context |
| `window_bytes` | param | `8000` | collector -- byte cap over the window |
| `turn_chars` | param | `4000` | collector -- per-turn cap before the byte cap |
| `tool_chars` | param | `4000` | collector -- per-item cap on tool results |
| `round_bytes` | param | `16000` | collector -- byte cap over the whole tool round |
| `memory_chars` | param | `8000` | collector -- cap on the memory bundle |
| `max_iter` | param | `8` | collector -- **the loop bound**; at the cap the seam leaves on `answer` with `hop.round_capped == "1"`, `hop.partial == "1"` and a named partial answer as its last turn ([#570](https://github.com/mmeyerlein/meclaw/issues/570)). Raise it per instance for research-sized errands -- see above |
| `round_idle_ms` | param | `120000` | collector -- idle window of one tool round |
| `memory_tier` | param | `""` | collector -- the AMBIENT memory leg, and it stays **empty** at this template since 4.4.0: a problem solver asks on purpose. Setting it gives the core a bundle before it has read the question, and pays for it every consult |
| `memory_form` | param | `"readable"` | collector -- `readable` / `json` / `both` |
| `interim` | param | `""` | dispatcher -- **off at this template since 4.4.0** ([#539](https://github.com/mmeyerlein/meclaw/issues/539)). On (the shipped default, and what a channel voice keeps) a sentence standing next to a tool bundle leaves on the `answer` lane at once. This core has no channel, and its `answer` lane is the asking voice's advice lane, so such a sentence arrives as an advice nobody gave. Off it does not leave the dispatcher at all, and therefore does not enter this core's own window either -- a sentence nobody could hear was never said. The FINAL answer is untouched |
| `prune_after_ms` | param | `604800000` | collector -- age gate on the prune lane (7 d) |
| `turn_write` | param | `"1"` | collector -- per-turn episodes, **on by default** since GH #298. The write belongs at the **talky**, not here: set it to `"0"` at the core unless the core's own `turn_write` route is wired -- see below |
| `context_window` | param | `128000` | collector -- **the curator's budget in tokens**; `0`/empty = curation off. **Set at this template since `4.1.0`** (GH #451) -- see [The curator, live](#the-curator-live). This is the knob the core wants and the channel voice does not: a cogny is exactly the shape the curator was built for (few turns, huge tool results), a talky is the other one. The number is the window of the model this template DEFAULTS to (`openai/gpt-4o-mini`, 128k), not the largest window in the catalogue: an estimate here may only err low, because a budget set too high curates too late while one set too low merely curates a little early. Instantiating with a 200k model means raising it in the same mutation |
| `tools` | param | `["*"]` | collector -- the tool names this core **declares** it uses (GH #464). Set at this template since `4.3.0`, and set to EVERYTHING on purpose: a reasoning core should reach whatever its surface can, and a list typed here would be a second copy of a catalogue that drifts on the first tool added to the hive. The declarations are asked for on the `schemas` lane and written into the brain as durable `system.tools`, together with the one name the collector serves itself (`thread_recall`). `memory_recall` reaches this list the ordinary way since 5.0.0: `["*"]` asks every answerer the level wired, the member's memory among them ([#552](https://github.com/mmeyerlein/meclaw/issues/552)) |
| `curate_soft` / `curate_hard` | param | `0.5` / `0.75` | collector -- the working mark and the emergency mark, as fractions of the budget |
| `keep_rounds` | param | `2` | collector -- newest tool iterations kept verbatim whatever the budget says |
| `recoverability` | param | `""` | collector -- what may be elided, declared per tool NAME (`lookup:repeatable,write:env`). Undeclared = `unique` = never elided. **Declare the core's own tools here**, because the core is where the large results are |
| `tool_menu` | param | `""` | collector -- the tool menu as the provider-native JSON array, if this core wants its DECLARATIONS curated too (GH #451). Empty (the shipped default) leaves the menu in `./brain`'s own `system.tools`, exactly where it has always been. Set it, and the collector owns the slot: the declarations count towards the budget and the ones nobody called for `keep_rounds` iterations are stubbed to name + one line. It is per instance for the same reason the tool cells are -- this composite ships no tool set |
| `tool_desc_chars` | param | `200` | collector -- how much of a stubbed declaration's description survives |
| `curate_slot_chars` | param | `2000` | collector -- size above which a `system.*` slot of the collector's OWN making is cut; the protected families are never candidates |
| `curate_budget_line` | param | `"1"` | collector -- the deterministic remaining-budget sentence in `system.budget`; `""`/`"0"` sends the leaf empty |
| `thread_recall` | param | `"1"` | collector -- the `thread_recall` tool. **The edge is drawn since `4.1.0`**: `./dispatcher -> ./collector` on `hop.route == 'tool' && hop.tool_name == 'thread_recall'` with `set_hop {"route": "'in_thread_call'"}`, in this template's own `params.graph`, because a parent cannot draw an edge into a sealed sub-unit. It is exactly the one ordinary edge the guarded default exit of `4.0.2` was designed to cost for a second reserved name, and nothing else changed. It landed together with `context_window` and not before it: until the edge exists every stub the curator leaves is a dead end, and a dead end is worse than no stub |
| `thread_recall_budget` | param | `0.2` | collector -- share of the budget one turn's recalls may spend; over it the call is refused, never truncated |
| `max_calls` | param | `16` | cogny/dispatcher -- per-answer call budget |
| `async_tools` | param | `""` | cogny/dispatcher -- the core's OWN async tools, as a JSON array or one comma-separated string. The `consult_cogny` declaration belongs on the **asking** side, and since `dispatcher@1.2.0` it can stay there: the knob is a param of each dispatcher cell (GH #138), so the surface's list and this core's list are two statements instead of one shared key |
| `handoff_tools` | param | `""` | cogny/dispatcher -- async tools whose call ends the TURN because the answer comes from a later one. This core needs **none** since 4.4.0: `escalate_to_deep` is gone, and `consult_cogny` belongs on the asking side, where an advisor's answer arrives as its own turn. A name in this list that no cell serves is a call the dispatcher marks as answered-elsewhere and nothing ever answers |

**There is no `env` column above any more.** Since `dispatcher@1.2.0` the last
three knobs of a cogny tree moved onto `params` with the rest
([#138](https://github.com/mmeyerlein/meclaw/issues/138)); what is left in `.env`
is the provider lane -- the API key, the endpoint, the model id. Each row names
the CELL its knob belongs to, because that is what an `override_params` entry
addresses (GH #140).

**The sharp edge is gone (1.3.0).** Until `collector@1.1.0` every collector knob was a
colony-global env name, and because an env key is colony-global by construction, a `cogny` and
a `talky` in the same colony read the *same* `COLLECTOR_*` keys. R-CG-1 moves the memory leg to the core --
but setting `COLLECTOR_MEMORY_TIER` in `.env` turned it on at *every* collector in the tree,
including talkies whose `recall` port is not wired. The two ways out were "wire the talkies'
recall port too and pay for the extra leg" and "`override_params` on
`…/assemble.params.script_inline`", which was a fork of the script that the byte pin of the
day did not cover.

Now the knob is set where it belongs, and the sub-unit stays a reference to the standalone
`collector`:

```json
{"op": "instantiate", "template": "cogny@5.0.0", "at": "/cores/deep",
 "override_params": {"collector/assemble": {"context_window": 200000,
                                            "recoverability": "lookup:repeatable,write:env"}}}
```

The key is `collector/assemble`, not `collector`. Since
[#140](https://github.com/mmeyerlein/meclaw/issues/140) an `override_params` key is a
cell's path inside the template, and `collector` is a valid one -- it is the sealed
sub-unit's HIVE. A hive reads only `graph`, `ports` and `contract`, so the validator
accepts the key, nothing consumes the params, and the core comes up configured as if the
override had never been written. The knobs live one level down, on the `code` cell behind
the door.

**The curator knobs sat on the same edge and are now on the same footing.** A cogny core
wants `context_window` set -- it is the topology the curator exists for -- while a talky in
the same colony wants nothing of the kind. Set it at the core's `collector` and the
talky is untouched.

### The curator, live

Until `4.1.0` `context_window` was `0` everywhere, and `0` means curation **off**. The
curator was shipped, tested, documented -- and dark in every composite in the library,
including the one it was designed for. It ships **on** here now (GH
[#451](https://github.com/mmeyerlein/meclaw/issues/451)), and two things landed with it,
neither of them optional:

* **The budget is the shipped model's window, not the biggest one on the market.**
  `128000` is `openai/gpt-4o-mini`, which is what `ctx.model` defaults to in
  `./brain`'s contract. The estimate inside the cell is already a deliberate lower bound,
  and a budget set too **high** would compound that in the one direction that hurts:
  curation that starts too late has to take more at once. Instantiating with a 200k model
  raises the number in the same mutation -- the override block above shows exactly where.
* **The way back is wired.** `./dispatcher -> ./collector` on `hop.tool_name ==
  'thread_recall'` keeps the recall inside the composite, so every stub the curator leaves
  can be redeemed. That edge could not come from a parent -- it crosses into a sealed
  sub-unit -- so it lives in this template's `params.graph`, and it is exactly the single
  ordinary edge the guarded default exit of `4.0.2` predicted a second reserved name would
  cost.

What the composite still does **not** decide is the tool set, and therefore not
`recoverability` and not `tool_menu` either. Both are per instance, for the same reason the
tool cells are: declare the core's own tools where the core is built. Without
`recoverability` every result is `unique` and the curator can only take `tool_call`
arguments -- correct, and much less than it could do.

**`turn_write` was the sharper of the two, and it is the clearest win.** The core sees only
the turns of a consultation, the talky sees the conversation -- so the per-turn write belongs
on the **talky's** collector, exactly where the close batch already goes. Colony-wide it also
fired at the core's collector, whose `turn_write` route is either unrouted (a dead letter per
consult turn) or, worse, wired to the same memory, where a second session's turns land as if
somebody had said them. Being a param, it is decided per collector -- and since GH #298 it
is decided in the other direction: the knob ships **on**, because it is the only path from a
conversation into an episodes table and an agent that ships with it off remembers nothing.
**A core whose `turn_write` route is unwired therefore needs `"turn_write": "0"` in its
`override_params`**, and that is the one knob this composite expects a parent to switch off
rather than on.

**`ctx.model` is the one instantiation-class knob** and it is strict: `add_nodes` without
it is rejected with `ctx_key_missing`. Two equally valid forms (session ruling 2026-08-15):
a **resolved literal** (the K-H2 builder convention -- the builder resolves `MODEL_<ROLE>`
from `.env` itself), or the `MODEL_<ROLE>` token verbatim so the cell re-resolves it at every
read.

- `ctx.model` -> `brain`. A **thinking** model: the shipped `external_timeout_ms` is 300 s
  and `message_timeout` 400 s, sized for a model that reasons. A fast channel model here
  wastes the split between the surface and the core.
- `ctx.model_fast` is **gone since 4.4.0** ([#528](https://github.com/mmeyerlein/meclaw/issues/528)),
  with the lane it fed. An instantiation that still passes it is not refused -- an unused ctx
  key is ignored -- but nothing reads it, and a level that still declares it is describing a
  cell that is not there.

**This template ships no system state at all since 4.4.0.** The one piece it used to ship was
`brain_fast/seed/system.jsonl`, a `brevity` leaf carrying the lookup lane's length discipline
and its escalation instruction; the lane is gone and so is the slot. Identity, instructions
and the tool menu are instance business, exactly as they always were -- and the tool menu is
asked for rather than seeded since 4.3.0.

**The TTL budget.** With `restore_ttl` on the seam and on both port edges the colony
default of 64 carries the loop: only ONE round has to fit the budget.

## Instantiating it

```bash
curl -s -X POST http://127.0.0.1:PORT/colony/mutations -H 'Content-Type: application/json' \
  -d '{"scope":"/main/agent",
       "ctx":{"model":"anthropic/claude-opus-5"},
       "diff":{
        "add_nodes":[{"name":"cogny","template":"cogny"}],
        "add_edges":[ ... the two port PAIRS plus the tool lanes, in the SAME mutation ... ]}}'
```

The composite comes up with five cells (plus two hive markers); the `store` and `llm`
cells report `active=true` + `NotYetSpawned`, which is the correct hot/cold form for a
stateful cell. Two things to have ready before the mutation:

1. **A `colony.db` whose three tables agree** (`registry`, `edges`, `hive_scopes`). A
   mutation that is REJECTED leaves a colony whose next boot panics (GH #89), so bring the
   edges in the same diff and check the table counts after any rejection.
2. **The core's identity**, either as `brain/seed/system.jsonl` (which only takes on a
   FRESH birth -- a `cell.db` that already exists means `Resumed` and an inert seed) or as
   a system update message. Neither is this template's business.

**The `identity` slot is a projection target.** The brain puts `identity` first in
`system_order` (`brain/config.json`), and that first
slot is where a person -- the caller, the agent itself -- is rendered into the prompt. An
`affinity` hive may push into it: one edge per subscribing cell on
`hop.route == 'answer' && hop.subscriber == '<that brain>'`, and every change to the record
reaches the brain as a `system.*` write and not as an inference (the recipe is in the
`affinity` template's own README, § Wiring `out_push` for a subscribing
brain). Nothing here configures it, and nothing here needs to: the lane is the parent's
business, exactly like the seed above -- and the subscriber key names an address and not a
hive, so a composite with two brains would have needed two edges. A brain with nobody pushing into `identity`
is not a broken brain -- `system_order` names the key it would render first, and a `system`
tree that does not carry the key is simply concatenated without it
(`crates/meclaw-cells/src/llm/translate.rs:56-60`); nothing declares it unbound. Since
[#285](https://github.com/mmeyerlein/meclaw/issues/285) a hive port may be declared as a
slot (`{"name": "...", "slot": true, "unbound": "park"}`), so a composite that means to bind
the lane later says so in its contract from birth instead of parking a placeholder at the
address.

One line of the CALLER's identity is this template's business, though, because it decides
how often this core is woken for nothing. The persona of the talky in front of it has to
carry the boundary verbatim — see
[`../talky/README.md`](../talky/README.md) § The sentence a memory-carrying persona has to
contain. Without it the front model consults the core for what it was handed a moment ago
(#150): the answer comes back correct, so nothing looks broken, and every such question
costs a consult round trip instead of a direct reply. A core that is asked what the window
already says is not a slow core — it is a persona that was never told where its own
knowledge ends.

## What it is not

- **Not a channel voice.** No proxy, no chat id, no tone. Who talks to the user is the
  talky's job; the core answers the talky.
- **Not a session.** Nothing here mints a `session_id` or closes a generation. The
  `session_id` that rides in on the port edge is the *channel's*, and it is what keeps one
  consultation inside the conversation that asked for it.
- **Not a memory.** The recall leg is optional and the hive it asks belongs to the member
  the agent works for, not to this template and not to the agent.
- **Not a persona.** Identity and core instructions live in the brain's `cell.db`, one
  writer per `system` path, and the tool menu is asked for rather than typed. The ONE
  schema this composite owns is its own errand (#528), which is the opposite of a persona:
  a caller cannot reach what nobody declared.
- **Not a classifier, and no longer classified either.** Until 4.4.0 an ingress edge lifted
  a tool name into `context.consult_class` and the seam chose a lane from it. There is one
  class and one lane now; nothing in here or above it picks between two.
- **Not an initiator.** In v1 the talky triggers and the core answers; cogny never opens a
  conversation of its own (R-CG-3).
- **Not in the turn hot path.** A consultation is not a second seam inside a chat turn
  (R-CG-1, explicitly not v1). Reasoning-upgrading a single turn is an `override_params`
  on the talky's brain, not a cogny job.

## The credential connect point (GH #560)

Since `cogny@4.6.0` the rim declares both halves of the **credential lane** and names
`./brain` as their connect point:

```json
"emits":  [{"route": "credential_request", "at": ["./brain"], "because": "…"}]
"accepts":[{"route": "in_sealed",          "at": ["./brain"], "because": "…"}]
```

`params.ports` stays literally `[]`. `at` is not a second kind of port: it is the
one opening this template pronounces about **itself**, for **one** named lane and
**one** address inside it (`docs/meclaw-overview.md` § *v-lanes*). What it buys is
that a member can wire this brain to the person's own `access` in **one** edge per
direction instead of a pass-through chain through three rims — and that neither
this level nor the generation above has to declare, forward or guard a lane it
takes no part in.

The brain accordingly ships `params.credential_grant_id` as the empty
string, which is no grant at all (GH #271): standalone this composite behaves
exactly as it did before and spends its `api_key`. Since 5.0.0 that empty string
is a LITERAL and not a `${COGNY_CREDENTIAL_GRANT_ID:-}` token
([#138](https://github.com/mmeyerlein/meclaw/issues/138), ruling R-0904-6): a
grant id is a reference, not material, and two generations in one colony present
different ones -- which an environment variable, being colony-wide, could not
say. Switching it over takes **two**
`override_params` keys and not one, because a cell asks for a credential only
while it holds none — and `params.api_key` counts as one:

```json
"override_params": {
  "brain": {"api_key": "", "credential_grant_id": "grant:…"}
}
```

Set the grant and leave the shipped `api_key: "${OPENROUTER_API_KEY}"` standing
and the cell never asks: it keeps spending the environment key and the lane
carries nothing, silently, because a model that answers looks like a model that
answers. With both keys set the model runs with **no credential in its config** —
the value arrives sealed against an ephemeral key it mints per ask, is opened in
its own task and is written nowhere. Both keys are **immutable** (`docs/cell-types.md`
§ `llm`), so this is a birth act: a generation grown without the empty `api_key`
is repaired by growing another one, not by a message. The recipe, both edges and
the two operator gestures that go with them are in `templates/member/README.md`
§ *The credential v-lanes*; `examples/vault-pilot/` is the small runnable version
of the same round.

## Pins

- `crates/meclaw-cells/tests/cogny_template.rs` -- the shipped template in a running
  colony against the mock OpenAI wire: a consult errand enters on the documented ingress,
  the core runs its OWN tool round, and the advice leaves on the return lane under the
  `consult_id` it was given. Since 4.4.0 also the three pins of #528: an `in_schemas`
  request comes back on `tool_schemas` carrying the `consult_cogny` schema with `question`
  and `context` both required; `ask_memory` and `escalate_to_deep` appear in no config or
  manifest of the template any more; and the core is one brain, `collector` + `dispatcher` + `brain` +
  `schemas` and nothing else.
- `crates/meclaw-colony/tests/gh277_composite_instantiation_is_byte_identical.rs` -- the
  two golden manifests over the instantiated tree (the sub-unit refs produce the same
  bytes the copies did) plus the stamp pin: a cell inside a referenced sub-unit carries
  its OWN template and names the composite above it.
- `crates/meclaw-cells/tests/talky_cogny_advisor.rs` -- the other half: the bilateral
  advisor connection end to end, from a talky's interim answer to the correlated
  follow-up in the channel.
- `crates/meclaw-cells/tests/gh539_a_core_without_a_channel_says_nothing_in_between.rs`
  -- the interim knob: off, a sentence beside a bundle that is waited for emits nothing on
  the answer lane while the calls travel unchanged; the final answer still leaves, exactly
  once; a sentence beside an async-non-handoff bundle still leaves unmarked (GH #378 is not
  rebuilt); the knob is on by default; and this template is the one that turns it off.
- The sub-units keep their own pins: `collector_window.rs`, `collector_colony.rs`,
  `dispatcher_template.rs`.

## Lanes

`params.ports` is empty (GH #228): the address is `./cogny` itself and what a caller wants
rides on `hop.route`.

| lane | direction | what travels |
|---|---|---|
| `in_turn` | in | an errand for this core -- ONE class since 4.4.0: synthesis, a development over time, multi-step work, research. The body is the `consult_cogny` tool_call turn, `question` and `context` both required. The lane's `accepts[].context` names **`session_id`**, and the ingress edge has to promote it: the core's memory tool asks about sessions ([#528](https://github.com/mmeyerlein/meclaw/issues/528)) |
| `in_tool` | in | one tool result, coming back from a tool cell the parent wired |
| `in_bundle` | in | a memory bundle, coming back from whatever keeps this agent's memory |
| `answer` | out | the core's answer, for whoever asked. Since `collector@2.1.1` a **third** sort travels here -- beside a real answer and a round that hit `max_iter` (`hop.round_capped`, and since `collector@3.5.0` `hop.partial == "1"` with a named partial answer as its last turn, #570) -- and it is marked `hop.degraded == "1"`: a turn that could not be assembled at all because the store refused a read or a write, with `hop.store_error` (the store's `error_code`) and `hop.store_operation` beside it ([#343](https://github.com/mmeyerlein/meclaw/issues/343)). It carries no `round_capped`, so an asker that renders an advice must branch on `degraded` -- without it a failure reads as a real answer |
| `tool` | out | a tool call for a cell the parent wired; `hop.tool_name` says which one |
| `recall` | out | a memory read the brain ASKED for, since 4.4.0: `hop.memory_call_id` names the tool call it belongs to and must come back on `in_bundle`, or the answer is filed as a turn's memory leg and the round waits for a result that never comes |
| `error` | out | a failed inference on the brain. **Wire it** -- unwired it dead-letters, loudly |
| `in_pack` | in | a durable `system.*` slot for the brain: `identity`, `persona`, `handover` or `instructions`, and nothing else. **Paired**: see `pack_ack`. Since 4.2.0 |
| `pack_ack` | out | the receipt `in_pack` answers with -- ONE per pack, not one per brain: `hop.pack_owner`, `hop.pack_slots`, `hop.error_code` (empty, `slot_unknown` or `pack_empty`), `hop.pack_unknown`. Since 4.2.0 |
| `schemas` | out | the tool names this core declares it uses (`{"tools": ["*"]}` as shipped), for a tools hive's `in_schemas` door. It leaves on a TICK, not per turn. **Paired**: see `in_menu`. Since 4.3.0 |
| `in_menu` | in | their declarations coming back, plus the names that hive had nothing under. They are written into the brain as durable `system.tools`. Since 4.3.0 |
| `in_schemas` | in | somebody asking what THIS core's errand looks like: `{"tools": ["consult_cogny"]}` or `["*"]`. **Paired**: see `tool_schemas`. Since 4.4.0 |
| `tool_schemas` | out | the `consult_cogny` schema, provider-neutral, in the tools hive's own answer shape. Since 4.4.0 |

**The door in the wall (`in_pack`, GH #458).** This composite is sealed, and until 4.2.0
that seal was complete in a way nobody had meant it to be: an edge naming `./brain` is
refused with `hive_port_boundary`, the only path from outside runs
through `./collector`, and the collector drops `system.*` on every lane that could have
carried one. So a shipped core had no entrance for its own identity, and `affinity` could
push into nowhere. `in_pack` is that entrance. It reached BOTH brains until 4.4.0, because
they were two lanes of one agent and a core whose thinking lane knew who it was while its
lookup lane did not would have answered as two different people; there is one brain now and
the lane is one edge. It still answers **once**, before whatever fan-out there is, so a
caller counts packs and not cores. What may be written is the
closed list `identity` / `persona` / `handover` / `instructions`, a subset of the
collector's `SYS_KEEP`; an unknown slot refuses the whole pack rather than writing the half
it understood. The charter is on that list since GH #488, which measured what holding it
out cost: nothing else exported it and no template seeded it, so a rebuilt core came up
with an empty charter and answered as the vendor's default assistant. It is guarded by the
edge that stamps the lane -- drawn only where a brain may draw its own push edge, from a
source whose single writer is `affinity`'s audited gate -- and not by being unwritable. The
owner comes off `envelope.reply_to`, never out of the body. The full account of the lane,
its two body shapes and the mutation that opens it lives in
[`templates/talky/README.md`](../talky/README.md) § "The door in the wall"; everything
there holds here without exception.

**The menu is asked for, not typed (`schemas` / `in_menu`, GH #464).** Since 4.3.0 this core
does not carry a tool menu either. `./collector`'s `params.tools` names what it uses and the
schemas behind those names are asked for, on a tick, and written into the brain as durable
`system.tools` -- the same door the pack takes, and durable for the same reason. This core
declares `["*"]`, and that is a decision rather than a default: a reasoning core should reach
whatever its surface can, and a list typed here would be a second copy of a catalogue that
drifts on the first tool added to the hive. The lane pair is `schemas` out and `in_menu`
back, two edges and never one; a name that hive has nothing under comes back in
`hop.menu_unknown` and as a warn line rather than as a silence. The full account lives in
[`templates/talky/README.md`](../talky/README.md) § "The menu is asked for" and
[`templates/collector/README.md`](../collector/README.md) § "The menu is asked for";
everything there holds here without exception. Since 4.1.0 the menu the collector writes
also carries the tool it answers ITSELF -- `thread_recall`, routed by name inside this
composite and not a tool any hive has a declaration for. That is the `collector`'s own doing
since GH #512, and the switch that decides it is the one this template sets:
`thread_recall`. `memory_recall` was on that list from 4.4.0 to 4.6.1 and is on the ordinary
one since 5.0.0: the member's memory declares it, the `["*"]` above asks for it, and the
merge of GH #529 files it under a third answerer
([#552](https://github.com/mmeyerlein/meclaw/issues/552)).

**And this core answers a menu question of its own (`in_schemas` / `tool_schemas`,
[#528](https://github.com/mmeyerlein/meclaw/issues/528)).** The two lane pairs point in
opposite directions and must not be confused: `schemas` / `in_menu` is this core ASKING a
tools hive what it may call, and `in_schemas` / `tool_schemas` is this core ANSWERING what
IT may be asked. The full account is above, under
[The core declares its own errand](#the-core-declares-its-own-errand-528).

Which cell takes the question, which one calls the tools and which one produces the
answer is this template's business and may change without a caller noticing. The brain
is put on the `error` lane by the exit edge's own `set_hop.route`,
because this composite carries no `errors` cell of its own -- see
[Per-instance lanes](#per-instance-lanes-not-ports-of-this-template).
