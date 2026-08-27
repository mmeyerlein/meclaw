# `builder@1.0.3`

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

- **No cell in this template has an edge onto `/colony/*`.** A cell emission is
  routed over the SENDER's out-edges — `target` on an emission is a diagnostic,
  not an address — so a cell with no such edge cannot reach the mutation door
  whatever its script does.
- **The edge cannot be added later either.** `/colony/mutations` is not among
  the endpoints a mutation may draw (`MUTATION_DRAWABLE_VIRTUAL_ENDPOINTS` holds
  `/colony/graph` and `/colony/ledger`, on every scope). The one edge onto the
  mutation door lives in the BIRTH topology, and it belongs to the submitter.
- **Both facts are measured**, off this tree, by
  `crates/meclaw-cells/tests/gh425_the_builder_cannot_reach_the_mutation_door.rs`
  and, at runtime, by the scenario case `I2`.

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
| `compose` | `llm` | The one model call of the design lane. |
| `normalise` | `code` | Reads the answer instead of forwarding it: extraction, shape, diff keys, control plane. Stamps the digest. |

## Lanes

| Direction | Lane | What travels |
|---|---|---|
| in | `in_build` | a structural wish. It carries no promoted identity: this hive never emits a mutation, so it has nothing to attribute, and an edge modifier cannot reach the envelope where the only real identity lives |
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
