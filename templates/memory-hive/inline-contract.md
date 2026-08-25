# The inline extraction contract

This block used to ask for a **tool call**: *"AFTER your answer, call `remember` with both
parts"*. **That instruction is retracted, not quietly reworded** (owner ruling 2026-08-24,
GitHub #373; built in GitHub #379). It was measured across seven model families and it did not
hold: in the best case 44 % of turns carried the call, in most cases far less, and a completion
that mixed a sentence with an asynchronous call stranded its own round. The same rules delivered
as a fenced block IN the answer were adopted on 12 of 12 turns by every one of five models, with
zero malformed blocks. So the delivery changed and the rules did not: everything from
`DELTA, NOT STATE` down is the byte-identical text the tool form carried. What takes the block
back out of the answer is a cell, not a model -- `templates/talky/splitter`, between the brain
and the dispatcher; without the block in the instructions it is a pure pass-through. The
`remember` tool is gone from the shipped tool list.

This file also used to open by naming **two** extraction ingresses -- a batched extractor that
prompted itself out of `extract-glue`'s `build_instructions()`, and this inline one. **That
sentence is retracted, not quietly rewritten:** per-turn extraction (GitHub #298) removed the
batch lane entirely. `build_instructions()`, the `extractor` cell and the flush that fed them are
gone. There is one ingress now and this contract is it; what a turn's annotation fails to produce
is picked up at the end of the session by the close pass (Wave 5, GitHub #300), never by a second
extractor running mid-stream against the same turns.

GitHub #53 was the asymmetry between the two while both existed: the batch prompt was sharpened
until it extracted world state only, and the inline contract was never sharpened at all, because
there was no inline contract to sharpen -- every consumer wrote its own, and the two ingresses
wrote into one table. The hive ships one for the same reason it ships
[`predicate-core.json`](predicate-core.json): a vocabulary each extractor invents for itself is
a vocabulary nothing can hold to account, and an extraction discipline each persona invents for
itself is the same thing one dimension over. With the second lane gone, that reason got stronger:
this block is the whole of what the memory is told about a turn.

## What was measured

A production colony, two history questions about a preference axis: *"which editors did I
favour in the last 10 days"*, then *"which between the 5th and the 8th"*. The front model
answered both correctly out of the bundle the hive had delivered -- and then minted its own two
answers as facts through the inline block, on a fresh predicate spelling, each with an explicit
`valid_until` derived from the range the question had asked about.

Three damages out of one missing rule:

1. **Axis fragmentation.** The answer facts arrived on a new spelling, the fifth on an axis
   that already had four. Chain arithmetic runs per `(subject, predicate)`, so the genuine
   supersession chain was outranked by duplicates -- and the duplicates carry the query's exact
   tokens, so the keyword and semantic legs rank them top.
2. **Self-closing validity.** A `valid_until` taken from a question's range closes the fact on
   arrival. The as-of temporal leg never sees it; keyword and semantic still do. One statement,
   visible to some legs and invisible to others, is worse than a duplicate.
3. **A feedback loop.** Every history answer becomes new material for the next recall about the
   same axis, compounding once per question asked.

## The contract

Paste this block into the **instructions** of any model that emits inline extraction. There is
no tool alternative any more: the annotation is something the model writes, so the place it is
asked for is the place everything else it writes is asked for.

It states an **obligation**, and that is the whole change of GitHub #299: every turn is
annotated, and the annotation has two parts -- the *delta of world state* the turn carried and
the *movement of the conversation*. A turn that carried nothing is annotated as carrying
nothing; an absent block is a fault, not a modest answer, because a turn nobody annotated is now
a turn nobody extracts.

The block is deliberately short. It is carried on **every** turn and length here is paid for per
call -- so what is in it is what the ingress can actually read, and nothing else. GitHub #299 cut
it by more than half (3,302 characters to 1,573); the flip to the sidecar bought some of that
back (1,878), because a delivery the model has to build costs more words than a tool it only has
to call. The lock below keeps it under 1,900 so the sprawl cannot grow back unnoticed.

````text
ANNOTATE EVERY TURN. You have no tool for this: the annotation is part of what you write. After your answer -- always after, never instead of it -- append a fenced block that opens with ```memory, holds ONE JSON object and nothing else, and closes with ```. Write it on every turn, including the turns that changed nothing. It has both parts -- `facts`, this turn's delta of world state, and `topic`, where the conversation stands.

```memory
{"facts":[{"subject":"","predicate":"","claim":"",
"fact_kind":"world|experience|foresight","valid_from":"<RFC3339|null>"}],
"topic":{"movement":"start|continue|end","name":""}}
```

experience = what happened between us, foresight = expected or planned.
start opens a thread, end closes it by name, continue is ordinary.

A turn that carried nothing is still ANNOTATED; leaving the block out is a fault:

```memory
{"nothing_new": true, "facts": [], "topic": {"movement": "continue"}}
```

DELTA, NOT STATE: what the memory handed you is already remembered; say what this turn
CHANGED. Your own answer is not a fact, and a question asserts nothing about what it asks.

PREDICATES ARE KEYS: English, snake_case -- one relation, one key, in every language.
single (one at a time; a later replaces it): born_on, diet, favorite_color,
favorite_food, favorite_music, job_title, lives_in, partner_of, preferred_language,
pronouns, relationship_status, timezone, works_at
multi (the axis ENUMERATES; values coexist): allergic_to, dislikes, friend_of, has_child,
has_parent, has_pet, has_sibling, likes, lived_in, member_of, owns, skilled_in,
speaks_language, studied_at, visited, worked_at
A PREDICATE NAMES THE SUBJECT MATTER, NEVER THE SPEECH ACT: an intention is
`fact_kind: foresight` on the matter -- `lives_in`, never `wants_to_move_to`.

ENTITIES ARE VERBATIM: names and values are copied byte for byte, never translated or
corrected.
````

**What is not in the block, and why that is a shape decision rather than a fence.**
`valid_until` is not an emitted field: a validity taken from the range a question asks about
closes the fact on arrival, which is damage 2 above, and the one legitimate case (the turn
itself named an end date) is what the close pass and the night are for. `episode_id` is not one
either: it is a key the memory mints while the answer is being generated, so a model that names
one names somebody else's turn. The ingress supplies both. `nothing_new` appears only on the
empty form for the same kind of reason: the ingress reads it as a flag that is either true or
not there (`is_explicit_nothing`), and a `false` printed on every content block would read as a
required field that has to be got right on the turns where it means nothing.

**Three sentences left this block on purpose, named here so the removal is a retraction:**

- *"an empty facts list is a correct answer"* -- replaced by the obligation, which says the
  same thing from the other end. Permission was never the problem; the empty annotation is not
  merely allowed, it is required, and its absence is now readable as a gap.
- *"WHAT YOU EMIT IS A CANDIDATE, not a verdict"* -- it told the model that something better
  informed would redo its work. Nothing does. This is the only extractor the turn gets, and a
  model that believes it is writing a first draft writes like one.
- the block's overall prohibition framing -- the run of *never* paragraphs that made it the
  longest description the agent carried, on every turn. The disciplines that survived are
  stated as things to do; the ones that cannot be left to a prompt at all are enforced at the
  ingress instead (below), which is where a rule belongs once a prompt cannot be trusted with it.

## Two seams worth knowing

**A block names no turn, and is BOUND rather than orphaned.** That was already true of the tool
form and it did not change with the delivery; since `episode_id` left the schema it is the only
form there is: an `episodes.id` is a uuid the writer mints, so
a model answering a turn has never seen the id of the turn it is answering. The ingress resolves
the turn itself -- the newest `user` episode of the session the call travelled in, which the
per-turn write lane minted while the answer was being generated -- and takes that episode's row
out of `pending` (GitHub #52). Two consequences worth knowing before wiring it:

- **The session has to be in the context.** It travels there by itself in the `talky`
  composite (the seam edge promotes it), and without it a block cannot be bound and is
  rejected. That is the safe direction: the turn stays in the queue and the close pass
  reads it later.
- **The per-turn write lane has to be on.** A block whose turn is not yet an episode has
  nothing to bind to and is likewise rejected. One extraction later is a delay; a fact hung
  on the wrong turn is a defect, and only one of the two can be repaired.

**"Nothing" is a status, not a rejection.** `{"nothing_new": true, "facts": [],
"topic": {"movement": "continue"}}` is an *answer*: the ingress covers the turn and moves its
queue row from `pending` to `nothing` (GitHub #298). Until this wave that answer left through
the `reject` port -- the same port a payload that was not JSON leaves through -- and the row
stayed `pending` either way, so a turn nobody annotated and a turn annotated as empty were
indistinguishable. Now `pending` means exactly one thing: **nobody annotated this turn.** A
malformed block still rejects, still covers nothing, and still leaves its row `pending` for the
close pass -- a considered silence and an unreadable block are not the same event.

## What the ingress enforces, whatever the persona says

The contract lives in a persona this hive does not own, so the damages it was written against
are also stopped mechanically at the ingress. These three shape what is written; none of them
decides what a statement *means* -- that is the close pass's work and the night's:

| rule | why it cannot be left to the prompt |
|---|---|
| the written predicate is folded to its KEY form (lower case, snake_case, camel case split) | `Favorite Color`, `favorite color` and `FavoriteColor` are one relation; a model inside one turn cannot see the spellings the axis already carries. Synonyms are NOT merged here -- that needs the whole axis and belongs to the night |
| a `valid_until` already in the past is dropped, the fact is kept | a validity taken from a question's range is closed on arrival: invisible to the as-of leg, visible to keyword and semantic. The measured form of damage 2, and the only one that cannot be seen from outside |
| a `movement: "end"` naming a topic no row of this session holds open closes **nothing** | the close is scoped by `(session_id, name, still open)`, and a model inside one turn cannot see which names the session actually opened. A name that matches nothing updates no row and the topic stays open -- the safe direction of the two, because a topic wrongly still open costs the close pass one sweep, while a topic wrongly closed loses the thread |

## Consumers

- Front-line personas that wire the `inline-extraction` port (see [README](README.md) § Ports).
  This file is the authority; a persona carries a copy of the block. The `talky` composite
  documents the seam and both edges in
  [`../talky/README.md`](../talky/README.md), section "The extraction sidecar".
- `templates/talky/splitter` -- the cell between the brain and the dispatcher that takes the
  block back OUT of the answer, on lane `extraction`. Its fence grammar is the same one the
  harness measured this wording with; without the block in the instructions it is a pure
  pass-through, which is what lets a colony run this composite without any memory at all.
- `extract-glue` -- **the only ingress.** It validates the block, canonicalises the predicates,
  dedups against what the turn already carries, writes the facts, writes the `topic` row next to
  them, and marks the queue row of the turn it covered with the status its answer earned
  (`inline` or `nothing`). Nothing stands behind it any more.
- `close-glue` (Wave 5, GitHub #300 -- the lane lands later in this wave) -- the reader of what
  this contract fails to produce. At the end of a session it reads the session whole, with the
  turns whose annotation never arrived (`pending_extraction`, status `pending`) as its priority
  list, and sharpens what the per-turn pass left rough. Its blocks travel back through
  `extract-glue`, so it is a second *reader*, never a second write path.
- `crates/meclaw-cells/tests/gh299_the_contract_asks_for_both_parts.rs` -- the drift lock, one
  direction now that there is one lane: the block must state the obligation, name both parts,
  show the `nothing` form the ingress actually parses, carry the core list byte-identical to
  [`predicate-core.json`](predicate-core.json), and stay under its length bound. It replaces
  `f9_inline_contract.rs`, whose two-phrasing premise died with the batch prompt.
- `crates/meclaw-cells/tests/f8_inline_coverage.rs` and
  `crates/meclaw-cells/tests/gh298_nothing_is_a_value.rs` -- the coverage half: a turn this
  block spoke for is never extracted a second time, and a well-formed "nothing" covers its turn
  while a malformed block does not.
- `crates/meclaw-cells/tests/gh298_per_turn_annotation.rs` -- the second part of the
  annotation: the three movements, at most one topic op per block, and the name copied verbatim.
- `crates/meclaw-cells/tests/w10b_inline_gate.rs` -- the enforced rules above, as the quality
  gate the inline lane needs and the invariance gate cannot give it: predicate spread per axis,
  and facts closed on arrival. It still drives the ingress the way the tool form did, on purpose:
  the DOOR did not change with the delivery, and a colony that still sends a `remember` hop is
  still served.
- `crates/meclaw-cells/tests/gh379_the_splitter_cuts_the_sidecar.rs` -- the other end of the
  sentence: the marker this block tells a model to open with is the marker the splitter looks
  for.
- `crates/meclaw-cells/tests/w10b_remember_colony.rs` -- both of them in a running colony,
  including the half that matters most: a broken block costs the answer nothing.
