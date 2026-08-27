# `builder@1.1.0`

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
| `recipes` | `code` | The fast lane. Renders one of three predefined recipes straight into a manifest, deterministically. |
| `librarian` | `ref builder-librarian` | Retrieval over the corpus. Referenced, never copied (ADR-0011). |
| `brief` | `code` | Assembles the authoring prompt: the retrieved sections become instructions, the request stays a user turn. |
| `compose` | `llm` | The model call of the design lane — asked once per round, not once per build. |
| `dispatch` | `ref dispatcher` | Which tools did that answer ask for, and is the bundle within budget? Fans one answer out into one call per tool, referenced rather than copied. |
| `lib` | `code` | What does the corpus say? Adapts `librarian_search` and `catalogue_lookup` onto the referenced librarian and its briefing back into a `tool_result`. |
| `eyes` | `code` | What does the colony actually look like right now? Turns `graph_read` and `registry_read` into a `/colony` question and the answer back into a `tool_result`. |
| `unknown` | `code` | A tool that is not one of the four — answered, by name, with `unknown_tool`, so a round never waits for a call that will never run. |
| `weave` | `code` | Is this round complete, and what happens next? The fan-in: it counts, it adopts a refusal, and it decides between another round, a draft and a named stop. |
| `transcript` | `store` | What was said in this build so far? One row per turn, plus the binding row that lets a later refusal find the build it is talking about, plus the compare-and-set guard that lets exactly one path cross the re-entry edge. |

## Lanes

| Direction | Lane | What travels |
|---|---|---|
| in | `in_build` | a structural wish. It carries no promoted identity: this hive never emits a mutation, so it has nothing to attribute, and an edge modifier cannot reach the envelope where the only real identity lives |
| in | `in_receipt` | a draft that was refused at the mutation door, on its way back to the composer that wrote it. It carries `hop.error_code` and the digest of the manifest it refused — no identity, for the same reason `in_build` carries none |
| out | `manifest` | the draft: the declaration list, `hop.manifest_sha256`, `hop.manifest_class`, `hop.declaration_count` |
| out | `error` | a wish this hive did not turn into a manifest, named in `hop.error_code` |

A build that stops says so on a lane. Never as silence, and never as an empty
manifest — an empty manifest is a failure wearing the face of an honest answer.

## The two classes

**Fast lane** — the caller named a recipe and its parameters validate. Three
recipes ship: `rewire_edge` (remove the old edge, then draw the new one),
`add_node` (grow a cell from a template and wire it in), `attach_drain` (hang a
lane on an existing pair). No model is consulted, no network is reachable, and
the whole walk is a python start plus string work.

A recipe that is NAMED but incomplete is an **error**, never a downgrade into
the design lane. A typo in one argument would otherwise silently buy an
inference and answer a different question than the one that was asked.

**Design lane** — everything else. The corpus is consulted first, the briefing
is assembled, one model call is made, and its answer is read. Retrieval is an
ENHANCEMENT: a corpus that is down comes back marked `degraded`, the composer is
TOLD it is working without patterns, and the build carries on.

The briefing carries a **grammar** as well as a vocabulary: the diff keys that
exist, the ENTRY SHAPE of an `add_nodes` entry (`name` and `template`, both
required), the endpoint rule for `add_edges`, and the one topology rule that
decides whether what the model wrote is reachable — a unit whose edges all stay
inside it is born inactive. It sits in the prompt HEAD rather than among the
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

## The design lane is a loop, and it is bounded four times

The composer is asked, it may call one of four tools, the results come back, and
it is asked again — until it answers with a manifest or until a bound stops it.
Four bounds, and each one buys something different:

| Bound | Where it lives | Default | What it stops |
|---|---|---|---|
| `BUILDER_MAX_ITER` | the condition of `./weave → ./compose` | 6 | a model that keeps looking and never writes |
| `DISPATCHER_MAX_CALLS` | the referenced `dispatcher` | 16 | one answer asking for forty tools at once |
| `BUILDER_MAX_REPAIRS` | the condition of the repair edge | 2 | a refusal being retried forever |
| `BUILDER_ROUND_IDLE_MS` | arithmetic in `weave` | 120000 | a fan-in waiting on a result that will never come |

The substrate's own guard is deliberately out of the game: the re-entry edge
declares `restore_ttl`, so the loop pays for one round at a time instead of
fitting all of them into one budget — about a dozen routing hops per round
against a default `ttl` of 64 (`docs/store-backed-tool-loop.en.md` § *The TTL
budget of one round*). That is why the four above have to carry. A TTL death
would be silent: straight to the dead-letter queue, no `reply_to` cascade, and
the `tool_call_id` waiting in `templates/tools/build` would never close.

Three of the four are `params` of `weave` rather than environment variables, so
two builders in one colony are tuned apart and a mutation retunes one without
touching a colony-wide setting. A CEL condition cannot read `params`, so the two
bounds that live on an edge are written twice — as a literal on the edge that
enforces them and as a settings default in the cell that documents them — and
the two spellings are held to one number off the tree by
`crates/meclaw-cells/tests/builder_bounds_agree_with_their_settings.rs`.

## Four eyes, and no hand

| Tool | Answers the refusal | Reads |
|---|---|---|
| `catalogue_lookup` | `requirement_missing` | the corpus row that opens `CONTRACT —` |
| `librarian_search` | a shape written from memory | the spec, the cookbook, the rewiring doc |
| `graph_read` | the ISLAND | `/colony/graph` — activity is edge-derived, so it SHOWS |
| `registry_read` | the wrong template, named plausibly | `/colony/registry` — what actually stands where |

Each one answers a refusal this system has really produced
(`plans/welle-2026-08-27/receipts/s12-luna-run.md`). That is the difference
between this and a general harness: the vocabulary is closed, and its closedness
is a property of the files
(`crates/meclaw-cells/tests/builder_tool_vocabulary_is_closed.rs`).

There is no fifth tool that applies anything. Submitting stays with whoever
asked, under the identity the substrate stamped on the envelope — ADR-0015
decision (2), amended on 2026-08-27 only in the shape of one assertion, not in
what it decides.

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
(`requirement_missing`). The whole measurement is in
`plans/welle-2026-08-27/receipts/s12-luna-run.md`.

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
| `params.max_tokens` | 16 384 | the completion budget, and **reasoning spends it too** |

The two caps were both measured, and both measurements first looked like
statements about the model.

**B was missing entirely.** Until `builder@1.0.3` this file declared A and said
nothing about B, so B fell to the colony default of 60 s and killed the cell
before the 170 s call it was supposed to outlast. A run series over 34 builds
(`plans/welle-2026-08-27/receipts/builder-messreihe.md`) read that as "high
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
reason once already and was still not enough. 16 384 is set at twice the budget
that was measurably exhausted, which leaves roughly 6 000 tokens of answer under
the largest observed reasoning spend — a four-declaration manifest is well under
3 000. It is a cap, not an allocation: a lane that does not think does not spend
it, and the runs that answered cost the same as before.

There is deliberately **no environment knob** for either. Both are properties of
what this cell does, not of where it runs; an operator who needs different ones
overrides the `params` of the instance, which is what `params` are for.
