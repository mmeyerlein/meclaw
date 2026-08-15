# The inline extraction contract

The hive has two extraction ingresses. The **batched** one prompts its own extractor and
carries its contract in `extract-glue`'s `build_instructions()`. The **inline** one takes a
finished block from a front-line model that extracted while it was answering, and it prompts
nobody: the contract of that ingress lives in the front model's persona, outside this hive and
outside this repository.

That asymmetry is what GitHub #53 was. The batch prompt was sharpened until it extracted world
state only; the inline contract was never sharpened at all, because there was no inline contract
to sharpen -- every consumer wrote its own, and the two ingresses write into one table.

So the hive ships it, for the same reason it ships
[`predicate-core.json`](predicate-core.json): a vocabulary each extractor invents for itself is
a vocabulary nothing can hold to account, and an extraction discipline each persona invents for
itself is the same thing one dimension over.

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

Paste this block into the persona of any model that emits inline extraction -- as the
`description` of its `remember` tool if it has one, otherwise into its instructions. It is
the same discipline the batched extractor is given, phrased for a model that is standing in
the middle of a turn rather than reading a list of finished ones.

```text
AFTER your answer -- never before it, never instead of it -- emit the durable memory this
turn carries. Call `remember` with the facts if you have that tool; otherwise end your
answer with the same JSON:
{"episode_id":"<the id of the turn you are answering, ONLY if you were given one>",
"facts":[{"subject":"","predicate":"",
"claim":"","fact_kind":"world|experience|foresight","valid_from":"<RFC3339 or null>",
"valid_until":null,"confidence":0-100}]}
fact_kind: world = state of the world, experience = what happened between us,
foresight = something expected or planned.

WRITE THE ANSWER FIRST. The order is not a formatting preference: a model that produces
its structured field before its prose answers out of nothing, having done its thinking
nowhere. Say the thing, then remember it.

NEVER INVENT AN `episode_id`. It is a key the memory mints, not a name you choose. If you
were not handed one, leave it out -- the memory knows which turn you are answering and
binds your facts to it. An id you guessed files them against somebody else's turn.

WHAT YOU EMIT IS A CANDIDATE, not a verdict. Something slower than you reads these later
with the whole memory in front of it: it merges the spellings of one relation, decides
which statement replaced which, and closes what is over. So you may be generous, and you
may not do any of those three. In particular, never end a fact: leave `valid_until` empty
unless the TURN itself named an end date that has not passed yet.

You extract WORLD STATE. The turns themselves are ALREADY stored as episodes, so that
something was asked, answered, mentioned or discussed is never a fact. Extract what a turn
is ABOUT, never that the turn happened.

YOUR OWN ANSWER IS NOT A FACT. What you are about to say is either something this memory
already holds or something you concluded from it, and writing it back mints a second copy of
a fact that already exists -- under a new wording, therefore on a new axis, carrying the
question's own words, so the copy outranks the original in every later recall. The memory
handed you the bundle you are answering from. It does not need to be told what it said.

A QUESTION IS NOT A FACT. "Which editors did I favour in the last ten days" states nothing
about the world; it names a range and a matter, and both belong to the answer, never to a
claim. In particular, never derive `valid_from` or `valid_until` from the range a question
asks about: a fact minted that way is closed on arrival, which makes it invisible to the
as-of leg while the keyword and semantic legs still return it.

RESTATING STORED KNOWLEDGE MINTS NOTHING. A fact you read out of the memory delivered to you
is already in that memory. Only what the USER's turn carries into it is new: a value stated,
a value corrected, or a value confirmed against something you had wrong.

PREDICATES ARE KEYS, not prose: English, snake_case, lower case, no spaces. The same relation
always gets the SAME key, whatever language the turn is in. Everything the turn NAMES --
subjects, objects, values, proper names -- is copied byte for byte and never translated,
never spell-corrected.

A turn that carries no world state yields NO fact; an empty facts list is a correct answer,
and the turn still counts as extracted. Emit nothing you cannot ground in the turn.
```

## Two seams worth knowing

**`episode_id` is not decoration.** The ingress takes the queue rows of every episode a block
NAMES out of the extraction queue (GitHub #52), so the batched lane never buys a second opinion
on a turn the front model already answered. A block that names no turn covers none -- and an
empty block only counts as the verdict it is if it says which turn it is a verdict about.

**An empty block is a verdict.** `{"episode_id":"…","facts":[]}` is rejected for the fact lane,
because there is nothing to insert, and it still covers its turn. The last sentence of the
contract says so on purpose: a model that suppresses an empty block to avoid looking useless
hands that turn straight back to the batch.

**A block that names no turn is BOUND, not orphaned.** That is the tool form, and it is the
normal one: an `episodes.id` is a uuid the writer mints, so a model answering a turn has
never seen the id of the turn it is answering. Such a block therefore names none, and the
ingress resolves the turn itself -- the newest `user` episode of the session the call
travelled in, which the per-turn write lane minted while the answer was being generated.
Two consequences worth knowing before wiring it:

- **The session has to be in the context.** It travels there by itself in the `talky`
  composite (the seam edge promotes it), and without it a block cannot be bound and is
  rejected. That is the safe direction: the turn stays in the queue and the batched lane
  extracts it later.
- **The per-turn write lane has to be on.** A block whose turn is not yet an episode has
  nothing to bind to and is likewise rejected. One extraction later is a delay; a fact hung
  on the wrong turn is a defect, and only one of the two can be repaired.

## What the ingress enforces, whatever the persona says

The contract lives in a persona this hive does not own, so the two damages it was written
against are also stopped mechanically at the ingress. Both are candidate-stage rules --
they shape what is written, they never decide anything:

| rule | why it cannot be left to the prompt |
|---|---|
| the written predicate is folded to its KEY form (lower case, snake_case, camel case split) | `Favorite Color`, `favorite color` and `FavoriteColor` are one relation; a model inside one turn cannot see the spellings the axis already carries. Synonyms are NOT merged here -- that needs the whole axis and belongs to the night |
| a `valid_until` already in the past is dropped, the fact is kept | a validity taken from a question's range is closed on arrival: invisible to the as-of leg, visible to keyword and semantic. The measured form of damage 2, and the only one that cannot be seen from outside |

## Consumers

- Front-line personas that wire the `inline-extraction` port (see [README](README.md) § Ports).
  This file is the authority; a persona carries a copy of the block. The `talky` composite
  documents the tool schema and both edges in
  [`../talky/README.md`](../talky/README.md), section "The memory tool `remember`".
- `extract-glue` -- validates what this contract asks for, through the same validator the
  batched extractor's payload passes, covers the episodes a block names, and binds the ones
  that name none.
- `crates/meclaw-cells/tests/f9_inline_contract.rs` -- the drift lock, in both directions: a
  discipline the batch prompt states must be in this block, and a discipline this block states
  must still be in the batch prompt. Two sentences are shared byte for byte and are compared as
  bytes; the rest are two phrasings of one rule and are probed per lane.
- `crates/meclaw-cells/tests/w10b_inline_gate.rs` -- the tool form and the two enforced
  rules above, as the quality gate the inline lane needs and the invariance gate cannot
  give it: predicate spread per axis, and facts closed on arrival.
- `crates/meclaw-cells/tests/w10b_remember_colony.rs` -- both of them in a running colony,
  including the half that matters most: a broken block costs the answer nothing.
