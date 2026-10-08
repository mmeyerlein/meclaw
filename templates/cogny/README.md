# `cogny@5.9.3`

The agent core as one template. Seven units under one hive: [`collector`](../collector/),
[`curator`](../curator/) and [`dispatcher`](../dispatcher/) -- each carrying its
template's own name -- plus ONE `llm` `brain` and three `code` cells: `splitter`, which cuts
the sidecar block out of the brain's answer for the curator (a copy of the talky's, GH #892),
`schemas`, which hands out the declarations of the consult contract, and `ask`, which turns
the core's question back into one sentence (GH #894). No new cell type, no Rust.

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
owns the thinking. The core therefore carries no session keeper and no
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

- **The seam, already bounded, and its own.** Since `5.2.0` the curator hands the window to
  the brain over ONE edge (GH #889) carrying the iteration counter and `restore_ttl` -- a second
  copy of the mechanism the talky has, with its own bound, because a consultation is a
  longer round than a chat turn.
- **A tool round that only needs its tools.** `brain -> dispatcher -> (your tools) ->
  collector -> curator -> brain` is pre-wired except for the one lane that is genuinely
  per-instance: which cell answers to `web_search`. Adding a tool is one edge pair.
- **A consultation that looks like a turn.** The errand arrives on the collector's
  `in_turn` lane and is filed as the turn it is: the talky IS the core's user. Nothing in
  here knows that its user is a machine.
- **An answer that is an event.** The advice leaves on the ordinary `answer` route and
  becomes the asking talky's `in_advice` event. Since
  [#894](https://github.com/mmeyerlein/meclaw/issues/894) a *question back* has a lane of
  its own, `ask` -- see [The core asks back](#the-core-asks-back-894).
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
  voices' half of its consult contract -- `consult_cogny` and, since #894,
  `reply_to_consult` -- in the tools hive's own shape, on `tool_schemas`. Whoever is
  reached declares themselves -- see [The core declares its own errand](#the-core-declares-its-own-errand-528).

## Cells

| path | type | from |
|---|---|---|
| `collector/{assemble,window}` | `code`, `store` | `collector` **(sealed)** |
| `curator/{intake,policy,writer,ledger,summarizer,clock,schemas,history,push,handover}` | `code`, `code`, `code`, `store`, `llm`, `timer`, `code`, `code`, `code`, `code` | `curator` **(sealed)**, since `5.2.0`; `schemas`, `history`, `push` and `handover` since `5.3.0` (GH #892, GH #893, GH #895, GH #896) |
| `dispatcher` | `code` | `dispatcher` (a single-cell template) |
| `brain` | `llm` | this template -- the one inference |
| `splitter` | `code` | this template -- a copy of `talky/splitter`, params and contract byte for byte (GH #892) |
| `schemas` | `code` | this template -- the consult contract's declarations (4.4.0; named `declare` until 4.5.0; two audiences since #894) |
| `ask` | `code` | this template -- the core's question back, `the core asks: <question>` on `ask` (#894) |

**The braces are an inventory, not an address list.** `collector` and `curator` declare
`params.ports: []`, so `./collector` and `./curator` are the only addresses an edge from
outside may name and `./collector/assemble` is refused with `hive_port_boundary`; which
cell inside takes the message is decided by the `in_` lane the edge sets.

### How the sub-units are referenced: by name and version (GH #277)

The three sub-units are **references**, not copies. Each of the three directories holds one
`config.json` and nothing else:

```json
{"cell": {"type": "ref", "template": "collector@5.1.1"},
 "override_params": {"assemble": {"tools": ["*"]}}}
```

```json
{"cell": {"type": "ref", "template": "curator@1.11.3"},
 "override_params": {"writer": {"turn_write": "0"}}}
```

**`5.3.1` moves the `curator` pin to 1.1.1** ([#904](https://github.com/mmeyerlein/meclaw/issues/904)): the curator's cache clock keeps one
standing order instead of adding one per call. Only the pin moved, so it is the third digit.

**`5.3.0` cuts its own block and can ask back** ([#892](https://github.com/mmeyerlein/meclaw/issues/892), [#893](https://github.com/mmeyerlein/meclaw/issues/893), [#894](https://github.com/mmeyerlein/meclaw/issues/894), [#895](https://github.com/mmeyerlein/meclaw/issues/895)). A splitter
like talky's stands between `brain` and `dispatcher` and hands every section to the curator, which
runs as role `consult`; the `history_*` tools are answered inside; the new cell `./ask` turns an
`ask_requester` call into the question that leaves on the new route `ask`, and the collector runs
with `defer_turns` `"0"`, so an errand or a reply that arrives during an open tool round opens a
round of its own; a recall question, where an instance sets a memory tier, goes through the
curator. It moves the `curator` pin to 1.1.0 and the `collector` pin to 5.1.0. A route joined the
boundary, so it is the second digit.

**`5.2.0` references `curator`** and moves the window, the curation and the identity pack
from the collector to it ([#889](https://github.com/mmeyerlein/meclaw/issues/889)). No lane of the boundary moved; the
`in_model` door now also reaches the curator's summarizer, and `brain` declares every hop key
the `llm` cell writes into an answer, the cache keys among them
([#890](https://github.com/mmeyerlein/meclaw/issues/890)), so it is the second digit.

**`5.1.4` declares `hop.model` on `brain`, and nothing else** ([#886](https://github.com/mmeyerlein/meclaw/issues/886)). The cell always wrote the
model the provider served into the header; the contract now says so, in the one wording the library
uses. The third digit.

**`5.1.3` moves both pins, `collector` to 4.4.1 and `dispatcher` to 1.2.2, and nothing else**
([#871](https://github.com/mmeyerlein/meclaw/issues/871)). That collector keeps the block a
splitter cut out of an answer and shows it with that answer in every later window, and that
dispatcher passes the block on to it. This core has no splitter, so no answer brings a block
along and its window is what it was. Only the pins move, which is the third digit.

**`5.1.0` moves both pins, `collector` to 4.4.0 and `dispatcher` to 1.2.1**
([#843](https://github.com/mmeyerlein/meclaw/issues/843), [#842](https://github.com/mmeyerlein/meclaw/issues/842)):
the `length` edge above still goes straight to the collector, because a core has no
splitter, and the answer it becomes now carries `hop.finish_reason = 'length'` and
`hop.truncated = "1"` -- whoever asked can tell a cut advice from a finished one. It also
opens the door `in_model` ([#855](https://github.com/mmeyerlein/meclaw/issues/855)): a model
package the colony's `llm-registry` pushes goes straight to `./brain`, past the collector, and
nothing answers it (see the port table below).

**`5.0.1` moved that pin and nothing else** ([#606](https://github.com/mmeyerlein/meclaw/issues/606)).
The `collector` composes the block contract it asks a brain for out of the sections its
answerers OFFER, instead of carrying one as a literal. This core is unaffected in substance: it
has no `splitter`, so it asks for no block, offers none and ignores any that is offered to it --
a section describing a fence nobody would cut.

**`5.0.3` moves it once more, and names its app**
([#728](https://github.com/mmeyerlein/meclaw/issues/728)). The collector at `4.2.1` writes a
`depart` row for a handed call and keys the round of a late answer on the member's turn; this
core hands nothing over, so what it does is unchanged. The same version gives `brain` the
OpenRouter app attribution (`http_referer` / `x_title`, overridable by `OPENROUTER_HTTP_REFERER` /
`OPENROUTER_X_TITLE`, the form the memory-hive cells use). Since the same fix the assistant's
consult edges drop `context.turn_id` on the way in, so each consult is a round of this core
with an id of its own.

**`5.0.4` moves it to `4.3.0`, and nothing else**
([#834](https://github.com/mmeyerlein/meclaw/issues/834)). That collector can brief the
member's record about the counterpart of a turn; the knob that switches it on,
`brief_slots`, stays empty here, because only a surface opens a turn with a counterpart and
this core is consulted, never addressed.

**`5.0.2` moves it again, and nothing else**
([#784](https://github.com/mmeyerlein/meclaw/issues/784), [#794](https://github.com/mmeyerlein/meclaw/issues/794)).
The version the ref block above names is the one that ships; what the collector grew since
`4.1.0` is the `in_delegation` lane and the `advise` assembly mode of a duplex call -- a voice
model that talks to the caller itself and is advised from behind.
This core's rim declares neither, so again nothing it does changes. A reference resolves
EXACTLY, so the pin has to move with the sub-unit or this template stops instantiating; that
is the whole of both numbers, and it is why each of them is a third digit.


The collector's curator that `4.1.0` switched on here moved to `curator` with `5.2.0`
(GH #889) -- see [The curator, live](#the-curator-live) below.

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
person waiting. `thread_recall` (GH #451) left with `5.2.0`, together with the round table it
read (GH #889).

At instantiation the referenced template's tree takes that position, so the instance is
byte-for-byte the tree the copies used to produce -- and every cell inside it now records
the template it really came from: `collector/assemble` is stamped with the `collector` version it was grown from, with
`cogny@5.9.3` above it in its provenance chain.

**The library has to carry all three.** A reference resolves against the colony's template
registry, so `collector`, `curator` and `dispatcher` have to sit in the same `templates/` directory
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
| advice exit | `./cogny` | out | `hop.route == 'answer'` -- the advice **or** a store refusal marked `hop.degraded` (see Lanes); since #894 a question back leaves on its own lane `ask` (see [The core asks back](#the-core-asks-back-894)) |
| declaration ingress | `./cogny` | in | `in_schemas` with `{"tools": [...]}` -- what does your errand look like? |
| declaration exit | `./cogny` | out | `hop.route == 'tool_schemas'` -- the voices' declarations (`consult_cogny`, `reply_to_consult`), provider-neutral |

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
  *fresh* consult is named by the call that opened it; a follow-up passes the id of the
  consultation it continues, and so does the reply to a question the core asked back
  (`reply_to_consult`, #894) -- the dispatcher decides which, and all arrive here as the
  same key.
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

**What a caller sends** is `{"tools": ["consult_cogny", "reply_to_consult"]}`, or `["*"]`
for everything this core declares to a voice, which is two schemas (#894). **What comes back** on `tool_schemas` is
`schemas[]` / `unknown[]` / `messages[]` in the body and `operation` / `schema_count` /
`unknown_count` / `error_code` (`tool_unknown`, `tools_missing`) on the hop -- byte for
byte the tools hive's answer shape, and provider-neutral for the same reason: wrapping the
envelope is the caller's job, because the caller is the one that knows its provider.

The errand's schema:

| field | type | |
|---|---|---|
| `question` | string | **required** -- the one thing the core has to answer |
| `context` | string | **required** -- everything the core needs: what the person wants, what was already said, what is excluded |
| `eta` | string | optional -- the asker's own coarse guess, said to the person in the same reply |
| `consult_id` | string | optional -- a follow-up: the id of the earlier consultation this one continues (#894) |

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

### The core asks back (#894)

A consultation is a complete order (the `consult_cogny` description names its five parts:
the goal, the facts already known, the constraints, the form of the answer and how long it
may be, in words -- nothing on this road cuts an answer). When it is still not enough, the
core calls **`ask_requester`** with one question. It is a handoff in `./dispatcher`, so the
core's turn ends; `./ask` sends the question out on the `ask` lane as one sentence,
`the core asks: <question>`, under `context.consult_id` -- the consult's id, which the
asker's departure row (#728) is written under. The asker answers with **`reply_to_consult`**
(`consult_id`, `answer`), which arrives here as the next errand on `in_turn`, with the
consultation still in the curator's window. `ask_requester` is offered to the core only and
`reply_to_consult` to a voice only: the core asks its own `./schemas` on its menu tick
(stamped `hop.tool_caller` 'cogny', answered back into `./collector` on
`hop.audience == 'core'`), and a voice asks
at the rim. The level that holds a core draws `ask` to the asker's advice lane
(`templates/assistant/README.md` § The consult edges).

The question travels as the words the core wrote, never as a reference into its window:
`./ask` takes a short block id the core quoted out of it (`[#<12 hex>]`, bare `#` and twelve to sixteen hex digits, the full sixty-four-digit block
id, or the window's form of a released, shortened or expired block), because the asker
cannot read the core's wall and the question goes on to a person. The handoff leaves one
departure row in the core's own collector, under the `ask_requester` call's id; nothing
answers under that id (the reply comes back as an errand, not as an advice), so the row is
never shown and falls with every other departure after seven days (`DEPART_AFTER_MS`).

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

**One tool family is served inside since `5.3.0`: the model's own wall**
([#893](https://github.com/mmeyerlein/meclaw/issues/893)). `history_search`, `history_read`
and `history_outline` read `./curator`'s ledger, which no cell outside that hive may read, so
one ordinary edge takes every name that starts `history_` to the curator (`in_history_call`)
and one takes its `tool_result` to the collector (`in_tool`); the curator declares the three
in its menu answer, which `["*"]` asks for. They replace `thread_recall` (GH #451), which
read the collector's round table and left with it in `5.2.0`
([#889](https://github.com/mmeyerlein/meclaw/issues/889)), as `memory_recall` left with 5.0.0
([#552](https://github.com/mmeyerlein/meclaw/issues/552)). **One tool name is reserved since
#894: `ask_requester`**, held by the ordinary edge `./dispatcher -> ./ask` and sent out on
`ask` (see [The core asks back](#the-core-asks-back-894)). Every other name leaves on the
**guarded default edge** of `4.0.2`
([#283](https://github.com/mmeyerlein/meclaw/issues/283), ruling Q1), `{"from": "./dispatcher",
"to": ".", "default": true, "condition": "has(hop.route) && hop.route == 'tool'"}`, consulted
only when no ordinary edge out of `./dispatcher` fired for the message.

The guard on that default is not decoration: `./dispatcher` emits four sorts (`calls`,
`result`, `answer`, `tool`) and default suppression is **sender-wide**, so an unguarded
default would try to carry `calls`/`result`/`answer` outward whenever nothing ordinary
fired for them. For the same reason there is **no unconditional tee** from `./dispatcher`
here -- six out-edges, each conditioned on its own lane. A tee added later, at
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
| housekeeping (`in_round_sweep`; `in_prune` left the collector with `5.2.0`, GH #889) | not declared |
| a normalising `errors` cell | R-CG-2 names "collector + dispatcher + llm, and nothing else"; an `errors` cell is not among them, so the brain is put on the lane by the exit edge's `set_hop.route` instead. That is enough to make the failure reachable; it is not enough to give it a body a reader can grep, which is what `talky/errors` adds |

**The one tool this composite served ITSELF**, `thread_recall` (since 4.1.0), is gone since
`5.2.0`: the round table it read moved into `curator`'s ledger, which serves no tool
([#889](https://github.com/mmeyerlein/meclaw/issues/889)).

The tool SCHEMAS of the tools a PARENT wires are a different thing again: they live in the
brain's `system.tools`, asked for on the `schemas` lane since 4.3.0 and written there by
the collector (through `./curator` since 5.2.0). **This composite declares none of them.**
What it does declare, since 4.4.0, is its OWN errand -- see [The core declares its own
errand](#the-core-declares-its-own-errand-528). That is the same rule read from the other
side, and it is the rule `talky` stated for itself
([#55](https://github.com/mmeyerlein/meclaw/issues/55)): a tool the composite *implements*
is topology and ships with it, schema and edge together; a tool the parent wires is the
agent. `consult_cogny` is not a tool this core implements -- it is the door this core IS
-- and the answer is the same, for the sharper reason that nobody else can hold it.

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
        curator ══(brain, iter < 20, restore_ttl)══> brain
                                                       │
                   dispatcher <──(stop | tool_calls)───┘
                          │
                          └──> collector ──(curate)──> curator
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

Forty-six edges in this hive's `params.graph`, plus the five the sealed collector brings
with it and those the sealed curator brings
([`../curator/README.md`](../curator/README.md)) -- those are their own door and store
edges and are neither drawn nor wireable from here. Every edge below names `collector` and
`curator` by their HIVE path; the lane in the third column is what the door behind it
reads:

```
collector  ==(curate, iter < 20, restore_ttl)==========> curator   in_curate  <- the whole
                                                                    round, #889
curator    ==(brain, iter < 20, restore_ttl)===========> brain       <- THE SEAM
collector  --(menu)------------------------------------> curator   in_slots   <- the answered
                                                                    menu, #464
collector  --(schemas)----------------------------------> curator   in_schemas <- the curator
                                                                    answers the menu too, #892
curator    --(tool_schemas, !refused_subscriber)---------> collector in_menu   <- tool_answerer
                                                                    'curator', #892
collector  --(recall)----------------------------------> curator   in_recall_ask  <- the
                                                                    question, #895
brain      --(stop | tool_calls | length)--> splitter   <- the sidecar cut, #892
brain      --(any answer, !refused_subscriber)--> curator  in_llm   <- the tap, #889
splitter   --(stop | tool_calls)--> dispatcher
splitter   --(length)-------------> collector  in_answer   <- the collector marks it truncated
splitter   --(sidecar)------------> curator    in_section  <- window, gap, memory, #892
splitter   --(sidecar)------------> .                      <- every other section, as a
                                                             talky's leaves, #916

dispatcher --(calls)---> collector  in_calls
dispatcher --(result)--> collector  in_tool
dispatcher --(answer)--> collector  in_answer     -> and out of the advice port
dispatcher --(tool, history_*)--> curator  in_history_call  <- the model's own wall, #893
curator    --(tool_result)-------> collector  in_tool

.          --(in_turn)-----------> collector         THE DOORS
.          --(in_tool|in_bundle|in_menu)-> collector   (a bundle with context.gap_ask: not)
.          --(in_bundle, context.gap_ask)--> curator  in_gap_bundle   <- #895
.          --(mutation_committed)-> collector
.          --(in_pack)-----------> curator           <- THE DOOR IN THE WALL, #458
.          --(in_pin)------------> curator           <- another hive's pin, #916
.          --(in_stats)----------> curator           <- an observer's question, #926
.          --(in_schemas)--------> schemas           <- #528
.          --(in_model)----------> brain             <- THE MODEL DOOR, #855
.          --(in_model, subscriber ends /curator/summarizer)--> curator   <- #889
collector  --(answer)-----------> .                  THE EXITS
curator    --(recall, !gap_ask)--> .                  <- the ask, question built, #895
curator    --(recall, gap_ask)---> .  context.gap_ask := hop.gap_ask   <- #895
curator    --(pack_ack)---------> .
curator    --(model_refused)----> .                  <- the summarizer's refused push, #889
curator    --(stats, !refused_subscriber)--> .       <- its answer, #926
collector  --(schemas)----------> .
schemas    --(operation == schemas, audience != 'core')--> .  route := 'tool_schemas'
collector  --(schemas)----------> schemas  route := 'in_schemas', hop.tool_caller := 'cogny'  <- #894
schemas    --(operation == schemas, audience == 'core')--> collector  route := 'in_menu'  <- #894
dispatcher --(tool, tool_name == 'ask_requester')--> ask   <- #894
ask        --(ask)--------------> .                  <- the question back, #894
dispatcher ==(tool, DEFAULT)==============> .
brain      --(error|content_filter, !refused_subscriber)--> .  route := 'error'
brain      --(has(refused_subscriber))--> .  route := 'model_refused'  <- a refused push, #863
```

**The seam is one edge again since 4.4.0.** Until then it was two complementary
conditions, and complementary was a correctness property rather than tidiness: fan-out
copies a message to *every* matching edge, so two overlapping seam conditions would have run
both brains on one errand and answered twice. With one brain there is nothing to overlap.

**The `==` on the exit marks the default edge** (`4.0.2`, [#283](https://github.com/mmeyerlein/meclaw/issues/283)): it is
consulted only after every ordinary edge out of `dispatcher` has declined. From `5.2.0`
(GH #889) no ordinary edge claimed a tool name; since #894 one does again,
`dispatcher -> ask` for `ask_requester`.

**The loopback bound is an edge literal, on purpose.** `int(hop.iter) < 20` is a safety
belt, not the policy: the round is bounded by `max_iter`, which ends a runaway
round with a message on the `answer` lane instead of a silence. The edge number only has
to be larger -- it was `12` until [#980](https://github.com/mmeyerlein/meclaw/issues/980),
which raised this template's `max_iter` to `16`, and a literal at or under the knob cuts the
round off without the capped answer (the talky keeps `12` over its `8`). Env substitution does not reach edge conditions -- a `${VAR}` there would be
registered verbatim and fail to parse as CEL -- so raising it is a mutation:
`remove_edges` first, `add_edges` second, in **two** mutations.

**`max_iter` is a knob, and a thorough errand can reach it.** The collector's own default
is `8`, generous for a question that takes two or three lookups; this template ships `16`
([#980](https://github.com/mmeyerlein/meclaw/issues/980)): a core that changes code reads,
writes, runs the tests, commits, exports, pushes and notes in one round, ten to thirteen
tool calls.
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
lost -- since `5.2.0` the raw round stands in `./curator`'s ledger (GH #889) -- but what
reached a reader was not a sentence. Now it is.

An operator who runs this core on research-sized work still raises the knob per instance
(`override_params` on the `./collector` copy) rather than living with the cap. The default
stays `8`: a colony that has not measured its own rounds is better served by a bound that
ends a runaway early than by one that pays for it.

**`restore_ttl` sits on two edges of the seam, each once per round.** `iter` counts brain
answers, and a bundle of fifteen calls is one answer, one iteration: the edge into the brain
restores once and the `curate` edge restores once.
The `curate` edge restores under the same bound (GH #919): the curator between
collector and brain spends about twenty routing decisions of ledger round trips per round,
and without it paid them out of what the legs before the round left.

Since `5.8.6` ([#1044](https://github.com/mmeyerlein/meclaw/issues/1044)) `./curator -> ./brain` lifts `hop.recall_input_soft` (the curator's
usable window, empty without a package) into the call's context, and a `memory_recall` of the
model leaves on the tool exit with it on the hop (empty on every other tool); every exit of the
composite clears the key.

Since `5.8.7` ([#1036](https://github.com/mmeyerlein/meclaw/issues/1036)) the splitter is again the talky
splitter byte for byte (contract 1.0.4): the sentence the model writes beside a tool call loses its
block as well, and a readable block rides beside the round as `sidecar_raw`. The calls and the order
of the turns stay as they came; no lane moved.

Since `5.9.0` ([#1079](https://github.com/mmeyerlein/meclaw/issues/1079)) the splitter is the talky
splitter byte for byte with the knob `sidecar_verify` (default `{}`, byte-identical output), its
edges are mirrored from talky's, and the curator pin moves on to the version that checks the quoted
fields of a section the knob names word for word before the section leaves. A new knob, so it is
the second digit.

## Knobs

The collector's knobs are **params of `./collector`** (since `collector@1.2.0`):
they ship with their defaults inside the sub-unit copy and are retuned in the instantiated
tree, per core. Three of the dispatcher's are still `${VAR:-default}` env literals that travel into the
instance and bind **late**, at every read -- and therefore move every unit in the colony at
once. Its fourth, `interim`, is a param like the collector's, and this template sets it
([#539](https://github.com/mmeyerlein/meclaw/issues/539)).

| knob | where | default | unit |
|---|---|---|---|
| `max_iter` | param | `16` (collector default `8`, [#980](https://github.com/mmeyerlein/meclaw/issues/980)) | collector -- **the loop bound**; at the cap the seam leaves on `answer` with `hop.round_capped == "1"`, `hop.partial == "1"` and a named partial answer as its last turn ([#570](https://github.com/mmeyerlein/meclaw/issues/570)). Raise it per instance for research-sized errands -- see above |
| `round_idle_ms` | param | `630000` (collector default `120000`, [#980](https://github.com/mmeyerlein/meclaw/issues/980)) | collector -- idle window of one tool round; one program run of a file space's projection may take `exec_timeout_ms` (600000), so the window is that plus 30 s |
| `memory_tier` | param | `""` | collector -- the AMBIENT memory leg, and it stays **empty** at this template since 4.4.0: a problem solver asks on purpose. Setting it gives the core a bundle before it has read the question, and pays for it every consult |
| `memory_form` | param | `"readable"` | collector -- `readable` / `json` / `both` |
| `interim` | param | `""` | dispatcher -- **off at this template since 4.4.0** ([#539](https://github.com/mmeyerlein/meclaw/issues/539)). On (the shipped default, and what a channel voice keeps) a sentence standing next to a tool bundle leaves on the `answer` lane at once. This core has no channel, and its `answer` lane is the asking voice's advice lane, so such a sentence arrives as an advice nobody gave. Off it does not leave the dispatcher at all, and therefore does not enter this core's own window either -- a sentence nobody could hear was never said. The FINAL answer is untouched |
| `turn_write` | param | `"0"` | curator/writer -- per-turn episodes, **off at this template since `5.2.0`** (GH #889): the write belongs at the **talky**, and at an unwired core it dead-lettered one message per consult turn. The curator's own default is `"1"` (GH #298) |
| `tools` | param | `["*"]` | collector -- the tool names this core **declares** it uses (GH #464). Set at this template since `4.3.0`, and set to EVERYTHING on purpose: a reasoning core should reach whatever its surface can, and a list typed here would be a second copy of a catalogue that drifts on the first tool added to the hive. The declarations are asked for on the `schemas` lane and written into the brain as durable `system.tools`. `memory_recall` reaches this list the ordinary way since 5.0.0: `["*"]` asks every answerer the level wired, the member's memory among them ([#552](https://github.com/mmeyerlein/meclaw/issues/552)) |
| `defer_turns` | param | `"0"` (#894) | collector -- a turn that lands in another errand's open tool round opens a round of its own instead of waiting for the next one: this core's session is the conversation that consults it, so a second errand or the asker's `reply_to_consult` would otherwise wait for a round nobody opens and come back under the other errand's `consult_id`. The collector's own default, `"1"`, is the telephone model of a channel voice |
| `role`, `keep_recent`, `compress_at`, `rebuild_to`, `horizon`, `tiers`, `keep_rounds`, `stub_tools_after`, `context_window` | param | `role` `"consult"` (GH #892), the rest see [`curator`](../curator/#knobs) | curator/policy -- the window, since `5.2.0` (GH #889), by the consult role's presets since GH #892; the full table is in the curator's README |
| `max_calls` | param | `16` | cogny/dispatcher -- per-answer call budget |
| `sidecar_verify` | param | `{}` | splitter -- the talky's word-for-word check of quoted fields ([#1079](https://github.com/mmeyerlein/meclaw/issues/1079)), and it stays **empty** at this core: the splitter and its edges are the talky's, so a section named here goes to `./curator` and is checked there, but this core has no `./errors` and draws neither the curator's `reject` nor a checked section out of its curator -- both would dead-letter `no_route`, loud and classified, and the answer text never leaves the core |
| `async_tools` | param | `["ask_requester"]` (ref marker, #894) | cogny/dispatcher -- the core's OWN async tools, as a JSON array or one comma-separated string. The `consult_cogny` declaration belongs on the **asking** side, and since `dispatcher@1.2.0` it can stay there: the knob is a param of each dispatcher cell (GH #138), so the surface's list and this core's list are two statements instead of one shared key |
| `handoff_tools` | param | `["ask_requester"]` (ref marker, #894) | cogny/dispatcher -- async tools whose call ends the TURN because the answer comes from a later one. Since #894 that is `ask_requester`: the answer to the core's question arrives as its next errand. `escalate_to_deep` is gone since 4.4.0, and `consult_cogny` belongs on the asking side, where an advisor's answer arrives as its own turn. A name in this list that no cell serves is a call the dispatcher marks as answered-elsewhere and nothing ever answers |

**There is no `env` column above any more.** Since `dispatcher@1.2.0` the last
three knobs of a cogny tree moved onto `params` with the rest
([#138](https://github.com/mmeyerlein/meclaw/issues/138)); what is left in `.env`
is the provider lane -- the endpoint and the model id; the API key lives in the
vault (#801). Each row names
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
{"op": "instantiate", "template": "cogny@5.9.3", "at": "/cores/deep",
 "override_params": {"collector/assemble": {"max_iter": 16}}}
```

The key is `collector/assemble`, not `collector`. Since
[#140](https://github.com/mmeyerlein/meclaw/issues/140) an `override_params` key is a
cell's path inside the template, and `collector` is a valid one -- it is the sealed
sub-unit's HIVE. A hive reads only `graph`, `ports` and `contract`, so the validator
accepts the key, nothing consumes the params, and the core comes up configured as if the
override had never been written. The knobs live one level down, on the `code` cell behind
the door.

**The curator knobs that sat on the same edge moved to `curator`** with `5.2.0`
(GH #889).

### The curator, live

**Moved to `curator` with `5.2.0`** ([#889](https://github.com/mmeyerlein/meclaw/issues/889)):
the collector's curator, its budget and its `thread_recall` way back are gone, the window is
`./curator`'s, and the `"turn_write": "0"` this section asked every parent to set is the shipped
value of `./curator`'s writer since then.

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
       "ctx":{"model":"anthropic/claude-opus-5.5"},
       "diff":{
        "add_nodes":[{"name":"cogny","template":"cogny"}],
        "add_edges":[ ... the two port PAIRS plus the tool lanes, in the SAME mutation ... ]}}'
```

The composite comes up with eighteen cells (plus three hive markers); the `store` and `llm`
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
string, which is no grant at all (GH #271): standalone this composite
calls its provider with no key at all, because since #801 `api_key` ships empty too. Since 5.0.0 that empty string
is a LITERAL and not a `${COGNY_CREDENTIAL_GRANT_ID:-}` token
([#138](https://github.com/mmeyerlein/meclaw/issues/138), ruling R-0904-6): a
grant id is a reference, not material, and two generations in one colony present
different ones -- which an environment variable, being colony-wide, could not
say. Switching it over is the grant id; the recipe states the empty `api_key` beside it,
because a cell asks for a credential only while it holds none:

```json
"override_params": {
  "brain": {"api_key": "", "credential_grant_id": "grant:…"}
}
```

A key put back into `api_key` next to a grant is ignored (#801). With the grant
set the model runs with **no credential in its config** —
the value arrives sealed against an ephemeral key it mints per ask, is opened in
its own task and is written nowhere. Both keys are **immutable** (`docs/cell-types.md`
§ `llm`), so this is a birth act: a generation grown without the empty `api_key`
is repaired by growing another one, not by a message. The recipe, both edges and
the two operator gestures that go with them are in `templates/member/README.md`
§ *The credential v-lanes*; `examples/vault-pilot/` is the small runnable version
of the same round. The key itself goes into the vault: `meclaw --vault-add cred:openrouter`
(stdin), and the vault has to be unlockable (`access/vault` `key_source` systemd-cred for a
unit, plainfile for a local run).

## Pins

- `crates/meclaw-cells/tests/cogny_template.rs` -- the shipped template in a running
  colony against the mock OpenAI wire: a consult errand enters on the documented ingress,
  the core runs its OWN tool round, and the advice leaves on the return lane under the
  `consult_id` it was given. Since 4.4.0 also the three pins of #528: an `in_schemas`
  request comes back on `tool_schemas` carrying the `consult_cogny` schema with `question`
  and `context` both required; `ask_memory` and `escalate_to_deep` appear in no config or
  manifest of the template any more; and the core is one brain, `collector` + `curator` + `dispatcher` +
  `brain` + `schemas` and nothing else (`curator` since 5.2.0, GH #889).
- `crates/meclaw-colony/tests/gh277_composite_instantiation_is_byte_identical.rs` -- the
  two golden manifests over the instantiated tree (the sub-unit refs produce the same
  bytes the copies did) plus the stamp pin: a cell inside a referenced sub-unit carries
  its OWN template and names the composite above it.
- `crates/meclaw-cells/tests/gh855_an_override_reaches_the_brain_through_its_door.rs` --
  the model door of both composites, with the talky half measured end to end at the mock.
- `crates/meclaw-cells/tests/talky_cogny_advisor.rs` -- the other half: the bilateral
  advisor connection end to end, from a talky's interim answer to the correlated
  follow-up in the channel.
- `crates/meclaw-cells/tests/gh539_a_core_without_a_channel_says_nothing_in_between.rs`
  -- the interim knob: off, a sentence beside a bundle that is waited for emits nothing on
  the answer lane while the calls travel unchanged; the final answer still leaves, exactly
  once; a sentence beside an async-non-handoff bundle still leaves unmarked (GH #378 is not
  rebuilt); the knob is on by default; and this template is the one that turns it off.
- The sub-units keep their own pins: `collector_window.rs`, `collector_colony.rs`,
  `curator_cells.rs`, `dispatcher_template.rs`.

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
| `in_pack` | in | a durable `system.*` slot for the brain: `identity`, `identity_short`, `persona`, `handover` or `instructions`, and nothing else. **Paired**: see `pack_ack`. Since 4.2.0 |
| `in_pin` | in | a pin of another hive for the brain's window, `{pins: [{text, source, until?}], replace_sources?}`, handed to `./curator`'s own `in_pin` (`templates/curator/README.md`). Nothing answers it. Since GH #916 |
| `sidecar` | out | one section of the block the core's answer carried, `hop.section` and `hop.turn_id` (the round) beside it, the body `{messages: [], section, payload}`: every section but `window`, `gap` and `memory`, which `./curator` takes, as a talky does. Since GH #916 |
| `pack_ack` | out | the receipt `in_pack` answers with -- ONE per pack, not one per brain: `hop.pack_owner`, `hop.pack_slots`, `hop.error_code` (empty, `slot_unknown` or `pack_empty`), `hop.pack_unknown`. Since 4.2.0 |
| `in_model` | in | a model package for the brain: a **params-only** body (an empty `system` slot, no `messages`) the colony's `llm-registry` pushes. It goes straight to `./brain`, past the collector, and nothing answers it; since 5.2.0 a package whose `hop.subscriber` ends on `/curator/summarizer` goes to `./curator` instead (GH #889). Since 5.1.0 ([#855](https://github.com/mmeyerlein/meclaw/issues/855)) |
| `model_refused` | out | a model push the brain refused: its error, with `hop.refused_subscriber` (the brain's path) and `hop.refused_model`, instead of on `error`. Draw it back to the registry beside the push edge, or it dead-letters `no_route`. Since 5.1.2 ([#863](https://github.com/mmeyerlein/meclaw/issues/863)) |
| `schemas` | out | the tool names this core declares it uses (`{"tools": ["*"]}` as shipped), for a tools hive's `in_schemas` door. It leaves on a TICK, not per turn. **Paired**: see `in_menu`. Since 4.3.0 |
| `in_menu` | in | their declarations coming back, plus the names that hive had nothing under. They are written into the brain as durable `system.tools`. Since 4.3.0 |
| `in_schemas` | in | somebody asking what THIS core's consult contract looks like: `{"tools": ["consult_cogny", "reply_to_consult"]}` or `["*"]`. **Paired**: see `tool_schemas`. Since 4.4.0 |
| `tool_schemas` | out | the voices' declarations, `consult_cogny` and `reply_to_consult` (#894), provider-neutral, in the tools hive's own answer shape. Since 4.4.0 |
| `ask` | out | the core's question back: one text turn `the core asks: <question>`, `hop.consult_id` naming the consultation. Route it to the asker's advice lane, promoting `hop.consult_id` and clearing `col_phase`. Since #894 |

**The door in the wall (`in_pack`, GH #458) moved to `curator`** with `5.2.0`
([#889](https://github.com/mmeyerlein/meclaw/issues/889)): the pack enters `./curator`, which
holds the closed list `identity` / `identity_short` / `persona` / `handover` / `instructions` in its ledger, hands
it to the brain with the next call and answers `pack_ack` once per pack; the lane and the
mutation that opens it are in [`templates/talky/README.md`](../talky/README.md) § "The door in
the wall".

**The model door (`in_model`, GH #855).** A model package reaches the brain through
`in_model` and nothing else: the door is one edge from `.` straight to `./brain`, past the
collector, because a params-only body is not a turn and nothing answers it; since `5.2.0` a
second one reaches `./curator` for its summarizer (GH #889). The brain takes
package keys from a params-only message only, so a turn on any other lane cannot change the
model it talks to. The full account lives in [`templates/talky/README.md`](../talky/README.md)
§ "The model door"; everything there holds here without exception.

**The brain says what it needs, in prose** (since 5.1.1, [#858](https://github.com/mmeyerlein/meclaw/issues/858)).
`./brain` carries `params.requirement`: a reasoning core that plans and works through tools over
several steps, needs a long context and dependable tool calling, may take a minute or more and is
called far less often than the voice, so a higher price per call is acceptable -- and no model
name. A registry translates it against its catalogue once per change and keeps the result; the
param is immutable and inert without a registry.

**A refused push goes back** (since 5.1.2, [#863](https://github.com/mmeyerlein/meclaw/issues/863)). A push the brain refuses -- a `base_url` outside its
`base_url_allow`, a timeout its backstop does not clear -- is no consultation's failure, and before 5.1.2 it left as `route error`. The refusal of a push
addressed to the cell itself carries two header keys of its own, `hop.refused_subscriber` (the
cell's path) and `hop.refused_model` (the model the push named); every other out-edge of the
cell excludes it, and one way back per cell leaves the hive with it as `model_refused`:

```json
{"from": "./brain", "to": ".", "condition": "has(hop.refused_subscriber)",
 "modifier": {"set_hop": {"route": "'model_refused'"}, "delete_context": ["col_phase"]}}
```

In `meclaw-os` the builder draws the way back beside the push edge (`grow_level`, one edge from
this composite to the container), and the shell carries it on to its registry's `in_refused`
lane, where `show` names the refusal until the next push or a `reset`
(`templates/llm-registry/README.md` § A refused push). Without that edge the refusal dead-letters
`no_route`, loudly. A refusal of anything else -- an operator's push without an address, a push
addressed to another cell -- keeps the shape it had and the edge it always took: the forward that
caused the second is the defect.

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
everything there holds here without exception. The tool the collector answered ITSELF,
`thread_recall`, left that menu with `5.2.0` (GH #889). `memory_recall` was on that list from 4.4.0 to 4.6.1 and is on the ordinary
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
