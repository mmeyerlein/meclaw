# `builder@1.5.2`

The intake that turns a structural wish into a **manifest** — an ordered list of
mutation declarations, ready to be submitted by whoever asked for it.

## What it delivers

A **draft**. Never an application. What leaves this hive on `manifest` is a
proposal plus the sha256 digest over its canonical bytes and one sentence a
human can read before saying yes. Whether it is ever applied is decided
somewhere else, by somebody else, under their own identity.

A draft may also propose a **reusable template**, not only a topology: an
`add_templates` declaration puts a class in the colony's instance-local library
(GH #440). It is applied like every other declaration — by `submit`, under the
identity of whoever asked — and it reaches no further than that library: the
target path is composed from the colony's own template root and the declared
name, so a declaration can add to the library but never rewrite it.

## This hive drafts and never applies

That is not a promise in this file. It is a property of the files:

- **No cell in this template has an edge onto anything but a READ.** The two
  `/colony` edges it does have are `eyes → /colony/graph` and
  `eyes → /colony/registry`, and both are reads. A cell emission is routed over
  the SENDER's out-edges — `target` on an emission is a diagnostic, not an
  address — so a cell without an edge onto the mutation door cannot reach it
  whatever its script does.
- **The edge cannot be added later either.** `/colony/mutations` is not among
  the endpoints a mutation may draw (`MUTATION_DRAWABLE_VIRTUAL_ENDPOINTS` holds
  `/colony/graph`, `/colony/registry` and `/colony/ledger`, on every scope). The
  one edge onto the mutation door lives in the BIRTH topology, and it belongs to
  the submitter.
- **Both facts are measured**, off this tree, by
  `crates/meclaw-cells/tests/gh425_the_builder_cannot_reach_the_mutation_door.rs`
  — where the sharper half has an assertion of its own
  (`no_config_in_the_builder_draws_an_edge_onto_the_mutation_door`), so widening
  the read whitelist cannot quietly widen the guardrail — and, at runtime, by the
  scenario case `I2`. ADR-0015 decision (2) is untouched in substance; its
  amendment of 2026-08-27 changes the SHAPE of one assertion and nothing it
  decides.

## This hive is sealed

`params.ports` is `[]`. There is one address — the hive path — and one way in.
The cells below are named so a reader can follow the walk, not so a caller can
address them.

## Cells

| Cell | Type | What it does |
|---|---|---|
| `classify` | `code` | Reads the tool arguments and decides the CLASS: a named recipe whose parameters are complete, or everything else. Calls no model. |
| `recipes` | `code` | The fast lane. Renders one of four predefined recipes straight into a manifest, deterministically — including the whole transit edge set of a composition level. |
| `builder-librarian` | `ref builder-librarian` | Retrieval over the corpus. Referenced, never copied (ADR-0011). |
| `brief` | `code` | Assembles the authoring prompt: the retrieved sections become instructions, the request stays a user turn. It emits twice — the prompt to the composer, and the same question and the same instruction tree into the round table, because round 1 onwards is briefed by the loop and not by this cell. |
| `compose` | `llm` | The model call of the design lane — asked once per round, not once per build. |
| `dispatcher` | `ref dispatcher` | Which tools did that answer ask for, and is the bundle within budget? Fans one answer out into one call per tool, referenced rather than copied. |
| `lib` | `code` | What does the corpus say? Adapts `librarian_search` and `catalogue_lookup` onto the referenced librarian and its briefing back into a `tool_result`. |
| `eyes` | `code` | What does the colony actually look like right now? Turns `graph_read` and `registry_read` into a `/colony` question and the answer back into a `tool_result`. |
| `unknown` | `code` | A tool that is not one of the four — answered, by name, with `unknown_tool`, so a round never waits for a call that will never run. |
| `weave` | `code` | Is this round complete, and what happens next? The fan-in: it counts, it adopts a refusal, and it decides between another round, a draft and a named stop. |
| `transcript` | `store` | What was said in this build so far? One row per turn — the question first — plus the row holding the instructions every round is re-briefed from, plus the binding row that lets a later refusal find the build it is talking about, plus the compare-and-set guard that lets exactly one path cross the re-entry edge. |

## Lanes

| Direction | Lane | What travels |
|---|---|---|
| in | `in_build` | a structural wish. It carries no promoted identity: this hive never emits a mutation, so it has nothing to attribute, and an edge modifier cannot reach the envelope where the only real identity lives |
| in | `in_receipt` | a draft that was refused at the mutation door, on its way back to the composer that wrote it. It carries `hop.error_code` and the digest of the manifest it refused — no identity, for the same reason `in_build` carries none |
| out | `manifest` | the draft: the declaration list, `hop.manifest_sha256`, `hop.manifest_class`, `hop.declaration_count` |
| out | `error` | a wish this hive did not turn into a manifest, named in `hop.error_code` |
| in | `in_ingest` | a nudge to reconcile the corpus behind this hive against the colony's own template registry. The body is not read: the message IS the nudge |
| out | `catalogue` | what that reconciliation did — `hop.catalogue_known`, `hop.catalogue_ingested` and the names it wrote |

A build that stops says so on a lane. Never as silence, and never as an empty
manifest — an empty manifest is a failure wearing the face of an honest answer.

### `in_ingest` is a TRANSIT lane, and that is the whole of its design (GH #504)

Since 1.5.0 the hive path takes a fourth lane that nothing in this template
reads. It goes straight to `./builder-librarian`, under the very name the librarian
accepts, and the reconciliation's report comes back out on `catalogue`. Two
edges, no cell, no decision.

It exists because the corpus behind this hive is a **seed**: it is loaded once,
when the librarian's store is created, so it describes the library of the moment
this colony was born. A class registered afterwards — an `add_templates`, a
directory dropped in plus a rescan — is resolvable at the mutation door and
invisible to the composer. [GH #496](https://github.com/mmeyerlein/meclaw/issues/496)
built the reconciliation that fixes that and named the one caller who can drive
it: the submitter, which is the only cell in a tree that knows both that a
manifest committed and that its diff registered a class. That caller stands
**outside** this hive, and an edge from it to a cell **inside** this one crosses
two seals — so the lane has to be at the hive path or it cannot exist at all.

The two are paired in `params.required_drains`: a caller that nudges and does not
subscribe to `catalogue` is refused. The counts are the only difference between
*nothing was missing* and *the nudge never ran*, and those two facts must not
look alike.

## The two classes

**Fast lane** — the caller named a recipe and its parameters validate. Four
recipes ship: `rewire_edge` (remove the old edge, then draw the new one),
`add_node` (grow a cell from a template and wire it in), `attach_drain` (hang a
lane on an existing pair) and `grow_level` (§ *A level is a recipe*). No model is
consulted, no network is reachable, and the whole walk is a python start plus
string work.

A recipe that is NAMED but incomplete is an **error**, never a downgrade into
the design lane. A typo in one argument would otherwise silently buy an
inference and answer a different question than the one that was asked.

One sentence is **recognised** rather than named: `grow a <level> named <name>
from <template> under <path>` takes the fast lane without a `recipe` argument.
That is not the forbidden downgrade in reverse — nothing was named, so nothing
is quietly re-asked of a model — and it fires only when every parameter it needs
is in the sentence. A half-read wish falls through to the design lane, which is
where an incomplete sentence belongs.

**Design lane** — everything else. The corpus is consulted first, the briefing
is assembled, one model call is made, and its answer is read. Retrieval is an
ENHANCEMENT: a corpus that is down comes back marked `degraded`, the composer is
TOLD it is working without patterns, and the build carries on.

The briefing carries a **grammar** as well as a vocabulary: the diff keys that
exist, the ENTRY SHAPE of an `add_nodes` entry (`name` and `template`, both
required, plus the one optional `override_params`), the endpoint rule for
`add_edges`, and the one topology rule that decides whether what the model wrote
is reachable — a unit whose edges all stay inside it is born inactive. It sits in the prompt HEAD rather than among the
retrieved patterns, because a corpus outage is when a model has the least to
lean on. Measured rather than assumed: before it existed, a capable hosted model
designed the right topology three times running and encoded every `add_nodes`
entry wrong, and the door refused all three at declaration 1
(`crates/meclaw-cells/tests/builder_brief_mutation_grammar.rs`).

It also carries the rule that a template may **demand** keys before it can be
instantiated, and points at where that demand is legible: every row of the
librarian's template catalogue now opens with a contract line naming the `ctx`
keys an instantiation owes, refs included. That line exists because the catalogue
did not publish what it enforced — a `template.json` is serialised
description-first, the retrieving cell hands the model the first 1200 characters
of a row, and for a large composite the `requires` block was not in that row at
all. Measured the same way: with the grammar in place and the contract still
invisible, the same model encoded every entry correctly and was refused one level
higher, with `requirement_missing`, for naming a template and passing it an empty
`ctx` (`crates/meclaw-cells/tests/librarian_catalogue_carries_the_contract.rs`).

Since `1.2.0` it carries four more blocks, and each one answers a draft that
LOOKS right — the failure class that costs a round rather than a refusal:

- **`OVERRIDES`** — `override_params` is admitted as the one optional key of an
  `add_nodes` entry, and it is **addressed**: the key is a cell path inside the
  template (`{"cogny/brain": {"temperature": 0.2}}`), `""` is the node itself,
  and a key addressing no cell is a refusal that lists the cells there are. It
  is not a channel for `ctx`, and the grammar says so in both directions.
- **`REFS`** — `ref` is a declaration form and not a cell type a model may ask
  for. It is where the union that the catalogue's `CONTRACT —` line computes
  actually comes from: `assistant` demands `ctx.model` and `ctx.model_fast`
  without restating either, because its refs do. The union goes the other way
  too: since [#516](https://github.com/mmeyerlein/meclaw/issues/516) that level
  declares one key of its **own** on top of what it inherits — `ctx.model_surface`,
  the model its conversation surface infers with — because a level that
  references `talky` AND `cogny` is instantiated with one flat `ctx`, so without
  a key of its own both brains resolve `ctx.model` and the surface runs the
  reasoning model. Read the `CONTRACT —` line and supply every key on it: the
  count is not fixed at two.
- **`LEVELS`** and **the address rule, v1** — the transit edge set of a level is
  fixed and is not to be invented as a subset, and one edge per assistant guarded
  on `context.assistant` is a **sum, never the cross product**, because `Edge.to`
  is a static path in this substrate. The rule holds one storey up unchanged, on
  `context.org` and `context.member`. The rule holds one storey up unchanged, on
  `context.org` and `context.member`. That rule lived only in
  `templates/README.md` and `templates/member/README.md`; a model that never read
  those wrote the cross product. Beside it stands the first example at path depth
  **two** (`assistants/scribe`), because every example the grammar had was one
  segment deep and a level is never one segment deep.
- **The model is not yours to invent** — see below.

Since `1.4.0` there is a fifth, and it answers the opposite failure — not a
draft that looks right, but **no draft at all**:

- **`TEMPLATES`** — `add_templates` was named in the head's list of eight diff
  keys and its FORM was published nowhere, so the one key that answers *the
  class I need does not exist* was the one key the composer could not use, while
  `seed_rows` had `ROWS`, `override_params` had `OVERRIDES` and `birth` had
  `BIRTH`. Measured on 2026-08-29 with a real hosted model on the first wish no
  recipe covers — *"a feed cell under the researcher that fetches three RSS
  feeds every ten minutes and emits one headline document per new item"*: the
  composer spent all seven rounds and every one of them well, never lost the
  thread, never wrote nonsense, and spent the whole budget searching the
  catalogue for a `timer`, a `web_fetch`, a `code` cell and a `store` — the four
  types `templates/_cell-types/README.md` deliberately ships no single-cell
  template for. It ended on `no_manifest_in_answer`, prompt grown 5 557 → 50 220
  tokens. The block gives the entry shape (`name` plus `files`, `template.json`
  required, the name pattern, `{templates_root}/local/<name>/`,
  `invalid_template_name` and `template_name_taken`), says the operation runs
  **first** in its diff so an `add_nodes` of the same diff resolves the class it
  just registered — which is why a build out of an own design is one manifest
  and not two — and names the price: a manifest bringing executable behaviour
  makes the submitter ask a **second** capability question, `code.author`, off
  by default, and a `code_author_denied` means *this colony does not allow
  imported execution*, not *your manifest was malformed*. Two sentences carry
  the whole saving. That the four types have no template to name is what ends
  the search loop; that the colony decides is what keeps the block from reading
  as a prohibition — the composer learns that `add_templates` EXISTS, and gains
  nothing it was not already allowed to do (#482).

### The composer may ASK instead of guessing

The one thing a design lane cannot do honestly is fill in a fact it was never
given. A measured run wrote its `ctx.model` as an invented literal: a manifest
that validates, applies cleanly and boots against an endpoint that does not
exist — which is the most expensive kind of wrong, because every gate before the
first inference says yes.

So `requires.ctx.model` is a **mandatory question** — and so is every other key
the `CONTRACT —` line names, `ctx.model_fast` and `ctx.model_surface` included.
If the request names no
model and the template's `CONTRACT —` line asks for one, the composer answers
with `{"question": "…"}` and no declarations, and `normalise` turns that into the
`error` lane under a code of its own: **`wish_incomplete`**, carrying the
question verbatim. It is deliberately not `declarations_not_a_list` — "you did
not tell me the model" and "your answer was not a list" call for opposite
repairs, and a refusal a human cannot read is one they cannot answer.

## A level is a recipe

Growing a child into a composition level was, until `1.2.0`, a paragraph a model
rewrote from scratch on every build: an organisation gets **18** transit edges, a
member **18**, an assistant **14**, a channel **3**, a screen **2**, an app
**2** — and they are the same edges every time, with the child's name
substituted in. `examples/organism` writes all six out by hand, which is what
made them measurable.

A container level — an org, a member — costs the doors `in_turn`, `in_recall`,
`in_brief`, `in_propose`, `in_build_result` and `in_export`, and the exits
`answer`, `ack`, `reject`, `error`, `write`, `turn_write`, `prune`, `build`,
`close_report`, `export_done` and `pack_ack`. The two levels share one renderer
because they share one contract, lane for lane, one storey apart. `in_import` is
the lane both accept and neither wires: an import addresses the level's own
path, so an edge from the container could never deliver one. **The table stood
at thirteen from before the member had a memory export** and nothing was red,
because the byte pin below compared it against examples generated out of itself
([#470](https://github.com/mmeyerlein/meclaw/issues/470)); the second generator
of that same set, `examples/memory-import/build_import.py`, had been writing it
correctly the whole time, and the two are now compared element for element.

**A container's children are ADDRESSES, not a broadcast** ([#478](https://github.com/mmeyerlein/meclaw/issues/478)). Each
of those six doors carries the child's own name beside the lane —
`(!has(context.member) || context.member == '<name>')` for a member,
`context.org` for an organisation — because `Edge.to` is a static path one storey
up as well. Without the name, a container holding two children fans every message
on the lane out to *both*: two inferences, two costs, a turn in the memory of a
member it has nothing to do with, and an `in_export` addressed at one member that
exports all of them. The guard is **permissive** on purpose. Nothing in a grown
topology promotes either key today, so a strict `has(…) && … == name` would
strand every existing colony's turns at the container the day it landed; without
the key the delivery is exactly what it always was, with it, the message reaches
one child. It is the form the assistant level already writes for
`in_build_result`. The table stood unguarded from the day it was written and
nothing was red, because every colony grown out of it held exactly one member per
organisation — where an unguarded lane and an address are indistinguishable. Same
blind spot as [#470](https://github.com/mmeyerlein/meclaw/issues/470), one guard
over.

### Where a level declares itself

A level **declares itself at the container it grows into**: an organisation at
`/os/orgs`, a member at `<org>/members`, an assistant at `<member>/assistants`, a
channel or a screen at `<member>/channels`, an app at `<member>/apps`. The scope
root *is* the container, the child is named **bare** in `add_nodes`, and the
edges are `.` ↔ `./<name>` — `.` being the declaration's own scope
([#487](https://github.com/mmeyerlein/meclaw/issues/487)).

Until `1.4.3` the declaration stood one storey higher: the scope was the
container's parent and the container travelled inside the node name
(`{"scope": "/os", "add_nodes": [{"name": "orgs/acme"}]}`, `./orgs → ./orgs/acme`).
Both forms grow the same tree — the absolute edges are identical to the byte —
but the scope root is what the **broker** judges, and the shipped
`colony.mutate.default` rule permits `/os/orgs` and below. The first
organisation of a colony is grown at `/os`, so the one build every colony starts
with came back `requester_not_permitted` at the front door while the very same
manifest, applied by an operator, committed. Every other level was already under
the prefix and never noticed
([#503](https://github.com/mmeyerlein/meclaw/issues/503)).

The recipe **parameter** `scope` is unchanged: it is still the parent the wish
talks about ("grow an org named acme under `/os`"). Only the rendered
declaration moves down into the container. One level keeps the wide form, and it
is a reachability fact rather than a taste — see § *The identity door is opt-in*.

An **assistant** level costs the two turn doors (`in_turn`, `in_bundle`, both
guarded on `context.assistant`), `in_build_result`, the eight exits `answer`,
`recall`, `extraction`, `write`, `turn_write`, `prune`, `error` and `build` —
and, since [#476](https://github.com/mmeyerlein/meclaw/issues/476), the three
**transfer** edges [#475](https://github.com/mmeyerlein/meclaw/issues/475)
opened: `in_export` and `in_import` down under the same `context.assistant`
guard a turn carries, and `dump` back up. A generation holds one store its
member cannot recompute — the session ledger of its own `session-keeper`, four
levels down — and until the recipe drew those three, a generation grown from a
wish could not receive it: an `in_export` that named it stopped as `no_route` at
the container, silently, because an export that walked three holders looks
exactly like a complete one. The `dump` edge is **plain** on purpose: every
level between the container and the keeper pairs `in_export` with `dump` in
`params.required_drains`, and the probe that checks the pairing runs the
described hop through the real edge evaluator — an edge that additionally tested
`hop.dump_kind` reads as no drain at all and the mutation is refused.

`grow_level` renders them from a table. What it does **not** decide is the
template: which class a level is filled with is a catalogue question, and the
catalogue is the librarian's. The recipe is told the template and renders the
edges; the model looks the template up and names it. Same for `ctx` — the recipe
passes what it was given straight into the declaration and asserts nothing about
it, because the authority on what a template demands is the mutation door, which
answers `requirement_missing` by name and hands it back on `in_receipt`. With
**one** exception, and it is the exception that proves the rule: `member_person`
on a `channel` (§ *A round is provenance*).

| Parameter | Required | What it is |
|---|---|---|
| `scope` | always | the parent level, e.g. a member path — the wish's own word for it. The rendered declaration stands one storey deeper, at the container (§ *Where a level declares itself*) |
| `level` | always | `org`, `member`, `assistant`, `channel`, `screen` or `app` — a name the table does not carry is refused as `level_unknown`, never rendered as something close |
| `name` | always | the child's own name, written **bare** into `add_nodes` — the container it lands in is the declaration's scope, not part of the name |
| `template` | always | the class to instantiate, pinned |
| `assistant` | `channel` | the agent a turn defaults to. A CEL guard is evaluated against `hop` and `context` and cannot read a node's `params`, so the default of a channel has nowhere to live but the edge that applies it |
| `screen` | `app` | the one screen the app writes its views onto |
| `ctx` | optional, **`member_person` required for `channel`** | the declaration's own `ctx` block, mutation-wide. The recipe reads exactly one key out of it — `member_person`, the identity of the person a channel speaks with — and a channel wish without it renders nothing and asks instead, as `wish_incomplete` (§ *A round is provenance*) |
| `override_params` | optional | addressed per cell of the template (`{"cogny/brain": {"temperature": 0.2}}`) |
| `birth` | optional | `active` or `inactive` — the door's own vocabulary, written top-level on the `add_nodes` entry. A name the door does not know is refused here as `birth_unknown`, one hop from the wish that made it, rather than at the door one hop from the manifest. The default is the door's (`active`) for every level except `channel`, which is born **asleep** |
| `subscribe` | optional, `assistant` | draw the identity door as well — the push edge from the member's own `./affinity` and the `pack_ack` drain beside it. It is not part of the level and is not counted in the table above; see § *The identity door is opt-in* |

### A round is provenance, and provenance is not derived

The ingress edge of a channel declares `context.audience_set`: the round every
turn on that channel is spoken in, as a JSON list in affinity vocabulary —
`["agent:<the assistant>", "member:<the person>"]`. Until
[#517](https://github.com/mmeyerlein/meclaw/issues/517) the recipe **invented**
the second half. It read the member's identity off the last segment of the scope
the wish had named — the DIRECTORY the member stands in — which is the right
answer only while the folder happens to be spelled like the person.

A member directory called `egon` holding a person called `marcus` is the normal
case, not a pathology, and there the edge declared `member:egon`: a participant
that exists in no row of the store. What that costs is not a wrong string.
`memory-hive/recall`'s gate admits a row only when the declared round is a
**subset** of the row's own, so one wrong name refuses **every** row, in every
leg, before the fusion. Measured on a live colony — 34 facts, 182 episodes, all
carrying one correct round:

| `audience_now` | keyword | semantic | graph | temporal | candidates |
|---|---|---|---|---|---|
| the round the rows carry | 22 | 20 | 9 | 20 | **19** |
| the round the recipe guessed | 0 | 0 | 0 | 0 | **0** |

The bundle then says *"Nothing in this memory answers this question"* — word for
word what a genuinely empty store produces. Nothing on the hop, in the
diagnostic or in the log told the two apart (`leg_sizes_raw` is post-gate by
design, GH #297). The gate behaved exactly as specified; the declaration handed
to it was the lie.

So the person is a thing the **wish** says, in the declaration's own `ctx` block
under `member_person`, and a wish that does not say it is asked rather than
guessed at — `wish_incomplete`, the same code `normalise` returns when a composer
declines to invent a model id, with the question in the words a human has to
answer. `ctx` is where it lives for a second reason: the claim then stands in the
rendered manifest, in front of the reviewer, before it is committed, and the
mutation door ignores a ctx key no template of the declaration requires.

The agent half is **not** the same defect: `agent:<assistant>` is built from the
`assistant` parameter, which is a word the wish is already required to say. It is
a quotation, not a guess.

Two smaller repairs ride on the same edge. `chat_id` and `user_id` are promoted
`has(...) ? ... : ''` rather than as bare reads — the rule
`templates/member/README.md` publishes one storey up, for the reason it gives
there: a modifier that fails to evaluate skips the **whole** edge, and the
connector's own failure emissions carry neither key, which logged
`cel eval set_context.chat_id: No such key: chat_id` on every one of them.

### A channel is a node and a chat, and they are two words now

The half #517 deliberately left alone, and
[#522](https://github.com/mmeyerlein/meclaw/issues/522) is where it was decided.
`context.channel` used to be the **node name**, although
`templates/session-keeper/README.md` describes that key as the chat identity and
the hand-drawn e9-era edge promoted `hop.chat_id` into it — the rows that exist
carry a chat id in every `channel` column a colony ever wrote. It had to be the
node name, because the set this recipe renders has three edges and the third is
the answer's way back: `. -> ./<name>`, guarded on the key, because `Edge.to` is
a static path and a container may hold several channels (the address rule, GH
#454). A `channel` carrying a chat id routed no answer anywhere and the agent
went mute on the surface it was reached on.

So the return path moved onto a key of its own, and the ingress edge renders
**two**:

| key | value | what it is for |
|---|---|---|
| `context.channel_node` | `'<name>'` | the ADDRESS. The third edge guards on it; so does `./assistants -> ./channels` one storey up |
| `context.channel` | `has(hop.chat_id) ? hop.chat_id : ''` | the CHAT. One session generation, one rate bucket and one memory room per value |

Written as one word, every chat of one connector shared **one** session
generation: the idle clock, the nightly close and the session id were computed
over the union of all of them, and the channel-local clause of the audience gate
could never match a row recorded under a chat id. A **screen** renders the same
word in both, because a screen is one room; a chat connector does not, and that
is the whole of the repair. `templates/member/README.md` § *The two channel keys*
is where the rule is published, and
`crates/meclaw-cells/tests/gh522_a_chat_is_a_generation_and_a_node_is_an_address.rs`
holds the renderer and the addressing rule together.

### Born asleep is a parameter, not a second door

`add_nodes[].birth` (GH #437) lets a whole subtree come to the world registered, addressable and
**taskless** — the mechanism a connector needs, because a channel must exist in the topology
before its upstream is real. Until `1.3.0` the recipe could not express it, so a channel grown
from a wish was always born awake and getting it asleep meant routing the draft through the
operator's `in_lifecycle` door instead of submitting it as a manifest: a second door for what is
one decision ([#472](https://github.com/mmeyerlein/meclaw/issues/472)).

It is now a parameter, and `channel` is the one level whose **default** is `inactive`. That is not
a preference about connectors — it is what the substrate does with a `proxy` cell the moment it
has a task, and the ruling behind
[#468](https://github.com/mmeyerlein/meclaw/issues/468) is that a connector comes asleep and is
armed deliberately. Since [#491](https://github.com/mmeyerlein/meclaw/issues/491) the
parameter is a durable decision and not a starting value: a level grown asleep stays asleep
across restarts and across every later mutation that does not name it, so the two mechanisms
now really are one decision each rather than one durable and one temporary.
The switch reads it out of the sentence too: a wish that says *asleep* or
*born asleep* renders `inactive`, one that says *awake* renders `active`, and an explicit `birth`
argument beats both — a key a caller filled in is a decision, a sentence is a reading of one.

### The identity door is opt-in, and only its graph half is renderable

A grown assistant reaches its brain with an **empty** `system` tree: nothing in a grown topology
writes a durable slot, and the one lane that can — `in_pack`, GH #458 — needs an edge from the
member's own record ([#473](https://github.com/mmeyerlein/meclaw/issues/473)). `subscribe: true`
renders it: `./affinity → ./assistants/<name>` on `hop.route == 'answer' && hop.subscriber ==
'./assistants/<name>'`, re-stamped onto `in_pack`, plus the `pack_ack` drain back into
`./assistants` — because `in_pack` and `pack_ack` are one decision and the door refuses the first
without the second.

**This one declaration keeps the wide form**, and the reason is measured rather than chosen.
`./affinity` is a *sibling* of `./assistants`: from a declaration standing in the container the
only spellings that could reach it are `../affinity` and an absolute path, and the mutation door
refuses both in an edge endpoint outright. Splitting the door off into a second declaration at the
member does not survive either — `templates/submit/gate` accepts an `in_pack` edge only when its
target is under the requester or is created by *that same* declaration, and a declaration that
only draws edges creates nothing (measured: `subscribe_target_not_self`). So a `subscribe` wish
renders scope `<member>` and node `assistants/<name>`, exactly as it did before
[#503](https://github.com/mmeyerlein/meclaw/issues/503). It costs nothing that matters: an
assistant is grown under `/os/orgs/…` either way, which is inside the prefix the broker permits.

Two things it deliberately does **not** do. It does not draw the edge by default — and since
[#479](https://github.com/mmeyerlein/meclaw/issues/479) that is a **decision** rather than a
mechanical consequence. The submitter's form check now has a second branch for exactly this case:
a parent may draw an `in_pack` edge into a node the same declaration creates, so a level that
always drew one *would* be submittable. It stays opt-in because writing into a brain's prompt is
not something a level should do to every generation grown from it without being asked. (Until
#479 the rule had one branch and the sentence here read the other way round — a level that always
drew one could be grown by nobody, including the brain being grown, because the requester at the
gate is never the brain that does not exist yet.) And it does not write the `subscribers` row —
that is a store write, not a mutation declaration, and it stays a `subscribe` op through
affinity's own gate. What the recipe renders is the half a manifest can carry: the graph the row
will need.

**Why a table and not a derivation.** The obvious idea is to compute the set from
the child's `contract.accepts` / `contract.emits`. It does not work, and the
reason is worth writing down so nobody re-derives the disappointment:

- **A connector has no contract at all.** `telegram-connector` is a `proxy` cell;
  its `accepts` and `emits` are both empty, and its two upward edges condition on
  `has(hop.error_code)` — on a failure key, not on a lane.
- **Lane count is not edge count**, in either direction. `assistant` declares
  twenty lanes and gets fourteen edges: four of them are addressed at the path
  directly (ruling W7-R5), `in_pack` and `pack_ack` are the opt-in identity door
  and no part of the level, and `display` folds `event`+`receipt` into **one**
  edge one level further out.
- **Guards, modifiers and literals live in no contract.** `context.assistant ==
  '<name>'`, the `audience_set` literal, `hop.owner.contains(…)`, the
  `set_hop.route = 'in_view'` restamp. Addressing is a property of the parent,
  and `params.contract` has no vocabulary for it.

So: a table, and the table is pinned against the examples rather than described.
`crates/meclaw-cells/tests/gh466_grow_level_renders_the_level.rs` renders all six
levels and compares them **byte for byte** against `examples/organism/grow-*.json`
— the recipe and the worked example cannot drift apart, because one is generated
and diffed against the other.

## The design lane is a loop, and it is bounded four times

The composer is asked, it may call one of four tools, the results come back, and
it is asked again — until it answers with a manifest or until a bound stops it.
Four bounds, and each one buys something different:

| Bound | Where it lives | Default | What it stops |
|---|---|---|---|
| `BUILDER_MAX_ITER` | the condition of `./weave → ./compose` | 6 | a model that keeps looking and never writes |
| `BUILDER_WRITE_ROUNDS` | `params` of `weave` | 1 | the last round being spent looking as well |
| `DISPATCHER_MAX_CALLS` | the referenced `dispatcher` | 16 | one answer asking for forty tools at once |
| `BUILDER_MAX_REPAIRS` | the condition of the repair edge | 2 | a refusal being retried forever |
| `BUILDER_ROUND_IDLE_MS` | arithmetic in `weave` | 120000 | a fan-in waiting on a result that will never come |

The substrate's own guard is deliberately out of the game: the re-entry edge
declares `restore_ttl`, so the loop pays for one round at a time instead of
fitting all of them into one budget — about a dozen routing hops per round
against a default `ttl` of 64 (`docs/store-backed-tool-loop.en.md` § *The TTL
budget of one round*). That is why the four above have to carry. A TTL death
would be silent: straight to the dead-letter queue, no `reply_to` cascade, and
the `tool_call_id` waiting in `templates/tools/build-draft` would never close.

Three of the four are `params` of `weave` rather than environment variables, so
two builders in one colony are tuned apart and a mutation retunes one without
touching a colony-wide setting. A CEL condition cannot read `params`, so the two
bounds that live on an edge are written twice — as a literal on the edge that
enforces them and as a settings default in the cell that documents them — and
the two spellings are held to one number off the tree by
`crates/meclaw-cells/tests/builder_bounds_agree_with_their_settings.rs`.

**Since `1.4.0` `BUILDER_MAX_ITER` is written a third time, in prose.** The
composer was never told how many rounds it had, and the measured cost of that is
the whole of #482: seven rounds of looking, the cap firing on an answer that was
still asking, and `no_manifest_in_answer` as the ending — paid for, and nothing
delivered. The head now names the number — as **re-entries**, which is what the
edge counts: the composer is asked once out of the brief and `max_iter` times
after that, so "six rounds" would have understated its own budget by one — and
says what to do at the end of it, which is to write the best manifest that
follows from what is known rather than ask once more: a draft the door refuses by name comes back on `in_receipt` and is
repairable, an empty answer comes back as nothing. A number in a prompt is still
a number on an edge, so the same test holds all three spellings to one value.

### The round the composer may not spend looking

**Since `1.4.1`.** The bound above was a bound the model could not see, and #485
measured what that costs: seven rounds, thirteen tool calls, **not one text
turn**, and the build ending on a code about the last answer rather than about
the budget. Eleven builds of that class ran in one rebuild; the only ones that
produced a manifest had a sentence in the *wish* telling the model not to call a
tool — which works for one turn, does not survive a repair round, and is a
workaround rather than a mechanism.

Three mechanisms replace it, and none of them is a sentence the model may weigh:

* **The budget rides in the prompt, recomputed every round.** `weave` already
  held both numbers — it stamps `hop.rounds_done` and `hop.round_capped` on
  every emission that closes a round — and kept them to itself. The re-briefing
  now carries a `system.budget` slot saying which round this is, how many may
  still call tools, and what happens at the end of them. It is a slot of its own
  rather than a paragraph appended to `instructions`, because the instructions
  are **parked** once per build (§ *The question survives in a row*) and a
  number written into them would be round 0's number forever. It is deliberately
  not named in `compose`'s `system_order`: the concatenation appends the slots
  that are not named there after the ones that are, so the budget lands **last**
  in the prompt — which is where a rule about *this* round belongs — and the
  declared order stays the two slots every other shipped `llm` cell declares.
* **The last `BUILDER_WRITE_ROUNDS` rounds are WRITING rounds: the tool menu is
  not published in them.** With no `system.tools`, the request carries no `tools`
  array, so a tool call is not a move the wire admits and the manifest is the
  only answer left. The research budget and the writing budget are two budgets
  for the same reason the repair budget is a third: a model that spends every
  round it has on looking has none left for the answer.
* **The re-briefing is authoritative.** The system tree now carries
  `"$replace": true` at its root (GH #264). An `llm` cell UPSERTS the system
  tree it is sent into its own `cell.db` per slot path, and this composer is ONE
  cell for every build in the colony — so a slot merely left out of a message is
  not gone, it is remembered. The first walk of the new writing round measured
  exactly that: the body carried no tools and the model answered
  `finish_reason: tool_calls`, because the menu of the round before was still in
  the store. The marker is also GH #477's *one spelling of the prompt* made true
  against residue: two builds in one colony cannot leak a prompt into each other.

And when the cap does fire, it says so. A capped thread reaches `normalise` with
`hop.round_capped` on it, and the two codes that mean *nothing usable was
written* — `no_manifest_in_answer`, `declarations_not_a_list` — become
**`design_budget_exhausted`**, carrying the rounds spent. Both old codes are true
sentences about the last turn and neither is the reason the build ended: "your
answer was not a list" sends a reader to look at an answer, "you ran out of
rounds" sends them to the wish, the corpus or the cap. The rename is narrow on
purpose — a capped round that *did* write a manifest still ships it, and a
composer that ASKED is still `wish_incomplete`, because a question is an answer
and the cap did not cause it.

### The sentence beside the bundle is not a dead letter

**Since `1.4.1`.** `dispatcher@1.1.2` splits one model answer into two emissions
when it carries content **and** tool calls: the bundle on `hop.route == 'calls'`,
the sentence beside it on `hop.route == 'answer'` with `hop.interim` set (GH
#378). This hive drew edges for `calls`, for `tool_name`, for `result` and a
default on `hop.route == 'tool'` — and none for `answer`. A hosted model narrates
while it calls, so **every narrating round left a `no_route` dead letter** beside
a round that otherwise worked: noise that hides real ones, and the one part of
the answer that could have carried a manifest, dropped unread.

`./dispatcher → ./weave` now carries it, and `weave` writes it into the round table
under the role `interim`. It is recorded and never replayed: an interim sentence
and its own bundle are ONE provider message, and re-entering the thread as a
second `assistant` turn would put it between a bundle and its results — a message
order no OpenAI-shaped endpoint accepts. `rebuild` skips the role for the same
reason it skips `caller` and `system`. The leg parks **without** a read-back, so
it never joins a round election it did not open.

### How a lane leaves a unit

**Since `1.4.1`.** The `ENDPOINTS` block of the briefing said that `.` is not an
endpoint and stopped there, and a design-lane wish asks for the other half more
often than for any other shape: a notice that must reach a unit's parent, an
answer that must leave a level. Measured twice — once as a model spending a whole
round budget deliberating about the contradiction, once as a manifest the door
refused with `edge_schema` — and the repair is a rule, not a refusal:

> Declare it ONE SCOPE UP and name the unit by its relative path: at the parent,
> `{"from": "./<unit>/<cell>", "to": "./<unit>"}`.

with the trap named beside it, because it is the reason the mistake is made:
**every shipped template spells exactly this lane `"to": "."` in its own
`params.graph`** — legal there, refused in a mutation (GH #487) — so the spelling
a model finds by looking a template up is the one the door will not take. The
block now says both, and says that a wish phrased from inside a unit ("leave the
member on the hold lane") is a declaration written at the organisation.

### A round trip is not a lane, and only one compartment can name its legs

**Since `1.5.1`.** The briefing taught lanes, conditions and modifiers and never
said what a cell does with the ANSWER that comes back on the lane its request
left on. Measured on a verification run: a composer drew the two-phase dedupe a
store that ships no constraints forces — ask what is already there, insert what
is new — and guarded the way back on `context.phase`, which the outbound edge had
set. `context` is persistent, so `phase` was still `'select'` on the answer to
the *insert*; the script re-entered its own write branch and the pair wrote
1 200 rows for 40 distinct links before the TTL stopped it
([#521](https://github.com/mmeyerlein/meclaw/issues/521)).

The `ROUND TRIP` block is the rule that was missing, and it is one asymmetry:

> `hop` is SINGLE-HOP — replaced on every emission, so nothing you stamped on the
> way out survives the answer. `context` is PERSISTENT — carried, and therefore
> identical on every leg. So a return edge is guarded on a field of the ANSWER,
> and POSITIVELY.

with the store's own spelling beside it (`hop.operation` is `select`, `insert`,
or `bundle` as soon as the message carried more than one call) and the second
half nobody remembers: the answer no guarded edge takes needs a home — a second
edge, or one with `"default": true` — or it dead-letters as `no_route`. The
negative shape the rule forbids, *anything that is not a fresh request*, is the
one `workshop/cookbook/reply-to-fallback-loops.md` is named after, and the
librarian this hive refs already recognises its own second phase the right way.

## Four eyes, and no hand

| Tool | Answers the refusal | Reads |
|---|---|---|
| `catalogue_lookup` | `requirement_missing` | the corpus row that opens `CONTRACT —` |
| `librarian_search` | a shape written from memory | the spec, the cookbook, the rewiring doc |
| `graph_read` | the ISLAND | `/colony/graph` — activity is edge-derived, so it SHOWS |
| `registry_read` | the wrong template, named plausibly | `/colony/registry` — what actually stands where |

Each one answers a refusal this system has really produced in a measured
build run (`CHANGELOG.md` § 0.26.0). That is the difference
between this and a general harness: the vocabulary is closed, and its closedness
is a property of the files
(`crates/meclaw-cells/tests/builder_tool_vocabulary_is_closed.rs`).

There is no fifth tool that applies anything. Submitting stays with whoever
asked, under the identity the substrate stamped on the envelope — ADR-0015
decision (2), amended on 2026-08-27 only in the shape of one assertion, not in
what it decides.

There is also one answer the four eyes cannot give: **no template does this**.
`catalogue_lookup` is FTS5 and always returns its best hits, so from the
caller's side *not found* and *found something adjacent* look alike — asked for
a `code` cell template on its last round, it answered with a cookbook page on
scheduled workflows, a rewiring section and `archive-bridge`, all plausible
neighbours and none of them a class to name. A caller that cannot tell the two
apart has no reason to stop asking. So the head states the absence outright,
for the four types that have none, and points at `add_templates` instead of at
another query (#482). Making the catalogue itself answer *no template by that
name, here are the names there are* would end the loop one round earlier and is
the open half of that issue.

### The question survives in a row, and so does the prompt

`brief` runs **once** per build. Everything after round 0 is assembled by the
fan-in out of the round table, so a turn that was never written into that table
exists nowhere one hop later. Two of them were not: the `user` turn carrying the
request, and the `system` tree carrying the grammar, the endpoint rule, the scope
line and the four tool schemas.

Measured (GH #477): a wish went in, the composer opened with two
`catalogue_lookup` calls — the first move the briefing asks for — the fan-in
closed the round, and the body that re-entered `./compose` held two `tool_call`
turns and two `tool_result` turns and nothing else. The model answered by asking
what it was supposed to build; `normalise` named that `wish_incomplete`; the
build was over, with nothing having gone wrong anywhere.

So `brief` parks both, in one bundle over the edge `./brief → ./transcript`, and
it emits that leg **first**: a multi-send is dispatched in plain order, and the
composer's answer still has to cross a model, the dispatcher and a tool before
the fan-in reads the slate back. `weave` reads them back on every route that
re-enters the composer — `fire` and `repair` alike — the `user` row as a turn
(`rebuild` already sorted `user` ahead of the `assistant` of its round; the role
was provided for in the sort key and had never been written), the `system` row
**not** as a turn but as the body's `system` slot, verbatim. One spelling of the
prompt, never two.

The `draft` route deliberately carries neither: it leaves for `normalise`, which
reads a manifest out of the last turn and has no use for a prompt.

Why no case caught it: every scenario case of the design lane drives a stub model
(`workshop/evals/builder-scenarios/answers/*.json`) that answers by position and
never reads its prompt, and a stub cannot notice a missing question. Pinned by
`crates/meclaw-cells/tests/gh477_the_second_round_still_knows_the_question.rs`,
which asserts the PROMPT and no answer at all.

### What the corpus publishes, and what it used to withhold

**Since `1.4.1`, and it is two repairs of one shape** — the shape the `CONTRACT —`
line was the first instance of: a rule the briefing states exactly, whose data
the corpus does not carry.

* **The level edge sets (GH #486).** The `LEVELS` block tells the composer that a
  level's edges are a fixed set, forbids inventing a subset, gives the counts —
  and named the *fast lane's* renderer as where the sets live, which is the one
  place a design-lane build cannot reach. Two measured builds spent all seven
  rounds reconstructing `grow-screen.json` from prose, one guard at a time.
  `examples/organism/grow-*.json` are the six rendered sets and are byte-pinned
  against that renderer, so they are now indexed — **condensed**, one row per
  level, because `grow-member.json` is 2 934 characters and the retrieving cell
  hands the model 1 200 of them: a fixed set published half is worse than one not
  published, because it looks complete.
* **The store schemas (GH #483).** `seed_rows` must name a table the target
  store's `params.schema` declares and may use no key that is not a declared
  column — and no shipped store's tables or columns were in any chunk. A wish
  needing exactly one `seed_rows` declaration spent seven rounds guessing column
  names, none of them real. The corpus now carries one row per **table** (the
  unit a `seed_rows` entry addresses, and the unit small enough to arrive whole),
  and every catalogue row opens with a second contract line beside `CONTRACT —`:

  ```
  STORES — `argus@<its version>` carries 2 store cell(s): ./charter (tables:
  goals, rules), ./receipts (tables: cycles, waits). …
  ```

  ahead of the truncation, for the same reason the contract line leads.

And the catalogue is now the catalogue. `./lib` has always stamped
`hop.lib_kind = "template"` for a `catalogue_lookup`, and its own contract
published that as *"the same corpus filtered to the rows that open with
`CONTRACT —`"* — but nothing read the key, so both tools ran one unfiltered BM25
query. Measured: `catalogue_lookup "member"` answered with `org` at position one
and `member` at position four, as a continuation chunk. `builder-librarian`'s
retriever now passes the kind through to the store's `where`, which costs one key
and no second query. `librarian_search` stays unfiltered, and it is how the level
rows and the store rows are reached.

### The two eyes carry their own coordinate

A `/colony` read is not a cell: it has no `cell.db`, no timer and no context of
its own, and a reply to it starts a FRESH trace. So nothing of the round survives
the roundtrip except what the question itself carried. That is the `tag`, an echo
field this wave put on `GraphQuery` and `RegistryQuery`, and it holds the whole
coordinate rather than the build alone — `<build_id>.<iter>.<repairs>#<tool_call_id>`,
truncated to 64 characters rather than refused. `eyes` reads it back off the
answer and puts the three numbers into `hop` again, where the edge to `weave`
lifts them into `context`. Correlation is therefore a property of the message,
not of a cell keeping notes: two builds and two rounds may be in flight at once
and no answer can land in the wrong one. A round number that went missing here
would leave a CEL modifier without its key, and a failed modifier SKIPS the edge
— which is why the tag carries all three and never some of them.

### The fan-in carries the caller, and the round table is where it survives

The three coordinates above are what the **loop** needs. What the **caller**
needs — `build_call_id`, `agent`, `build_op`, `build_scope` — was on no list, and
that was decided by a race rather than by a rule: the slate of a round is read by
the leg that arrives **last**, and it is that leg's `context` that travels on. A
`lib` leg comes back on the build's own chain and carries everything; an eye's
reply starts a fresh trace and carries the three restored numbers and nothing
else. Four legs a round and three rounds, and one measured build lost the race
once — the draft came back under an empty `tool_call_id` and the tool round that
had asked for it never closed (GH #460).

So the loop writes the caller down. `weave` parks a `caller` row in the round
table on route `calls` — the one leg of a round that is always on the build's own
chain — and reads it back off the slate whenever a round is decided, newest row
first and never an empty one over a named one. From there the six keys ride the
hop of every emission, present and empty rather than absent, and each edge out of
`./weave` lifts them into `context` again. It is the same move `normalise` makes
for the digest binding, one compartment over: a value that is recomputed from
rows cannot be lost by a hop that forgets to pass it on.

**Six, because the DOOR is one of them.** `meclaw-os` stamps `build_caller` —
and `build_auto_submit` beside it — once at its rim, and the four edges out of
`./builder` read them back to decide between the operator's front door and
`./orgs`. They were lost to exactly the two breaks above, and to a third thing
besides: the row was parked only when a `build_call_id` was named, and an
operator-driven build has none — there is no agent tool call behind a person at
the front door, so the one build whose door could not be recovered any other way
was the one build that never wrote a row. Measured (GH #480): an operator build
whose composer called `graph_read` had its answer delivered into an organisation,
where it ended as `hive_no_route`; the same build calling `catalogue_lookup`
instead kept the key, because that leg travels on the build's own chain. Which
door the answer found depended on which tool the model reached for. The row is
now parked whenever the context names **any** of the six, and both door keys ride
back with the other four.

The severity is the draft lane rather than the error lane: an operator build that
consults the graph and then *succeeds* delivered its manifest into an
organisation — and in a fresh colony `./orgs` is empty by construction, so that
is a silent dead letter, the exact failure GH #469 was opened to remove. What
this repair did **not** reach was the fast lane, and § *The fast lane writes its
own two rows* below is what it took to close that half.

`orig_request` stays in the row and never reaches the hop. It is prose, a header
is not where prose belongs, and the only cell that reads it —
`builder-librarian/retrieve` — re-sets it from its own `hop.orig` on every
lookup. The row is the record; the hop stays short.

Pinned by `crates/meclaw-cells/tests/gh460_the_caller_survives_a_lost_race.rs`,
which lets the wrong eye win, and by
`crates/meclaw-cells/tests/gh480_an_operator_keeps_its_door_across_a_trace_break.rs`,
which breaks the chain both measured ways and checks that the names the builder
restores are the names the shell guards on.

## A refusal comes back, twice at most

A draft refused at the mutation door returns to this hive on `in_receipt`, and
the code is NAMED in the turn the composer sees: `requirement_missing`,
`scope_out_of_bounds`, `schema`. A refusal a model cannot name is one it cannot
repair. After `BUILDER_MAX_REPAIRS` the build stops on the `error` lane carrying
that same code — never as silence, and never as an empty manifest.

**The attempt count is counted, not carried, and that is not a detail.** A
receipt arrives on a foreign message chain — the submitter parked the build and
popped it from its OWN store — so `context` does not survive the trip and there
is no counter riding along to increment. What bridges it is a row: `normalise`
writes a binding line when it mints a digest (`manifest_sha256 → build_id`), and
`weave` adopts the receipt over that line and then counts the `receipt` rows the
transcript holds for that build. The repair edge reads the resulting `hop.repairs`
and refuses to fire above the cap. A number that is recomputed from rows cannot
be lost by a hop that forgets to pass it on.

**And so is the ROUND count, since GH #501.** It was not, and the omission was
worth a whole second budget: `context.iter` is what the loop's cap reads, the
receipt lane arrives without it, and a repair stamped with a zero re-entered a
loop whose only guard is `int(context.iter) < 6` — on a wish whose briefing had
named six rounds and no more. The slate carries an `iter` on every row of the
build, so the read-back takes the highest of them as the round the build reached
and stamps the repair with that. Only there: on the loop's own chain the context
*is* the counter, and it is the one that is right, because the slate may hold
rows of a round that has not been decided yet.

### The fast lane writes its own two rows

A recipe asks no model, so nothing on that lane ever opened a round, so nothing
was ever written down. A refusal to a model-free build therefore reached `weave`
with a digest nobody in the hive claimed and left again as `build_unknown` — into
`./orgs`, which in a fresh colony is empty by construction. The named refusal
(`template_missing`, measured) was a silent dead letter, and the door it should
have gone back to was on the message that opened the build (GH #480).

`recipes` now writes the same two rows the design lane writes: the caller, and
the digest binding that says which build owns those bytes. Two decisions carry
it:

- **The build id is the digest.** A fast-lane build has no other identity — no
  composer opened a round to mint one — and the digest is the one handle that
  survives the submitter's chain, because the submitter parks and pops a
  submission under it.
- **A model-free build gets no repair round.** That was the open question, and
  this is the answer: a recipe is a pure function of the wish. There is no
  composer behind it, no thread to hand back and no question to re-ask, so a
  repair round would call a model that never ran to re-derive bytes that were
  already determined. The caller row carries `build_lane: "recipe"`, `weave`
  reads the mark on the read-back, and the refusal takes the short road instead —
  named on the `error` lane, at the door the build came from.

A **refused** recipe writes nothing: a build that produced no bytes has no digest
to be refused under later, and a row nobody can reach is a slate that only grows.
The binding travels on its own `bind` route rather than on `recipe`, because both
`./recipes -> .` edges match on `hop.operation == 'recipe'` and a binding wearing
that operation would leave the hive as a second answer to the same wish.

Pinned by `crates/meclaw-cells/tests/gh480_a_model_free_build_keeps_its_door.rs`
and `crates/meclaw-cells/tests/gh501_a_repair_does_not_restart_the_round_budget.rs`.

### Nothing this hive remembers about itself leaves it

`params.ports` is `[]` and the rim is where the hive's own memory ends. Seven
context keys are set on interior edges — `agent`, `build_id`, `build_op`,
`build_scope`, `lib_call_id`, `repairs`, `store_origin` — and every one of the
seven `X -> .` edges clears them with `delete_context`. Context is persistent for
the life of a chain and nothing removes it on its own, so a marker that survives
an exit edge rides on the caller's next message, where a cell that reads the
marker *before* it reads its inbound lane answers this hive's echo (GH #481,
GH #490, and structurally the whole library — GH #494, closed for this template
and `builder-librarian` as GH #499).

Three keys are deliberately not on that list, and each for a reason somebody else
reads them:

- `build_caller` and `build_auto_submit` — `meclaw-os` stamps them at its rim and
  its four `./builder -> X` edges decide the door on them.
- `build_call_id` — `templates/tools` sets it on its own exit edge and
  `tools/build-draft` and `tools/build-apply` read it back off `context` when the answer
  returns through their `in_build_result` door. Clearing it here would leave the
  assistant's build tool call open forever.

The rule is `templates/README.md` § *The hive boundary*, authoring rule 5; the
sweep that measures it is `workshop/tools/hive_context_sweep.py`, and
`crates/meclaw-cells/tests/gh494_no_interior_marker_leaves_a_hive.rs` holds it for
every shipped hive.

## The digest, and why it exists

The draft travels down into a chat as a `tool_result`. A human reads it. The
model then repeats it in the second tool call, the one that submits.

A model that reformats, reorders or quietly drops a declaration on the way
produces a manifest that LOOKS like the one that was approved. The digest is
taken over the canonical bytes of the declaration list — keys sorted, no spaces,
no ascii escaping — and travels with the draft, so the submitter can refuse a
changed manifest **by name** instead of applying it by luck.

The same two functions live in three shipped scripts (`recipes`, `normalise`,
and the submitter), because a `code` cell has no shared helper. They sit between
the markers `# --8<-- digest-helper` and `# --8<-- end`, and
`gh425_the_digest_is_one_definition` compares them byte for byte off the tree:
a copy that drifts turns the integrity check into a coin flip that always says
no.

## What it is not

- **Not an applier.** See above; it is a fact about the files.
- **Not an approver.** Whether a draft may be applied is decided where it is
  submitted, not where it is drawn.
- **Not a second corpus.** Retrieval is `builder-librarian`, referenced.
- **Not transactional.** A manifest rolls FORWARD and stops at the first
  refusal, with no rollback. That is why the ORDER of the declarations a recipe
  renders is semantics rather than taste.
- **Not a writer.** No disk, no rescan, no staging. A reusable template becomes
  a declaration in the manifest, not a side channel.

## Configuration

| Variable | What it is |
|---|---|
| `MODEL_BUILDER` | the model the composer asks |
| `LOCAL_LLM_BASE_URL` | the OpenAI-shaped endpoint it asks at |
| `LOCAL_LLM_API_KEY` | the credential, if the endpoint wants one — empty means absent, and no `Authorization` header is sent (GH #271) |
| `BUILDER_LIBRARIAN_TOPK` | how many corpus chunks the briefing carries (the librarian's own knob) |
| `BUILDER_LIBRARIAN_ROW_CHARS` | how much of one corpus chunk the briefing carries; a chunk that does not fit is cut on a word boundary and says so (the librarian's own knob, defaults beside it) |
| `BUILDER_LIBRARIAN_CATALOGUE_CHARS` | the same window for a CATALOGUE row, which is wide enough that a template's row -- its contract, its params and its worked example -- travels whole (the librarian's own knob) |

## One walk of the whole lane, with a real model

The composing lane has been walked end to end against a live model rather than a
stub: one sentence naming three stages went in, a draft with a digest came back,
the requester repeated it verbatim, the submitter had it checked and carried it
to the mutation door, and the subtree it declared was read back out of
`/colony/graph` afterwards. The scenario case that performs it names the model it
used, and the model was a free one — the walk cost nothing.

What that shows is that the lane carries a manifest a model wrote. It is one
walk, and the three limits it exposed belong beside it:

- **It was a template instantiation, not a composition.** Asked for a collector,
  a summarizer and a store, the model answered with one `add_nodes` naming a
  shipped composite that already carries those roles. A cheaper answer, and a
  legitimate one — but not evidence that the composer builds cells.
- **What it grew was an island.** `add_nodes` without a crossing `add_edges`
  produces a subtree that is registered and wired inside itself and that nothing
  can reach: activation is edge-derived, and the manifest declared no crossing
  edge. A build order that wants a reachable structure has to say where it
  attaches.
- **It is not a rate.** On the free tier the walk succeeded and then failed
  repeatedly, every time on the manifest the model produced —
  `no_manifest_in_answer`, `template_missing`, `declarations_not_a_list`,
  `schema` — and never on the lane itself. The case is therefore gated behind an
  explicit `model-dependent` class: a red run of it is a statement about the
  model, not about the substrate.

### And one walk whose structure lives

The first two of those three limits were properties of the free model's answer,
not of the lane, and the same walk on a capable hosted model shows it. On
2026-08-27 the case was run against the model the private colony uses as its
brain, on a throwaway colony, and the manifest it wrote was **applied and came up
alive**: one declaration, four units placed under `/os/orgs/acme/apps`, three
edges crossing between them, every one of the seventeen resulting cells `active`
in the registry — a composition, not a composite instantiation, and not an
island. The declaration carried the `ctx` its templates demanded.

That took three fixes over three rounds, and each one moved the failure one level
up rather than removing it: the entry-shape grammar (three `schema` refusals
before it), the composer's `max_tokens` (a reasoning model spent 2048 on thinking
and returned an empty answer), and the catalogue contract line
(`requirement_missing`). The whole measurement is written up in
`CHANGELOG.md` § 0.26.0.

What it still does **not** show is template CHOICE. The model reached for
`research-assistant` as a collector and `submit` as a store — names that read
right and describe something else — and it wrote its `ctx.model` as an invented
literal rather than one it had been given. The structure lives; whether it is the
structure the sentence asked for is a different question, and an open one.

### The two caps the composer runs on

`compose` declares three numbers that belong together, and reading any one of
them alone gets the lane wrong:

| | value | why |
|---|---|---|
| `params.external_timeout_ms` | 170 000 | the operation timeout (A): how long ONE model call may take before the cell emits a clean `provider_timeout` and carries on |
| `cell.message_timeout` | 240 000 | the substrate backstop (B), 70 s above A so that A always fires first — `docs/meclaw-overview.md` § Timeouts, "B generous, A precise" |
| `params.max_tokens` | 32 768 | the completion budget, and **reasoning spends it too** |

The two caps were both measured, and both measurements first looked like
statements about the model.

**B was missing entirely.** Until `builder@1.0.3` this file declared A and said
nothing about B, so B fell to the colony default of 60 s and killed the cell
before the 170 s call it was supposed to outlast. A run series over 34 builds
(`CHANGELOG.md` § 0.27.0) read that as "high
reasoning breaks the builder": `reasoning_effort: high` died at the backstop in
5 of 6 runs, `low` in none of them, for the plain reason that reasoning depth
lengthens exactly the call the backstop was cutting. Nine more shipped `llm`
cells carried the same inversion; the gate that now forbids it is
`crates/meclaw-cells/tests/a_shipped_llm_backstop_outlasts_its_own_call.rs`.

**`max_tokens` is a budget the thinking eats first.** On the OpenAI-compatible
wire this cell speaks, reasoning tokens are completion tokens: they are billed
as `completion_tokens`, they are itemised under
`completion_tokens_details.reasoning_tokens`, and they count against
`max_tokens` — so a model that thinks past the cap returns
`finish_reason: "length"` with an **empty** answer rather than a truncated one.
That is not inferred, it is what the runs returned: 2 of 4 `effort: high` runs
came back at exactly 8192 completion tokens with nothing written, while the same
lane without a reasoning field spent 679–2119. 2048 was raised to 8192 for this
reason once already and was still not enough, and 16 384 — twice the budget that
was measurably exhausted — left roughly 6 000 tokens of answer under the largest
observed reasoning spend.

**Since `1.2.0` it is 32 768**, and the reason is arithmetic rather than
appetite. The wish this template exists for is now a whole agent: an
organisation, a person, a generation and a channel is four declarations and
**40** edges, and an edge with a guard and a modifier costs 60–120 tokens of
JSON. That is 4 000–5 000 tokens of answer before a single word of prose, on top
of a reasoning spend that has been measured at 8 192. The old cap left the
composer writing the last third of a manifest under the same condition that
produced the empty answers: `finish_reason: "length"` and nothing usable.

Rendering the edges instead of generating them is the structural half of the
same fix — a wish that takes the `grow_level` lane spends **zero** completion
tokens on those 40 edges — but the design lane still exists precisely for the
wish that is not exactly one recipe, and that is the lane this cap protects.

It is a cap, not an allocation: a lane that does not think does not spend it, and
the runs that answered cost the same as before. The operation timeout is
unchanged at 170 s and stays the binding constraint for a maximum-length answer,
which it already was at 16 384.

There is deliberately **no environment knob** for either. Both are properties of
what this cell does, not of where it runs; an operator who needs different ones
overrides the `params` of the instance, which is what `params` are for.
