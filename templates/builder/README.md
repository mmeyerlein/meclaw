# `builder@1.0.0`

The intake that turns a structural wish into a **manifest** — an ordered list of
mutation declarations, ready to be submitted by whoever asked for it.

## What it delivers

A **draft**. Never an application. What leaves this hive on `manifest` is a
proposal plus the sha256 digest over its canonical bytes and one sentence a
human can read before saying yes. Whether it is ever applied is decided
somewhere else, by somebody else, under their own identity.

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
