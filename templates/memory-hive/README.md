# `memory-hive@3.1.0`

A **member's** memory as a hive of existing cell types — no new cell type, no Rust. Thirteen cells:
`store` (all durable data), `writer`, `recall`, `extract-glue`, `close-glue`, `closer`,
`dream-glue`, `dreamer`, `judge`, `cron`, `embed`, `dialectic`, `porter`.

What it delivers today (packages P2–P5 = spec phases 1–4, plus P15 = temporal truth):

- **Write path**: every turn becomes an append-only `episodes` row. LLM-free, immediate, the
  agent never waits.
- **Audience gate**: every durable row says WHO was present when it was learned, and the read
  path answers only with rows the current round could have heard. Fail-closed on both sides —
  see [The audience gate](#the-audience-gate--who-may-be-told-what-244) below.
- **Transfer**: the remembered content can leave this hive as a declared, versioned document and
  enter another RUNNING one, idempotently — see [Taking a memory out, putting it into another](#taking-a-memory-out-putting-it-into-another-243) below.
- **Recall tier 0**: a deterministic, token-budgeted bundle (active beliefs → open foresight
  facts → recent episodes). No LLM, no embedding, fixed latency.
- **Recall tier 1**: four retrieval legs — keyword (`search`), semantic (`similar`), graph
  (`traverse`), temporal (as-of `select`) — fused by RRF in code. Still **LLM-free** and
  deterministic. The message it delivers holds two documents: a bundle written for the model
  that has to answer, and the retrieval's own record beside it, in which every candidate says
  which leg found it — see [One tier-1 message, two documents](#one-tier-1-message-two-documents-gh-296).
- **Recall tier 2**: `dialectic` synthesises one answer over the tier-1 candidates with the
  source priority beliefs → facts → episodes and a **mandatory gap statement**.
- **As-of recall**: any tier can be evaluated at a past instant (`recall_as_of`) — "what was
  true in May" and "what did we believe in May" are parameters, not promises.
- **Supersession at read time (P15, narrowed by W2)**: which fact is in force is decided when the
  question is asked, on the version chain. Order alone decides only within ONE statement (a
  re-assertion of the same canonical claim); across two different values a span ends where an
  explicit closure says so, and `expired_at` is read rather than recomputed. A hit on a closed
  statement answers with the one that closed it and names the claim it replaced (`previously` in
  the bundle, the whole `history` chain in the diagnostic beside it). The
  invariant that buys: the same recall before and after a dream run returns the same candidates,
  byte for byte.
- **Time-range recall (P15)**: `recall_window_from`/`_to` turn the temporal leg from a point
  into an interval. Every version whose derived span intersects the window is a candidate of
  its own and carries the `span` it was valid for.
- **Extraction, one ingress (GitHub #298)**: the front-line model annotates the turn **in the
  answering turn itself** (`in_remember`), and that is the only lane that writes new facts
  mid-conversation. The batched `extractor`, its gate and its flush are gone — a second party
  reading the same turns a second time bought duplicates, not coverage. Two READERS stand behind
  the ingress and neither is a second write path: the night reads what is already in the store,
  and the close pass reads the session whole at its end, with the turns nobody annotated as its
  priority list. That lane is here since 3.0.0 (`close-glue` + `closer`, the `in_close_pass`
  ingress and the `close_report` exit,
  [#300](https://github.com/mmeyerlein/meclaw/issues/300), ruling Q9 of 2026-08-21) — see
  [The close pass](#the-close-pass-one-session-read-whole-gh-300) below.
  * **The annotation is an OBLIGATION with two parts (GitHub #299).** Every turn is annotated:
    `facts`, the delta of world state the turn carried, and `topic`, where the conversation
    stands. A turn that carried nothing is annotated as carrying nothing — an absent call is a
    fault, not a modest answer, because with one lane a turn nobody annotated is a turn nobody
    extracts. The queue is what makes that readable: `pending` now means exactly one thing,
    namely that no annotation ever arrived. Freshness is what the single lane buys back — a
    fact is queryable in the same turn that carried it, instead of after the gate interval the
    batch lane used to impose.
  * **One discipline, and the hive SHIPS it (GitHub #53)**: the contract lives in the front
    model's persona, outside this hive — so the hive ships it, in
    [`inline-contract.md`](inline-contract.md), the way it ships `predicate-core.json`. A
    vocabulary each extractor invents for itself is a vocabulary nothing can hold to account,
    and an extraction discipline each persona invents for itself is the same thing one dimension
    over. Measured without it: two history questions minted their own answers as facts on a
    fresh predicate spelling, each with a `valid_until` taken from the question's date range —
    closed on arrival, so the as-of leg could not see them while keyword and semantic still
    could. The block a persona pastes states what to DO rather than a run of prohibitions, it is
    carried on every single turn, and its drift lock is
    `crates/meclaw-cells/tests/gh299_the_contract_asks_for_both_parts.rs` — one direction now
    that there is one lane, plus a length bound, because what is in that block is paid for once
    per call.
  * **One turn is annotated once, and the queue says WHAT happened to it (GitHub #52, #298,
    #300)**: an annotation takes the queue rows of the turns it covered out of `pending` with a
    status of its own, and the queue therefore carries **four** values. `pending` — no annotation
    ever arrived, the one exception an operator looks for. `inline` — the front model annotated
    the turn while it was answering it and the block carried content. `nothing` — it annotated
    the turn and its answer was an honest empty one. `close` — the party that answered was the
    close pass rather than the turn itself: either one of its `close_write` blocks covered the
    turn, or its sweep settled a row of that session still sitting in `pending` when the pass
    finished. None of the three settled values is a rejection; the value says which reader
    handled the turn and how, which is what makes the per-turn contract measurable at all — the
    close pass wins over the other two, because booking a whole-session read as `inline` would
    lose exactly that distinction. The dedup cannot do this job: it compares claim bytes, and
    two models never phrase one claim identically, so a re-extraction lands on a NEW predicate
    spelling and the chain arithmetic runs on the wrong collective. A block that is not JSON at
    all, or that cannot be bound to a turn, covers nothing and leaves its row `pending` for the
    close pass — a considered silence and an unreadable block are not the same event.
- **Canonical predicates (0.2.0 P1)**: a predicate is a KEY, not prose — English, `snake_case`,
  the same key for the same relation whatever language the turn is in. The curated core
  vocabulary in [`predicate-core.json`](predicate-core.json) (29 entries, each marked `single` or
  `multi`) travels to the annotating model **inside the shipped contract block**, byte for byte
  and with its cardinality split; the axis hint the batched lane used to read out of the store
  and render into its own prompt went away with that prompt (#298), because the party that mints
  the facts now writes them from a persona this hive does not render. Reuse over minting is what
  makes the version chain fire at all, and what the ingress can still do mechanically it does:
  the written predicate is folded to its key form on arrival (lower case, `snake_case`, camel
  case split), so three spellings of one relation are one axis. Synonyms are NOT merged there —
  that needs the whole axis and belongs to the night. The counterpart rule is **entity
  fidelity**: only the predicate is canonicalised — subjects, objects, values and proper names
  are copied byte for byte, never translated and never spell-corrected, because a name the model
  "fixes" is a fact destroyed. Pinned by
  `crates/meclaw-cells/tests/extract_canonicalization.rs` (deterministic) and by the scenarios
  C1 / C2 (model, `--with llm`).
- **The extracting model closes what it can SEE (statement identity W4, GitHub #13, ruling Q2
  option C)**: a **replacement window** — the OPEN statements of the axes touched most recently,
  each with its value, the instant it started and the id it carries — is shown to the model, and
  a fact may then come back with `replaces: <id>`. That becomes exactly the closure the nightly
  judge writes: `expired_at`, `superseded_by`, `closure_source`, one `update` on `facts`. The
  point is the north star of this memory — resolve the conflict IN THE TURN — and the model that
  reads the conversation is the only party present in it. **Since 3.0.0 the window's producer is
  the CLOSE PASS** ([#300](https://github.com/mmeyerlein/meclaw/issues/300), ruling Q9 of
  2026-08-21): the batch prompt that used to render it went with the batch lane (#298), and the
  pass that reads a session whole is the one party that can see an axis whole. An ordinary
  per-turn annotation carries no window at all — a `replaces` on it matches an empty window and
  is discarded, which is the safe direction and the shipped behaviour of the inline lane.
  * **The window is the guard rail, and it is mechanical.** A `replaces` naming anything the
    window did not contain is discarded and logged; the window is parked in `scratch` under the
    round's own key and read back when the facts are written, so what is checked is what the
    model was provably shown (ruling Q2 rail 3). `expired_at is_null` in the `where` carries over
    from W3: a closure written on this lane never writes over a judged one.
  * **Being shown is EDGE TRUTH, never a body claim.** Only a block that came through the close
    lane may carry a `shown` array, and the ingress decides that from `context.close_pass` — a
    key the hive's own port edge stamps — not from anything the payload says. A front model does
    not get to declare what it was shown. An absent, unreadable or empty `shown` parks NOTHING: a
    missing window means no closure rather than an unguarded one, and a window that said nothing
    would be the second of those wearing the shape of the first.
  * **One list, by construction.** `open_axes()` in `close-glue` builds the `facts` block of the
    prompt AND the `shown` array of the write blocks out of the same rows, so "what was shown"
    and "what may be closed" cannot drift apart — the only way two lists never disagree is that
    there is only one of them. Its budget is `MEMORY_CLOSE_FACT_ROWS` over the open facts of the
    session being closed; what the page left behind is counted and stated on `close_report`
    (`truncated`) rather than guessed at.
  * **A replacement points FORWARDS in time (GH #71).** The window says which statements may be
    closed and never said which of the two is newer, and the first live sample of three such
    closures contained one inversion: a statement ended by a fact three days its senior, which
    cost the run its only wrong answer. So the apply phase compares before it writes — the
    statement being closed must not have been asserted after the fact replacing it — and a
    refused closure is not a refused extraction: the fact is minted exactly as before, both
    statements stay open, and the pair is receipted in `scratch` under the round's key
    (`kind = extract-refusals`, with both values, both instants and the reason) so the night can
    still judge it. The comparison is this lane's own recency, `(valid_from, recorded_at, id)`,
    on the assertion still standing: two statements of ONE instant still replace one another,
    because the row being closed was read out of the store before this round's clock.
  * **Prompt rules** (the P1 discipline one dimension over): replace only when the new value
    updates the SAME matter, never on an axis that ENUMERATES (a second child, one more language,
    another plan), never the statement the fact merely repeats, and when in doubt leave
    `replaces` out — a missed replacement costs one outdated line, a wrong one removes a value
    that was true. A turn that PLANS, WANTS or HOPES something about a shown statement lands on
    that same subject and predicate with `fact_kind: foresight` and an empty `replaces`: an
    intention stands next to the fact it is about, it does not end it.
  * **Receipt and revert.** The reasons of one extraction round are parked in `scratch` under
    that round's own key — the extraction lane's equivalent of the run receipt the night folds
    its closures into — which is exactly what `closure_source` names (`extract:<key>`), so a
    closed fact reads back to the turn that replaced it and one round reverts with one `where`
    (`update facts set closure_source = '' where closure_source = 'extract:<key>'`, plus the next
    re-derive). The key used to be a batch's; since per-turn extraction (#298) it is the key the
    annotation's own round trip minted, so the unit that reverts together is one annotation
    rather than one gate interval. The model's reason is structural rather than prose: the
    replacing value, the value replaced and the episode both came from.
  * **The night validates it** (guard rail 3, second half): the canonicalisation round's axis
    pages also carry statements a RECENT extract closure ended (`MEMORY_CANON_EXTRACT_LOOKBACK_DAYS`),
    and the judge may contradict one. A contradiction clears the attribution and the re-derive of
    that same night withdraws the columns — the W3 revert, one producer over. Only the closures
    this lane wrote are reviewable there: a round that could revoke a JUDGEMENT would make
    "only close, never delete" revocable by the party that wrote it. **Direction needs no
    judgement (GH #71)**: a closure whose `expired_at` lies before the statement it ended was
    written the wrong way round, and the scan of the night takes it back itself — same revert,
    same receipt key in the run's books (`reopenings`), and the closure is not put in front of
    the judge at all, because the round does not buy an opinion about a row it just cleared.
  Pinned by `crates/meclaw-cells/tests/w4_extract_replaces.rs` (free) and by scenario C14
  (free, the real window with the close pass's answer handed in, both gate halves). The paid
  twin C15 went with the batch lane it drove (#298) — a live model on this lane is measured by
  the conversation-guide harness now, not by a scenario of its own.
- **A predicate names the subject matter, never the speech act (GH #67)**: `plans_to_beat`,
  `wants_to_move_to` and `hopes_to_visit` are not relations, they are sentences about relations
  this memory already has — and a value that lands on one of them is invisible to every question
  about the matter it updates, because the currency question groups by
  `(canonical_subject, canonical_predicate)`. Measured: one store held `has_experience "setting a
  personal best time of 27:12 …"` and `plans_to_beat "personal best time of 25:50"`, two axes for
  one matter, and answered with the older value across the whole statement identity track.
  * **The intention did not disappear, it MOVED onto the statement.** `fact_kind: foresight` is
    the marker — the column the tier-0 foresight leg has always filtered on — so a plan and the
    fact it is about share an axis and the currency question can finally see them together, while
    nothing that could tell them apart before has lost the ability. The tier-0 foresight leg
    filters `fact_kind = 'foresight'` and is unchanged since P3; a tier-1/2 candidate carries
    `intent: planned` and its line reads `… (planned)`; the nightly currency question sees
    `"intent": "planned"` on the statement, and question 3 says what that means — an intention
    never closes a statement that happened and is never closed by one, while two PLANNED
    statements of an axis may still replace one another.
  * **The rule is prevention, not repair.** A `plans_to_*` key cannot be aliased into
    subject-matter form generically (the matter varies per statement, an alias is per predicate),
    so the fix is what the extracting model is TOLD (`A PREDICATE NAMES THE SUBJECT MATTER,
    NEVER THE SPEECH ACT`, with the named shapes it refuses) plus the guidance in
    [`predicate-core.json`](predicate-core.json). Per-turn extraction (GH #298) retired the
    batch prompt that used to carry that telling, and with it the subject-matter window
    selection; **the telling now sits in the shipped contract block**
    ([`inline-contract.md`](inline-contract.md), GH #299), which is the only thing the annotating
    model is handed — pinned there by
    `crates/meclaw-cells/tests/gh299_the_contract_asks_for_both_parts.rs`. Next to it
    [`predicate-core.json`](predicate-core.json) states the fate of the speech-act class and the
    `fact_kind: foresight` each example lands on. No
    speech act is seeded and none ever will be; the subject-matter
    examples there are deliberately seeded with NO cardinality, because a seeded verdict
    outranks the night's own and an over-cap axis of a `multi` relation is answered for good
    (GH #66).
  Pinned by `crates/meclaw-cells/tests/f6_subject_matter_axis.rs` (free, 8 — the marker from the
  store column through the bundle to the nightly currency question, plus the seed file). Its
  scenario C22 went with the batch lane (#298): the case's subject was the window SELECTION the
  batch prompt did, and the living half of the issue — a predicate names the subject matter,
  never the speech act — is the seed file and the test above.
- **Predicate identity in the store (0.2.0 P2)**: `facts.predicate` is what was WRITTEN,
  `facts.canonical_predicate` is what it MEANS. The store owns the second one: it derives it on
  every write from the `predicate_aliases` table (`params.canonical` binding), backfills it once
  per `cell.db` at spawn, and re-derives the whole column on demand (`canonicalize`). All three
  identity-sensitive reads consume ONLY the canonical column — FTS indexes it, the axis leg
  anchors on it, the version chain groups by it — so there is one alias-aware place instead of
  three. Nothing is rewritten and nothing is deleted: dropping an alias row and re-deriving puts
  every fact back on its original spelling. Pinned by
  `crates/meclaw-cells/tests/p2_canonical_chain.rs`.
- **Entity identity in the store (0.2.0 P4, ruling Q5)**: the same mechanism one dimension over.
  `facts.subject` is what was WRITTEN, `facts.canonical_subject` is WHO it is, derived from the
  `subject_aliases` table by the same binding machinery — so the axis the version chain groups by
  is `(canonical_subject, canonical_predicate)`, and `user` vs `user:alex` no longer splits a
  relation in half. The entity binding adds one thing the predicate binding does not have:
  `normalize`. Two spellings equal after Unicode composition, case fold and whitespace collapse
  are ONE identity at mint time, with no alias and no model — that is the only merge the store
  performs on its own, and it is provable rather than judged. Everything fuzzier than that stays
  a QUESTION: `alias_candidates` scores pairs (trigram Dice, hand-built, no extension) and hands
  them to the nightly GC, which judges with a top-tier model and persists an ordinary
  `set_alias`. Similarity never merges anything by itself — that is the safety net ruling Q2
  asks for, and it is why an unusual place name survives a store that has a similar one in it.
  Pinned by `crates/meclaw-cells/tests/p4_entity_axis.rs` and the scenario C4 (free).
- **Statement identity in the store (W2, GitHub #13, ruling Q1)**: the same mechanism a THIRD
  dimension over, and the one that changes what supersedes what. `facts.claim` is what was
  written, `facts.canonical_claim` is WHICH VALUE it is, derived from the `claim_aliases` table by
  the same binding machinery — so the supersession unit becomes the statement
  `(canonical_subject, canonical_predicate, canonical_claim)` while the axis stays the retrieval
  grouping. The binding is deliberately **not** normalising: byte identity of the claim is the
  day-one canonical value, and merging two wordings of one value is a JUDGED alias (W6, below)
  with the same `claim_rejected_pairs` refusal log the other two dimensions have. Pinned
  by `crates/meclaw-cells/tests/w2_statement_identity.rs` and
  `crates/meclaw-cells/tests/w2_statement_chain.rs`, plus the scenarios C9/C10/C11 (free).
- **Judged claim aliases (W6, GitHub #13, ruling Q1 stage 2)**: what fills those two tables. On
  the axes the currency question already shows it, the nightly judge is asked which of the open
  statements are two WORDINGS of one value; a yes becomes one `set_alias` on the claim dimension
  and the `canonicalize` the round already ends with does the rest — the two rows share a
  canonical claim, become ONE statement, and W2's re-assertion arithmetic turns the older wording
  into history by itself. Nothing in W6 writes a closure. The danger is the mirror image of the
  value, so three things stand against it: the prompt rule that **quantities, dates and sizes are
  never a rewording** (`yoga twice a week` and `yoga three times a week` are two values, and
  merging them deletes the change the memory exists to remember), the `claim_rejected_pairs` log
  that travels back into the next payload as `known_different` so a refused pair is not retried
  every night, and the revert that has been the same since P2 — one `delete` on the alias row plus
  the next re-derive. The keyword index keeps reading the **written** claim, so recall on the
  original wording survives a merge. Pinned by `crates/meclaw-cells/tests/w6_claim_aliases.rs`
  plus the scenarios C17 (free, the whole yoga form end to end) and C18 (`--with llm`, a real
  judge on a rewording pair and a quantity trap in one payload).
- **The canonicalisation round (0.2.0 P5, rulings Q1/Q3/Q5)**: the nightly run is the
  **persisting twin of the extractor**. The extractor keeps a NEW turn on the axes that exist;
  this round repairs the axes that already fell apart. Once a night, after the dreamer's
  verdicts are parked and before the supersession arithmetic, `dream-glue` reads back what it
  has already decided (the judged cardinality, and the claim pairs an earlier night refused),
  scans the store, asks `alias_candidates` for entity pairs and puts up to FIVE questions to
  `judge` in ONE payload (since GH #69 only the ones it has data for): which predicate keys name the same relation (with the subjects each key is used
  on, and the core vocabulary as the target anchor), which candidate pairs are the same entity
  (with the facts of both sides), which of the open statements on an axis are still true (W3),
  whether a relation is functional or enumerating (W5) and which statements of one axis are two
  wordings of one value (W6). One call a night, sections that give each other context: the
  currency question needs exactly the identity the first two establish, and the rewording
  question reads the same axes the currency question is built from. What comes back becomes the
  closures first, then the cardinality rows, then `set_alias` per accepted pair (all three
  dimensions), `reject_pair` per refusal, then exactly ONE `canonicalize` — in that order, so no
  reader ever sees a half-written judgement and no closure is written against an identity the
  same round has already moved. **Never a delete, never an
  edit of a written value**, so the whole round reverts with a `delete` on the alias table plus
  a `canonicalize`. Because it runs BEFORE the arithmetic, an axis merged tonight fires its
  chain tonight. Pinned by `crates/meclaw-cells/tests/p5_canonical_dream.rs` (free) and the
  scenarios C5 (free, judgement handed in) / C6 (`--with llm`, real judgement).
  * **A closure across two spellings does not hide the merge it proved (GitHub #73)**: when a
    statement written under spelling A is closed by a statement written under spelling B, A is
    open nowhere any more -- and the two identity questions used to be built from open rows only,
    so the alias that would unite the two chains was never proposed, in exactly the case where a
    closure had just proved the two spellings name one thing. The identity questions therefore
    have a **read of their own**: the closed rows, most recently ended first,
    `MEMORY_CANON_CLOSED_ROWS` of them, and up to `MEMORY_CANON_MAX_CLOSED_AXES` spellings out of
    that page are merged into the inventory and the entity context at the meeting point. Bounded
    on ROWS and never on a clock, because `expired_at` is an EVENT time -- the instant a
    statement stopped being TRUE -- and it says nothing about when the closure was written, so the
    review lane's lookback (`MEMORY_CANON_EXTRACT_LOOKBACK_DAYS`) is the wrong bound for this
    question and stays where it is. The merge is a union, so it can only ever ADD a spelling no
    open row carries; the open side keeps its slots in the entity context and a closed claim
    fills what is left. Pinned by `crates/meclaw-cells/tests/f10_closed_spelling_identity.rs`
    (free) and by C6, where spelling A is closed by spelling B and the next night proposes the
    alias.
- **The refusal is remembered (0.2.0 P5)**: a NO used to cost the same top-tier call every night,
  because only ACCEPTED pairs disappeared from the candidate feed. Each binding may now declare a
  `rejected` table (`params.canonical.rejected`); `reject_pair` upserts the unordered pair and
  `alias_candidates` excludes it. The alias table could not carry it — its `canonical` column is
  `NOT NULL`, and a NULL there would read as "resolves to nothing". Additive migration like every
  other step of this wave: an existing `cell.db` grows the table on the next wake.
- **The gate is split (0.2.0 P5, ruling Q1)**: a dream run is judged by two sets, not one.
  *Invariance set* — a question the run is not about answers **byte-identically** before and
  after it, history fields included (the P15 criterion, unchanged). *Improvement set* — the
  knowledge-update question has to MOVE toward the truth: the current value with its past
  instead of both values side by side. A regression in either half is a red run. Scenario C5
  carries both halves in one case, C6 the same against a real judgement; since W3 the same two
  halves cover the closures as well (C12 free with a mocked verdict, C13 against a real judge) —
  and the invariance half is what a closure on an uninvolved axis would break, which is why such
  a verdict has to be discarded rather than merely regretted.
- **Nightly consolidation**: delta-scoped, idempotent, supersession instead of deletion.
- **Embedding lane**: optional by construction — a dead embedder degrades to a NULL queue and
  never blocks a write or a recall.

The store query layer (P3) and the graph/vector legs (P4) carry the reads: predicates,
`order_by`/`limit`, BM25 `search`, `traverse` and `similar` all run IN the store, so the code
cells rank and fuse instead of fetching everything.

## Lanes

The hive is an **island**: instantiate `add_nodes` and the `add_edges` in ONE mutation,
otherwise the subtree stays inactive and `cron` never spawns (the island-activation pattern:
an island without a crossing edge is never woken, so the mutation that creates the subtree has
to be the mutation that connects it).

**The address is the hive; the lane is the port** (GH #197, ruling 2026-08-18). `config.json`
declares an empty port list and a contract in lanes:

```json
"params": { "ports": [], "contract": { "accepts": […], "emits": […] }, "graph": { … } }
```

A mutation whose `add_edges` reaches ANY cell of this hive from outside — `./memory/store`,
`./memory/writer`, `./memory/recall`, `./memory/extract-glue`, … — is rejected with `error_code:
"hive_port_boundary"`, pre-destructively, in either direction. There is no exception any more:
`writer`, `recall` and `extract-glue` used to be ports, which meant every caller had to know
what the inside was called and no rearrangement of the interior was possible without breaking
them. Wire the **hive path** and put the lane on `hop.route`. Inside the hive nothing changes:
the internal graph wires whatever it likes, at any depth.

**And the store is writable only from inside** (GH #132): `store/config.json` declares
`"write_surface": "internal"`, so a write op (`insert`/`update`/`delete`/`create_table`/
`set_alias`/`reject_pair`/`canonicalize`) whose sender sits outside this hive is refused with
`error_code: "write_denied"` before it reaches the database. **Reads stay free from anywhere** —
which is what keeps a debug probe straight into `./memory/store` a legitimate move. The six
writers (`writer`, `recall`, `extract-glue`, `dream-glue`, `embed`, `porter`) all live in the hive, so
nothing about the shipped topology changes; what changes is that the memory can no longer be
edited past its own lanes.

The same `config.json` also declares `"write_surface": "internal"` in its **`contract`** block
(GH #260). That is the other half, and it is a separate key on purpose: the one above bounds what
the store's own `handle()` runs, while the `transfer` body slot is answered by the substrate
**before** `handle()` — so without the contract key an `import` would write rows straight past
ruling F3. Both halves use the same owning scope, so the store still has exactly one boundary; an
`export` is a read and neither half bounds it.

| Lane | Direction | The edge must carry |
|---|---|---|
| `in_episode` | in → `./memory` | one turn to remember — **one message carries one turn and its own provenance**, which is why `speaker` is answerable at all: identity travels per message, so two turns of one session can name two different people (GH #272). `session_id`/`turn_id` are ingress context keys and travel by themselves; optionally `set_context: {happened_at: "hop.happened_at"}` for historical ingest. **Plus the provenance of the turn, and it is not optional**: `set_context: {audience_set: …, channel: …, speaker: …}` (or `agent_id` on an assistant turn). Missing `audience_set` or `channel` → nothing is written and the turn leaves on `reject` — see [The audience gate](#the-audience-gate--who-may-be-told-what-244) |
| `in_query` | in → `./memory` | `set_context: {recall_query: "hop.recall_query", memory_tier: "hop.memory_tier", recall_as_of: "''", recall_window_from: "hop.recall_window_from", recall_window_to: "hop.recall_window_to"}` — the caller must send all five keys on EVERY hop, empty string = unset (see the trap below). **`recall_as_of` has no producer in the shipped composites**: `collector/assemble`'s `emits.hop` carries `recall_query`, `memory_tier`, `recall_window_from` and `recall_window_to` and no as-of key, so an edge reading `hop.recall_as_of` would fail to evaluate and the colony would skip the whole edge. Promote the constant empty string there — a point recall — unless your own caller has a source for the instant. The `phase: "recall"` hop that starts a fresh chain is stamped by the hive's OWN door edge now, not by the caller. **Two more keys are read and neither is required**: `session_id` scopes the session read (the tier-0 leg answers with THAT session's episodes; a question naming none reads a session called `default`), and `recall_caller` is a **reply-to token this hive never reads** -- whatever a caller puts there rides through untouched and comes back on `hop.recall_caller` of the `bundle` and of a `reject`, which is how one hive serves several askers and each of them gets ITS answer ([#532](https://github.com/mmeyerlein/meclaw/issues/532)). It has to change compartment on the way out, and this hive is the only place that can do it for every caller at once: on the hop it would not survive, because the `recall` cell forms its own hop and only context travels (GH #411), and a door guarded on context ALONE is condemned by `crates/meclaw-cells/tests/gh173_shipped_hive_contracts.rs`. **Plus the asking round**: `audience_now` and `channel` are required, `channel_open_history` is optional (default closed); without the first two the question is refused rather than answered unfiltered |
| `in_remember` | in → `./memory` | the same `audience_set` and `channel` as `in_episode` — this lane mints facts directly, so it is the one place a missing audience would produce an untagged row with no episode to refuse it first. Beyond that, nothing but the block itself: the door stamps `store_origin`/`mem_phase` inside. The block form comes from [`inline-contract.md`](inline-contract.md) and is delivered by the front model's own composite rather than by its persona (`talky`'s collector writes it into the brain on every assembly, GH #525), and the block has TWO parts (#299): `facts`, the delta of world state the turn carried, and `topic`, where the conversation stands — a `topic` with `movement: "start"` or `"end"` writes the topics row next to the facts, `continue` writes none, and a turn that carried nothing still sends `{"nothing_new": true, "facts": [], "topic": {"movement": "continue"}}` rather than nothing at all. The caller's `session_id` must be in the context (it is, in the `talky` composite): a block that names no episode is BOUND to the newest `user` turn of that session, and one that arrives without a session cannot be bound and is rejected |
| `in_close_pass` | in → `./memory` | a session that just ended, to be read WHOLE once (GH #300, ruling Q9 of 2026-08-21). **Nothing travels in the body** — the lane names a session and the hive reads its own turns. The context is the same provenance every write lane of this hive demands and is not optional: `set_context: {session_id: …, audience_set: …, channel: …}`; the pass proposes writes, so a pass without them would mint sharpened rows nobody can filter afterwards. The shipped caller already has all three — `talky`'s `./session-keeper → ./collector` close edge promotes exactly this set. Wire `close_report` in the SAME mutation. The pass costs one strong-model call per session — see [What a close pass costs](#what-a-close-pass-costs-measured) |
| `in_export` | in → `./memory` | nothing. The lane names the whole memory; the hive walks its own tables and answers with one part per table on `dump` (see [Transfer](#taking-a-memory-out-putting-it-into-another-243)). Wire `dump` in the SAME mutation — `required_drains` enforces it, and an export nobody drains reads the whole store for nothing |
| `in_import` | in → `./memory` | ONE part of such a document, as the body of the message; nothing on the hop and nothing in the context. Applying the same part twice leaves the same state. A part whose declared schema lost `audience_set` or `channel` is refused on `reject` with nothing written |
| `bundle` | out → your consumer | condition `hop.route == 'bundle'` on an edge FROM `./memory`. It carries `hop.recall_caller` back, off the `context.recall_caller` the question came in with and empty when it came with none — a caller with more than one asker routes the answer on it, and one with a single asker ignores it ([#532](https://github.com/mmeyerlein/meclaw/issues/532)) |
| `close_report` | out → your drain | condition `hop.route == 'close_report'` on an edge FROM `./memory`. **Drain it.** It is the ONLY positive signal the close lane has — the pass writes through the inline ingress, which answers nobody, so without this drain a caller cannot tell a pass that ran and changed nothing from a pass that never ran at all. Eight numbers ride on the hop: `added`, `sharpened`, `corrected`, `closed`, `restated`, `unseen_refs`, `exceptions` (the `pending` rows of this session the pass swept) and `truncated` (what the page bounds left behind). A pass that got no verdict leaves on `reject` instead, with `hop.reject_reason == 'closer_failed'` — nothing was written and the exception list was NOT swept |
| `dump` | out → your drain | condition `hop.route == 'dump'` on an edge FROM `./memory`, and make it a PLAIN one: an edge that also tests `hop.dump_kind` evaluates to `false` under the `required_drains` probe and reads as no drain. `hop.dump_kind` tells the two payloads apart — `export_part` (one part of the document, `hop.export_part` of `hop.export_of`, `hop.export_final == '1'` on the last) and `import_receipt` (`hop.rows_written` for one applied part) |
| `reject` | out → your drain | condition `hop.route == 'reject'` on an edge FROM `./memory`. **Drain it.** `hop.reject_reason` names the case: `missing_audience` and `missing_channel` for a turn, block or question whose provenance was incomplete (#244), `inline_invalid` for a block that did not survive validation. The transfer lane adds `import_format`, `import_unknown_table`, `import_schema_drift`, `import_probe_failed`, `import_write_failed` and `export_read_failed`, and it reuses `missing_audience`/`missing_channel` for a document part that lost a provenance column on the way. Beyond those, two older things arrive here and the body says which: an inline block the hive could not bind, and a HALF window (exactly one of `recall_window_from`/`_to` non-empty), which is a caller bug and leaves at request entry before the leg fan. Undrained, a refused block is an unrouted dead end — nobody ever learns the memory was not written — and a refused question leaves the caller waiting for a bundle that never comes. A colony that ran the inline lane for weeks with only the recall half drained is where that lesson comes from. **Since 2.3.1 the same lane also carries what this hive's own STORE would not do** (`hop.reject_reason == 'store_refused'`, `hop.store_error` = the store's `error_code`, `hop.store_operation` = the op it refused): a read or a write that came back refused stops its lane there instead of being read as zero rows. The nightly consolidation reports here too -- it has no caller of its own, and the alternative was reporting nowhere. See [When the store says no](#when-the-store-says-no-gh-343-since-231) |

**The drain is enforced, and it is enforced in lanes** ([#237](https://github.com/mmeyerlein/meclaw/issues/237)).
`params.required_drains` used to pair a PORT with the route it must drain, and it fired when
something outside wired that port — which a sealed hive has no way of letting happen, so the
declaration could never fire again and was removed with the seal rather than left as decoration.
It is back in the vocabulary the seal left standing, and this hive declares nine entries:

```json
{"accepts": "in_episode",    "emits": "reject",       "because": "…"}
{"accepts": "in_remember",   "emits": "reject",       "because": "…"}
{"accepts": "in_query",      "emits": "reject",       "because": "…"}
{"accepts": "in_close_pass", "emits": "close_report", "because": "…"}
{"accepts": "in_close_pass", "emits": "reject",       "because": "…"}
{"accepts": "in_export",     "emits": "dump",         "because": "…"}
{"accepts": "in_export",     "emits": "reject",       "because": "…"}
{"accepts": "in_import",     "emits": "dump",         "because": "…"}
{"accepts": "in_import",     "emits": "reject",       "because": "…"}
```

Read as: *a caller that sends me `in_remember` must subscribe to `reject`.* A mutation that wires
either ingress without the drain comes back `required_drain_missing` and changes nothing; the
hive's own sentence travels into the refusal. The boot path only WARNS — a birth topology is
authorship, and a colony that cannot boot is worse than one that says so in its log.

One limit, stated so nobody meets it as a surprise: the check probes your subscription with the
lane alone. An edge that tells lanes apart by a second `has()`-guarded hop key evaluates to a
clean `false` under that probe and is read as no drain. Give the reject lane an edge of its own.

`recall_query`, `memory_tier`, `recall_as_of`, `recall_window_from`, `recall_window_to`,
`happened_at`, `store_origin` are **not** ingress context keys (that list is closed: `turn_id`,
`session_id`, `user_id`, `chat_id`, `locale`). They are promoted from `hop` by the caller's edge onto
the hive — the `rag_question` pattern.

**A second consumer is where the hive's own bookkeeping starts to travel** (GH #152).
`mem_phase` and `recall_id` belong to this hive and are *persistent* context: once a consumer
has asked once, they ride along in everything that consumer emits afterwards — including an
errand it hands to a **second** agent, whose collector then asks this hive with a phase it never
set. The request entry recognises that case by the hop the hive's own door edge stamps
(`phase: "recall"`) and starts a fresh chain regardless of what the context carried, so a caller
does not have to know about keys it does not own. Since GH #197 that stamp is genuinely the
hive's business: the door `. -> ./recall` sets it, and no caller can get it wrong. **Nothing is
required of the caller here** — but if you want to be explicit, `delete_context: ["mem_phase",
"recall_id"]` says the same thing at the wiring level. Before the fix this was a *silent* stall:
the request parked, the caller waited for a bundle that never came, and there was no error, no
dead letter and no log line to find.

**Trap worth knowing:** a `set_context` whose CEL expression reads a hop key that is *absent*
fails to evaluate, and a failed modifier makes the colony **skip the whole edge**. Any optional
hop key must therefore always be present (empty string = "unset"), or the modifier must not
mention it at all. Both P5 lanes hit this: the `happened_at` probe emits the key unconditionally,
and the `embed → recall` edge deliberately does not re-promote `recall_id` (context is persistent
anyway).

**That trap is the whole contract of the two window keys.** `recall_window_from` and
`recall_window_to` are optional in MEANING, never in PRESENCE: a `set_context` whose CEL
expression reads a missing hop key fails, and a failed modifier makes the colony skip the whole
edge — so the caller must ALWAYS send both keys, if need be as an empty string. Omitting them
does not fall back to a point query; it silently drops the entire recall request. Empty +
empty = point query on `recall_as_of` (or "now"), both filled = interval, exactly one filled =
`hop.route == 'reject'` out of the hive on the `reject` lane. Both filled **on a tier-0 request** is
the one case the lane cannot answer: the bundle comes back on the `bundle` lane as usual
and says so, with `hop.window_ignored = "1"` and a `window_ignored` block in the body (0.2.0 P7).

**Who derives the window is CURRENT DESIGN, not a settled boundary.** The rule above — the hive
does not guess, the consumer derives the window — is deliberate and it is also the reason the
window machinery has, so far, no caller: a consumer that pins both keys to the empty string runs
every question as a point recall, including the explicit time-range ones the window was built
for. Moving the derivation to the hive side is tracked in
[#55](https://github.com/mmeyerlein/meclaw/issues/55); the keys, the reject rule and the tier-0
notice are unaffected either way.

## The close pass: one session, read whole (GH #300)

Per-turn annotation is blind in one direction: the front model annotates the turn it has just
answered and cannot know the turn after it. So a value the conversation only settled at the end, a
correction three turns later and a claim the session itself retracted are all invisible to it. The
close pass is the answer, and ruling Q9 (2026-08-21) decided its shape: **one strong-model call
per closed session, reading the session whole**, not a cheap sweep and not a second continuous
writer.

Send a session to `in_close_pass` when it ends. Nothing travels in the body — the lane names a
session and the hive reads its own turns. Inside, `close-glue` reads four sets out of its own
store (the turns, the records those turns left standing, the topics still open and the
`pending` rows nobody annotated), parks them, renders one prompt, and `closer` — the hive's
`MODEL_CLOSER` slot — answers with a verdict. The verdict costs one extra store round trip on
purpose: a `code` cell is stateless, and every one of the four points needs something only the
READ phases saw, so the verdict parks itself next to those four sets and is read back with them.

**Four points, and they are obligations** (they are the prompt's own words, and each is checked
mechanically afterwards):

1. Add only what is missing.
2. Correct only by superseding the record you name.
3. A sharpening points at a record and never replaces it.
4. Do not restate what is already there — "nothing to add" is the expected answer for a
   well-annotated session.

Point 4 is checked **twice**: here against the session's own open records, and again at the
ingress, whose `claim_hash` dedup drops a restatement whatever this pass believes about it. The
turns marked `"annotated": false` are the priority list — the exception rows of
[the queue](#idempotency) — and a pass that reaches its verdict sweeps them; a pass that does
**not** reach one leaves them exactly where they were, because a turn nobody looked at must not
be booked as looked at.

Everything the pass writes goes out through the ordinary inline ingress — there is no second
write path in this hive. What separates it from a front model's annotation is one context key the
hive's own port edge stamps (`close_pass`), and that key buys exactly one privilege: the block may
carry a `shown` window, so its `replaces` can be honoured — see the replacement window in the
opening list above.

What the pass did leaves on `close_report` with eight numbers on the hop (`added`, `sharpened`,
`corrected`, `closed`, `restated`, `unseen_refs`, `exceptions`, `truncated`). **Drain it**: the
writes answer nobody, so without the report a caller cannot tell a pass that ran and changed
nothing from a pass that never ran at all. A pass with no verdict — the call errored, the answer
was not JSON, or the parked verdict was gone — leaves on `reject` with
`hop.reject_reason == 'closer_failed'` and writes nothing.

Nothing here forbids closing one session twice. A second close hands the model the same turns
with the facts of the first already in the record set, so point 4 plus the ingress dedup make it
cheap and harmless rather than duplicating — but it is a whole model call, and no guard exists.
Left unguarded deliberately for this release; if a real topology produces double closes, a
`close:<session_id>` marker in `scratch` is one op.

### What a close pass costs (measured)

**≈ 0.077 EUR per closed session**, on `anthropic/claude-opus-5` with
`MEMORY_REASONING_CLOSE=medium`. That is a measurement, not an estimate: three full runs of the
conversation-guide harness on 2026-08-23 (26 turns each, one close each) cost 0.0774 / 0.0768 /
0.0766 EUR for their close call, priced from `scripts/prices-openrouter-2026-08-22.json`. Over
those three runs the close call was **0.2308 EUR of 0.2899 EUR — about 80 % of everything the
colony spent**, front model, tool loop and all. The cost class follows from the shape rather than
from the model: one call, the whole session in the prompt, a strong model by ruling. Budget one
such call per closed session and size `MEMORY_CLOSE_TURN_ROWS` / `MEMORY_CLOSE_FACT_ROWS` knowing
that both bound what the prompt pays for.

## The audience gate — who may be told what (#244)

Every durable row of this hive says **who was present when it was learned**, and the read path
answers only with rows the current round could have heard. The rule and its vocabulary are the
ones `affinity` already uses (`member:<name>`, `agent:<name>`, `*` for universal); the two halves
of one rule speak one language on purpose.

**One name for the round, one minter for the names inside it** ([#330](https://github.com/mmeyerlein/meclaw/issues/330)).
The round is `audience_set` everywhere -- on the hop, in the context, in the column -- and
no template may ever introduce a second name for it: a second spelling is a second gate
that can stand open while the first one reads shut, which is exactly how `affinity` spent
a release reading a key nobody wrote. The references inside the set are `affinity`'s alone
to mint and map; this hive stores the string it was handed **byte for byte** and never
looks an identity up. A subset test over two vocabularies is not a test, it is a
coincidence -- so translating a connector's own user id into that vocabulary happens once,
on the talky edge (ADR-0002 E8), and nowhere else.

The reasoning behind each decision is recorded on
[GH #244](https://github.com/mmeyerlein/meclaw/issues/244) and, for the topology half — one talky
per channel, generations that end when the participant set changes, and the memory hive belonging
to a member rather than to an agent — on [GH #122](https://github.com/mmeyerlein/meclaw/issues/122).

### The columns

```
episodes      speaker       who spoke, as an identity (`member:alex`), NOT the role
              channel       the room it was said in
              audience_set  JSON list of who was present

facts         channel, audience_set        inherited from their episode
entity_edges  channel, audience_set        inherited from their episode
beliefs       audience_set                 INTERSECTION of their source facts'
skills        audience_set                 INTERSECTION of their source episodes'
entities      — nothing, deliberately
```

`sender` (`user`/`assistant`) stays what it was: a **role**. `speaker` is the **identity**, and
the two answer different questions. Translating a connector's own user id into the participant
vocabulary happens on the edge of the talky, never in here — this hive looks nothing up.

`channel` is stored **explicitly** and never parsed out of the `session_id` prefix. That prefix
is a convention of the `session-keeper`, not a promise to this hive.

`entities` carries no audience because an entity is only ever reached through an edge or a fact,
and both of those are filtered. A row you cannot reach visibly, you do not see.

### The context keys

| Lane | Key | Required | Meaning |
|---|---|---|---|
| `in_episode`, `in_remember` | `audience_set` | **yes** | JSON list of who was present. Missing, empty or not a list → `reject` |
| `in_episode`, `in_remember` | `channel` | **yes** | the room. Missing → `reject` |
| `in_episode` | `speaker` | no | who spoke, on a `user` turn. Absent, the `speaker` column stays **empty** — never the role, which lives in `sender` |
| `in_episode` | `agent_id` | no | which agent answered, on an `assistant` turn. Read instead of `speaker` on that role, so a lane carrying a constant `speaker` does not attribute the agent's answers to a person; absent, it too stays empty rather than reaching for the `speaker` beside it |
| `in_query` | `audience_now` | **yes** | JSON list of who is present right now |
| `in_query` | `channel` | **yes** | where the question is being asked |
| `in_query` | `channel_open_history` | no, default closed | true for `1`/`true`/`yes`/`on`; anything else, absence included, is closed |

Sets travel as a **JSON string** (the store column is text); a native list in the context is
accepted too. The stored form is always the string.

`speaker` and `agent_id` are deliberately optional: the audience is the security-bearing field
and the speaker is provenance detail. Refusing a turn because a participant id has not been
mapped to a person yet would make ingress brittle exactly where mapping is hardest — at the
newcomer nobody has named. Better the episode with the right audience and an empty speaker than
no episode.

### Fail-closed, on both sides

A write lane that does not get an audience or a channel **writes nothing at all** and emits one
message on `reject` with `hop.reject_reason` set to `missing_audience` or `missing_channel`. It
does not guess and it does not write silently. The reason is that an untagged row cannot be
tagged later honestly: the audience of a fact is the participant set of the conversation it was
learned in, and once the turn is gone nobody can reconstruct it. Both readings of an untagged
row are wrong — fail-closed makes it a fact nothing may ever use, lenient makes it a fact
anything may use — so the row is refused instead of created.

A read lane without `audience_now` or `channel` is **refused, not filtered**: a recall without an
audience is not a recall with an empty one.

The `reject` lane already exists and `params.required_drains` already makes callers of
`in_remember` and `in_query` subscribe to it. **Drain it on `in_episode` too**, or a refused turn
is an unrouted dead end and nobody learns the memory was not written.

### The rule, in the order it is evaluated

```python
def visible(row_audience_set, row_channel, now_set, now_channel, open_history):
    aud = set(json.loads(row_audience_set or "[]"))
    if not aud:
        return False              # untagged is invisible — FIRST, before everything else
    if "*" in aud:
        return True               # universal
    if now_set <= aud:            # the subset rule, same as affinity/brief
        return True
    if open_history and row_channel and row_channel == now_channel:
        return True               # this room has shown it anyway
    return False
```

The order matters and is normative. If the open-history clause ran first, the one row that got
through would be the row whose provenance we do not have — an open channel says "the room showed
it anyway", but for a row without an audience we do not know **whether** the room showed it. The
clause is a loosening for rows whose provenance we hold, never a rescue for rows whose
provenance is missing.

Two properties worth stating separately:

1. **The open-history clause never crosses a channel boundary.** `row_channel == now_channel` is
   a condition, not a nicety. The dangerous direction — a private two-person channel leaking into
   a group channel — stays closed in every case.
2. **A row without an `audience_set` is invisible**, not visible. The empty set is the empty set:
   no non-empty round is a subset of it.

What that produces:

| Case | said before | present now | result |
|---|---|---|---|
| someone joins | `{A,B}` | `{A,B,C}` | **silence** (`{A,B,C} ⊄ {A,B}`) |
| someone leaves | `{A,B,C}` | `{A,B}` | allowed (`{A,B} ⊆ {A,B,C}`) |
| same circle | `{A,B}` | `{A,B}` | allowed |
| other channel, open history | `{A,B}` in K1 | `{A,B,C}` in K2 | **silence** |
| same channel, open history | `{A,B}` in K1 | `{A,B,C}` in K1 | allowed |
| universal | `{*}` | anyone | allowed |
| untagged | `[]` | anyone | **silence** |

### Derived rows get the INTERSECTION, never the union

This is the part that is easy to get wrong. A belief rests on several facts. It may only be told
to whoever could have heard **every one** of them — the intersection of their audiences. Anything
wider is a laundry: two private facts go in and one shareable claim comes out.

- An **empty intersection is a legitimate result**. The belief is then visible to nobody. It is
  not widened to `["*"]` and not resolved into a union.
- `"*"` in one source is **neutral** — universal cuts nothing away.
- A source that is **missing, or carries no `audience_set`, contributes the empty set**, not
  "don't care". Nothing derived from an invisible fact may be more visible than the fact.

Mechanically: the nightly run reads the audiences of the source facts through the phase pair
`belief-audience` / `belief-audience-park` and parks them as a `scratch` row of kind
`fact_audience`, which meets the verdicts in the apply hop. A cell reads no foreign row — it asks.

A belief carries **no channel**, which means the open-history clause can never rescue one. That
is intentional: a belief is not something a room showed anybody.

`skills.audience_set` exists as a column but nothing populates or reads it yet (skills are spec
phase 5). It is there so a later consumer does not find a table full of untagged rows.

### Provenance is never rewritten

`audience_set` says **who was present**. No code path changes that afterwards — no dream run, no
consolidation, no channel that later opens its history. The policy lives in the rule; the data is
evidence. (The one apparent exception is not one: a belief's audience is recomputed when its
source list changes, because it is a *function of* those sources rather than a record of a turn.)

### When the gate costs certainty, it says so (`supersession_unknown`)

The temporal leg filters **before** it builds a version chain, and that order is deliberate. The
other way round — chain first, filter after — would let the *existence* of an invisible version
show through a validity span: ask often enough and you map out **when** something was said in a
room you were never in. A side channel that compounds with every question.

Filtering first closes that, but it has a price. A claim whose successor is invisible falls back
to the older version and would otherwise be presented as current — and answering wrongly is worse
than answering narrowly. So the surviving candidate carries `supersession_unknown: true`, and
neither the tier-0 bundle nor tier 2 asserts currency for it. The rendered line says
`(currency unknown: cannot vouch that this still holds)`, and the tier-2 prompt is told never to
let such a candidate decide a present-tense question on its own, and to name the uncertainty in
the gap statement it already owes.

**It is a boolean and nothing more.** Not a count, not an instant, not a channel of what was
removed — any of those would put the side channel back, only finer. What the asker learns is not
something about the other room; it is something about this memory's own certainty, and that is
what a memory owes the person asking.

The flag is absent unless true, so a bundle no invisible version touched is byte-identical to
what it was before the gate, and an unaffected candidate costs no token budget.


## Taking a memory out, putting it into another (#243)

Until 2.2.0 there was no way to get the content a hive had accumulated *out* of it, and no way
to put such content *into* another one. The only substrate-native content path was the JSONL
seeder, and that is **birth-only**: `seed_cell_db_if_present` runs during staging, and a
`cell.db` that already exists means an inert seed. So a memory could be born with content and
never receive any afterwards — every migration, every backup, every "run the benchmark against
the same remembered state" was a hand-built `sqlite3` pipeline reaching around the very
boundary [#132](https://github.com/mmeyerlein/meclaw/issues/132) and
[#160](https://github.com/mmeyerlein/meclaw/issues/160) exist to keep closed.

Two lanes close it. `in_export` writes the content out, `in_import` takes it back into a
**running** hive.

### This is the TEMPLATE-level answer, not the substrate one

Read this section as *what `memory-hive` does*, not as *what MeClaw does*. Every store in the
library has the same need — `affinity`'s curated record, `canvy`'s layout, the firewall's rules
and arrivals, a collector's window — and the `store` **cell type** still answers twelve
operations, none of which is an export or an import. That gap is
[#253](https://github.com/mmeyerlein/meclaw/issues/253), and it is where this belongs long
term. **#253 has since shipped and is closed** (2026-08-19): the substrate answers a
`transfer` body slot — `export` and `import` — for every cell that has a `cell.db`, above
the cell type and before `handle()` runs, which is exactly the paragraph on `write_surface`
above. Nothing was removed here in the same move, so what still lives in this section is a
lane pair built out of the twelve operations the store already had; the shrink onto the
substrate slot is the work that is now owed, not a condition that may or may not arrive.

Four substrate properties this path had to work **around** rather than through. They are stated
here because they are the evidence #253's design needs:

1. **The FTS index cannot be maintained from outside the cell.** `episodes` and `facts` carry
   FTS5 indexes built with `meclaw_stem_v1`, a tokenizer that lives in the Rust store cell, and
   their triggers are `AFTER INSERT/UPDATE/DELETE ON <table>` — not column-scoped. Any write
   from a plain `sqlite3` client fires a trigger that cannot resolve its tokenizer and fails;
   the 0.16.0 audience backfill ([#244](https://github.com/mmeyerlein/meclaw/issues/244)) had to detach and reattach those
   triggers inside one transaction to get a backfill through. **This lane simply does not have the
   problem**: it writes through the store's own `insert`, so the triggers fire inside the cell
   that owns the tokenizer and an imported row is searchable the moment it lands.
2. **`seed/<table>.jsonl` carries a schema header the boot validates.** Add a column to
   `params.schema` and every seed file that predates it fails the check — the colony does not
   start until the seeds are lifted, which is what `lift_seed.py` exists for in the migration
   this issue came out of. An export document carries the same header, but it lives outside the
   tree: applying it cannot break a boot, and a part whose header disagrees with the target is
   refused as `import_schema_drift` at the lane instead of at start-up.
3. **`params.schema` cannot express a key**, so idempotency has to be bought with a probe (see
   below), and the birth seeder's own table for a KEYED table is a table without its key.
4. **A store op is one op per message.** `parse_tool_call` reads `messages[0]` and the cell
   emits exactly one `tool_result` per message, so a lane cannot ask two questions at once.
   That is why an import costs four round trips and why both answers have to meet in `scratch`.

### The seeder is an import — what this does that it cannot

Fair question, and the short answer ("birth-only") is not the useful one. The mechanism:

`seed_cell_db_if_present` runs during **staging**, and only when the `cell.db` was freshly
created. An existing database opens as `Resumed` and the seed is **inert** — not merged, not
appended, not diffed; it is not read. There is no message, no operation and no flag that makes a
running cell load one, and there is no second staging for a cell that already exists. So:

| | JSONL seeder | `in_import` |
|---|---|---|
| target | a hive that does not exist yet | a **running** hive |
| a table that already has rows | seed is not read at all | inserts what is missing, skips what is there |
| repeat application | there is no second application | same state, every time |
| keys | table built from the header line: `CREATE TABLE IF NOT EXISTS "<t>" (<col> <type>)` — **no key**, and it runs before `apply_canonical_ddl`, whose `IF NOT EXISTS` then finds the keyless table and leaves it | keyed families go through `set_alias` / `reject_pair`, the store's own upserts on the key the store created |
| FTS | rebuilt after the load (`INSERT INTO <idx>(<idx>) VALUES ('rebuild')`) | maintained by the triggers, per row |
| failure mode | a stale header stops the **boot** | a wrong part stops the **part**, on the reject lane |

The two are complements, not competitors: the seeder births, the lane transfers. This document
format is deliberately readable by both.

### Three decisions #253 will have to make too

They are the design of the operation, not details of this one. Here is what this lane answers,
so the substrate version has a first data point rather than a blank page.

- **Collision on an existing key: the target wins, always.** An import never updates and never
  overwrites. Provenance is never rewritten (ADR-0002 E12) — a row the target already decided,
  including its participant set, is not something a document from elsewhere may replace. The
  operation is therefore a **merge**, and "which of the two is right" stays a question for the
  nightly identity round, which is where every other identity question in this hive lives.
- **Additive, never replacing.** No delete, no update, no truncate-and-load. A replacing import
  is a different operation and would need the no-delete policy's blessing before it could exist.
- **A partial import is a STATE, not a failure.** Validation happens before the first write, so
  a part applies whole or is refused whole; but a document is many parts, and stopping halfway
  leaves the target with a prefix. That is safe precisely because re-applying the whole document
  is idempotent — the repair for any failure is "send it again", and there is no
  compensating action to get wrong. Transactionality across parts is not offered and is not
  needed for that reason.


### The document

A document is a **sequence of parts**, one per content table, each a whole JSON object on the
`dump` lane. There is no monolithic file, and that is deliberate: the store cannot return two
result sets at once, so a part *is* what one read of one table answers.

```json
{"format": "meclaw-memory-export/1", "hive_template": "memory-hive",
 "export_id": "…", "exported_at": "…",
 "table": "episodes", "part": 9, "of": 16, "final": false, "absent": false,
 "key": ["id"],
 "schema": {"id": "text", "session_id": "text", …, "audience_set": "text"},
 "rows": [ {…}, {…} ]}
```

Three properties are load-bearing.

**`schema` is the store's own declaration for that table.** Write `{"schema": …}` as line 1 and
one row per line after it and you have a `seed/<table>.jsonl` — the birth path and the transfer
path speak one format. That is what makes "export the old hive, birth a new one from its parts"
a mechanical operation instead of a script that has to understand the memory.

**`final` is the completeness marker.** A document without the part carrying `final: true` (and
`hop.export_final == "1"`) is incomplete — the walk aborted, and the reject lane says why. A
partial document is not a backup, and nothing else in it says so.

**`absent` is not `rows: []`.** An empty table says *this hive remembered nothing here*; an
absent one says *this hive is older than the declaration and never had the table*.

### What travels, and what deliberately does not

Sixteen parts, in this order — and the order is load-bearing on the way in:

```
predicate_aliases  subject_aliases  claim_aliases
predicate_rejected_pairs  subject_rejected_pairs  claim_rejected_pairs
predicate_cardinality
entities  episodes  topics  facts  entity_edges  beliefs  skills  embeddings
consolidation_log
```

The six store-owned identity tables come first so that by the time a `facts` row is inserted,
the alias tables its canonical columns derive from are already there. `topics` holds what a
stretch of conversation was about — one row per thread, with the episode it opened on, the
episode it closed on and an empty `closed_at` while it is still open — and travels after
`episodes` because that is what its two episode references point at.

Three exclusions, each a decision:

- **`pending_extraction`, `recall_scratch`, `scratch`** are lane state, not memory. Carrying
  them over would restart another hive's half-finished extraction runs in a colony that never
  had them.
- **`emb_models`** is the *receiving* hive's configuration — which generation is live, behind
  which endpoint. Two rows with `active = 1` is a recall that picks its embedding generation at
  random, and rotating the model is an operator job this template deliberately does not do.
- **`facts.canonical_subject` / `_predicate` / `_claim`** travel in the document (a backup you
  cannot diff against the store it came from is not a backup) but are **stripped before
  insert**. The store owns them and re-derives them from the alias tables, which travelled too.
  That is not a second opinion: it is a deterministic function of transferred data. The
  distinction the whole lane rests on is *transfer what was decided, do not decide it again* —
  `in_episode` re-derives (same episodes, a different extractor, different results, and a model
  bill per episode), and that is precisely what this lane is not.

### The audience on the way through

This is the part that had to be got right, because a transfer is exactly where a participant set
can quietly fall off a row (#244, ADR-0002 E12).

- **On the way out**, the export projects `audience_set`, `channel` and `speaker` explicitly.
  A column nobody selects is a column that never leaves the store — the same lesson the read
  path learned when a filter fired over a column the query had not asked for.
- **On the way in**, a part for an audience-bearing table (`episodes`, `facts`, `entity_edges`,
  `beliefs`, `skills`) whose declared `schema` does **not** carry those columns is **refused
  whole**, with nothing written, `hop.reject_reason = "missing_audience"` or
  `"missing_channel"`. An imported row whose participant set did not survive is a row that may
  be told to anyone, and no downstream can reconstruct one honestly.
- **An audience that is present but empty stays empty.** Empty means invisible (contract ruling
  R2, evaluated first), which is the honest fate of a row from before the gate. Inventing one
  would *be* the laundering.
- **Nothing is recomputed.** A belief's `audience_set` is the intersection its dream run
  decided; the import copies it byte for byte and never intersects again. `speaker` is copied
  as it stands and never falls back to the role.

The fail-closed direction is *loss in transit*, not *absence at the source*. A hive full of
pre-gate untagged rows can still be moved; those rows arrive untagged and stay invisible.

### Idempotency, and why it needs a probe

`params.schema` declares column names and types and **no keys** — `apply_schema_ddl` renders
`CREATE TABLE IF NOT EXISTS "<t>" (<col> <type>, …)`, nothing more. So a repeated `insert` of the
same `episodes` row would simply duplicate it. The importer therefore asks first: it reads the
key column of the target table, parks the answer next to the parked part under one `scratch`
key, reads both back in a single `select`, and inserts only the rows whose key is not already
there. **The same document applied twice leaves the same state** — which is what makes it a
backup and a merge rather than only a birth seed.

`key` per table is `id`, except `predicate_cardinality` (`canonical_predicate`) and
`consolidation_log` (`run_id`).

The two **store-keyed** families never go through that path at all. `predicate_aliases` and
friends arrive as `set_alias`, the refusal tables as `reject_pair` — the store's own upserts on
a real `PRIMARY KEY`. This is also the answer to the half of #243 the JSONL seeder cannot reach:
the seeder builds its table from the header line alone, so a `seed/claim_aliases.jsonl` wins
with a table that has **no** primary key and silently costs `set_alias` its upsert property. A
hive that receives its aliases through this lane never has that problem, because the table was
created by `apply_canonical_ddl` with its key and only ever written through the op that owns it.

After the **final** part the importer emits one `canonicalize` per identity dimension, so a
document applied out of order still lands on the same identities as the source. `rows_affected`
counts the identities that moved — zero on a target that already agreed.

### What a migration looks like end to end

```bash
# 1. wire the lanes on the SOURCE hive (one mutation), drain `dump` into a collector
#    { "from": "./memory", "to": "./transfer-drain", "condition": "hop.route == 'dump'" }
# 2. send one message to the hive path with hop.route == 'in_export'
# 3. 16 parts arrive on `dump`; the last carries hop.export_final == "1"
#
# 4a. INTO A RUNNING HIVE: feed each part, in order, to the target hive's `in_import`.
#     Hive to hive, that is a single edge:
#     { "from": "./memory-old", "to": "./memory-new",
#       "condition": "hop.route == 'dump'",
#       "modifier": { "set_hop": { "route": "'in_import'" } } }
#
# 4b. INTO A FRESH HIVE AT BIRTH: write each part as a seed file --
#     line 1 = {"schema": <part.schema>}, then one line per row --
#     under <template>/store/seed/<part.table>.jsonl, and instantiate.
#     The two alias families are the exception: they have to go through 4a,
#     because a seeded keyed table is a table without its key.
#
# 5. the acceptance test: put the same question to both hives with the same
#    audience_now and channel. Same bundle, or the transfer was wrong.
```

Nothing in that sequence touches a `cell.db`. There is no `sqlite3`, no trigger surgery, no
schema header to lift by hand: the FTS indexes of `episodes` and `facts` are maintained by
`AFTER INSERT` triggers whose tokenizer lives in the store cell, so a row that arrives through
this lane is searchable the moment it lands — which is the exact obstacle
the 0.16.0 audience backfill ([#244](https://github.com/mmeyerlein/meclaw/issues/244)) had to detach and reattach triggers around.

### The import at birth, and a retraction (#255, #467)

**Retraction.** Three sentences above say the birth seeder cannot carry the
store-keyed identity families, and give the reason: a seed header carries column
names and a coarse type and no key, so the seeder builds `claim_aliases` and its
five siblings **without** one, and `apply_canonical_ddl`'s `IF NOT EXISTS` then
finds the keyless table and leaves it — which does not duplicate an upsert, it
makes it FAIL. The three are: the `keys` row of the seeder/`in_import` table, the
paragraph beginning *"The two **store-keyed** families never go through that path
at all"*, and step **4b** of the migration sketch (*"they have to go through 4a,
because a seeded keyed table is a table without its key"*).

The mechanism they describe was real and is fixed.
[#255](https://github.com/mmeyerlein/meclaw/issues/255) made the store **assert**
the key at its first wake instead of assuming it: a store-owned table found
standing without its primary key is rebuilt with it, every row carried over,
duplicates collapsed onto the key, and a column the old table had and the
declared shape has not is carried over too. So a seeded alias table keeps its
upsert property, and the exception in 4b is retired. What stays true of those
sentences is everything else: a seed header still cannot express a key, and it is
the store that repairs the consequence rather than the seeder that avoids it.

**What that buys.** The WHOLE document is a birth seed — all sixteen parts, the
identity families included. Writing each part as
`{"schema": <part.schema>}` plus one row per line under
`<template>/store/seed/<part.table>.jsonl` and instantiating is the complete
import at birth, and it needs no message at all.

**The one thing it still cannot be.** The seed is read when the `cell.db` is
created and is inert for ever after. A hive that is already running is `in_import`
territory and nothing else, which is why the two lanes are complements rather
than a choice: **the seeder births, the lane transfers.**

**Where the tree does it.** A member reaches this hive through a `ref`, and a
reference carries no files, so the seed set cannot be handed to it directly. The
reference has to be written out into a derived template that is registered and
instantiated in one diff — [`examples/memory-import/`](../../examples/memory-import/)
is that recipe end to end, and
`crates/meclaw-cells/tests/gh467_a_member_is_born_with_its_history.rs` drives it
against a colony that never saw the memory written.

### Limits, stated so nobody meets them as a surprise

- **A part is a whole table.** There is no paging: `select` carries no offset, and a truncated
  part would lie about being a table. A hive whose largest table outgrows one message needs a
  keyset-paged form of the part, which does not exist yet
  ([#243](https://github.com/mmeyerlein/meclaw/issues/243) follow-up).
- **Embeddings transfer, but only usefully within one generation.** `embeddings` rows carry
  their `model_id`; if the receiving hive's active generation is a different one, the imported
  vectors are inert and the imported facts are **not** re-queued for embedding. The semantic leg
  then runs on three legs for that material until an operator re-embeds. Same-generation
  transfer — the common case, same template and same `MEMORY_EMBED_MODEL` — is exact and saves
  the model bill entirely.
- **An import is confirmed by asking the hive.** The receipt on `dump` says how many inserts one
  part dispatched; a write that failed afterwards arrives on `reject` as `import_write_failed`.
  Neither is a transaction: re-applying the whole document is the repair, and it is safe by
  construction.
- **Two hives merged this way keep both sets of rows.** Nothing dedupes across identities — two
  hives that learned the same thing from different turns hold two facts about it afterwards, and
  it is the nightly identity round that decides whether they are one. That is the same division
  of labour as everywhere else here: the store never merges on similarity, the judge decides.

## Variables

Everything carries a `:-default` **except** `OPENROUTER_API_KEY` and the three `MODEL_*` slots the
hive buys inference on — `MODEL_CLOSER`, `MODEL_DREAMER`, `MODEL_DIALECTIC`. Those four must come
from `.env` (see the negative fixture `memory_hive_env_missing`). A model name has no defensible
default: picking one silently is how a memory lane ends up on a weak model without anybody
deciding it (see the recommendation below).

`MODEL_JUDGE` is the deliberate exception (0.2.0 P5). It carries a **top-tier placeholder**
default, because the failure mode is the other way round there: an unset variable would make the
nightly canonicalisation round fail silently, every night, and nobody would notice a merge that
did not happen. The default is not a recommendation to leave it alone — set it explicitly at
rollout, and set it to the strongest model you have (see below).

> **The `MEMORY_*` knob names are an EXPERIMENTAL configuration surface and carry no
> compatibility promise.** They will migrate onto the `params` surface in a 0.x release, at which
> point the environment-variable spellings below change or disappear; refs
> [#138](https://github.com/mmeyerlein/meclaw/issues/138).

| Variable | Default | Effect |
|---|---|---|
| `OPENROUTER_API_KEY` | — (required) | api_key of `closer`, `dreamer`, `judge` and `dialectic` |
| `MODEL_CLOSER` | — (required) | close-pass model (GH #300, ruling Q9 of 2026-08-21). **Put the strongest model you have here.** It is the one call that sees a whole session at once and it is the only party that can supply the replacement window, so a quiet fallback to a cheap model would not just measure less, it would revoke the ruling. Deliberately without a default for exactly that reason |
| `MODEL_DREAMER` | — (required) | consolidation model (change narrative) |
| `MODEL_JUDGE` | `anthropic/claude-opus-5` | identity judgement of the nightly canonicalisation round — **the strongest model of the hive belongs here** |
| `MEMORY_LLM_BASE_URL` | `https://openrouter.ai/api/v1` | OpenAI-compatible endpoint of every llm cell |
| `MEMORY_TIER0_TOKENS` | `1200` | token budget of the tier-0 bundle |
| `MEMORY_TIER0_MAX_EPISODES` | `12` | item cap of the bundle's episode leg |
| `MEMORY_TIER0_MAX_BELIEFS` | `20` | item cap of the bundle's belief leg, and the `limit` of the belief select behind it |
| `MEMORY_TIER0_MAX_FORESIGHT` | `10` | item cap of the bundle's foresight leg (facts that are about a future the memory has been told about) |
| `MEMORY_TIER0_EPISODE_CHARS` | `400` | episode truncation inside the bundle (truncate, never delete) |
| `MEMORY_DREAM_CRON` | `0 0 3 * * *` | 6-field Quartz schedule of the nightly run, **in UTC**. The `timer` cell type plans every occurrence on `DateTime<Utc>` and has no timezone knob (`crates/meclaw-cells/src/timer/io.rs`), so the default fires at 03:00 UTC — 05:00 in Berlin summer time, 04:00 in winter. Pick the field for the UTC hour you want, not for the local one |
| `MEMORY_EMBED_ENDPOINT` | `https://openrouter.ai/api/v1/embeddings` | OpenAI-compatible embeddings endpoint |
| `MEMORY_EMBED_MODEL` | `google/gemini-embedding-2` | must match the `model_id` in `seed/emb_models.jsonl` — the seed is NOT variable-substituted, so the two are coupled by hand. Pinned against the seed by `gh204_the_shipped_embedding_generation_agrees`; a disagreement empties the semantic leg silently, it does not raise |
| `MEMORY_EMBED_DIM` | `1024` | requested `dimensions`; must match `emb_models.dim` (1024 bits → 128 packed bytes) |
| `MEMORY_EMBED_API_KEY` | *(empty → falls back to `OPENROUTER_API_KEY`)* | bearer for the embedder |
| `MODEL_DIALECTIC` | — (required) | tier-2 synthesis model |
| `MEMORY_REASONING_CLOSE` | `medium` | `provider_extra.reasoning.effort` of the `closer` cell. **Not `minimal`**: a strong model asked to sharpen a session it is seeing whole is not a shape-filling call |
| `MEMORY_REASONING_DREAM` | `medium` | `provider_extra.reasoning.effort` of the `dreamer` cell (the nightly change narrative) |
| `MEMORY_REASONING_DIALECTIC` | `medium` | `provider_extra.reasoning.effort` of the `dialectic` cell (the tier-2 answer with its gap statement) |
| `MEMORY_REASONING_JUDGE` | `high` | `provider_extra.reasoning.effort` of the `judge` cell — the identity verdicts are written into the store and every later read consumes them, so this is the one lane where thinking is worth paying for |
| `MEMORY_CANON_JUDGE` | `1` | `0` switches the nightly canonicalisation round's ASK off (no scan, no candidate feed, no model call). A judgement handed to the lane from outside is still applied — applying is arithmetic, asking is what costs. The free scenario classes run with `0`, except the two that measure the QUESTION (`C19`/`C20`): those ask for real with the endpoint pointed at a dead port |
| `MEMORY_CANON_MAX_PREDICATES` | `60` | relation keys put to the judge per run, busiest first |
| `MEMORY_CANON_MAX_PAIRS` | `12` | entity candidate pairs put to the judge per run, best score first |
| `MEMORY_CLOSE_TURN_ROWS` | `512` | page bound of the close pass's session read. Ordered NEWEST first on purpose — a session longer than one page loses its oldest turns rather than its last ones, and the later a turn is, the likelier it is the one that corrects an earlier. The page is re-ordered oldest-first before it reaches the prompt: the order of the READ decides what survives the bound, the order of the RENDERING is a different question |
| `MEMORY_CLOSE_FACT_ROWS` | `256` | page bound of the close pass's fact read, one dimension over: a session has turns, and the facts those turns left standing are a different count entirely — a short session can carry a long history if its subject has been talked about before. This is also the budget of the replacement window, because the two are the same rows. What the bound left behind is reported on `close_report` as `truncated` |
| `MEMORY_TIER1_LEG_LIMIT` | `20` | per-leg candidate cap of the tier-1 fan |
| `MEMORY_TIER1_AXIS_LIMIT` | `200` | page bound of the AXIS reads — the hydration's chain select (`t1-hyd-axis`) **and** the window leg's generous pre-filter share it. Too small truncates a chain, and a candidate whose chain was cut is delivered **without its predecessors** — `history: []` on the record in `recall_diagnostic`, no `previously` key in the payload — rather than with a guessed chain |
| `MEMORY_CANON_EXTRACT_LOOKBACK_DAYS` | `7` | how far back the nightly round reviews the closures the EXTRACTOR wrote (W4, ruling Q2 guard rail 3). Derived from `delta_to`, so a re-run reads the same window back; a closure older than this has been in front of a round already |
| `MEMORY_CANON_MAX_AXES` | `8` | axes with more than one open statement put to the judge per run (the currency question of W3, and since W6 the rewording question on the same list). The whole budget, pages included: an axis too big for one page takes one of these slots rather than a slot of its own (GH #66) |
| `MEMORY_CANON_MAX_PAGED_AXES` | `2` | how many of those slots a night may spend on axes it can only show one PAGE of (GH #66). Such an axis is never truncated blind: it is offered only after its relation was seeded or judged FUNCTIONAL, the page is the most recent statements, and what it leaves behind is stated in the run receipt |
| `MEMORY_CANON_MAX_CARD` | `8` | relations whose CARDINALITY is put to the judge per run (statement identity W5). Only relations the seed list does not own and the store has not judged yet are offered, so the budget always reaches something the memory has not decided |
| `MEMORY_CANON_CLOSED_ROWS` | `256` | page bound of the identity questions' own read of the CLOSED rows (GH #73), most recently ended first. Bounded on ROWS and never on a clock: `expired_at` says when a statement stopped being TRUE, not when the closure was written, so a cutoff derived from `delta_to` would drop exactly the closure written last night about a change dated last spring |
| `MEMORY_CANON_MAX_CLOSED_AXES` | `12` | how many spellings out of that page reach the two identity questions (GH #73). ON TOP of `MEMORY_CANON_MAX_PREDICATES`, not out of it: a spelling a closure just proved belongs to another one is the best-founded question a night has, and the open vocabulary's budget was never sized for it |
| `MEMORY_SCRATCH_TTL_DAYS` | `7` | retention window of the lane bookkeeping tables (`scratch`, `recall_scratch`), in days before the nightly run's own window end (GH #375). What is older is deleted by the night; nothing else is — no memory table, no provenance, no durable row. Deliberately generous: a parked row is only ever read back inside the run that wrote it (a recall lives seconds, an extraction a round trip, a close pass one model call), so a week is orders of magnitude past the longest-lived parking. A window short enough to fall inside a lane's own round trip **would** delete state a running pass is about to read — this is days, not minutes |
| `MEMORY_DREAM_AXIS_LIMIT` | `5000` | page bound of the dream lane's axis select. A **full** page means the chain may be incomplete, so the materialisation SKIPS the derivation for that page instead of guessing supersession from half a chain |
| `MEMORY_TIER1_TOPK` | `20` | how many fused candidates survive the RRF cut into the tier-1 bundle |
| `MEMORY_TIER1_TOKENS` | `2000` | token budget of the tier-1 bundle; candidates are taken in fused order until the next one does not fit. It measures the **payload** candidate — what travels to the model — never the record in `recall_diagnostic`: the trace pays no prompt budget, so costing it would spend the whole #296 saving on refilling the budget instead of shipping a smaller bundle. `MEMORY_TIER1_TOPK` stays the count cap |
| `MEMORY_TIER1_ITEM_CHARS` | `400` | per-item truncation inside the tier-1 bundle (a claim, an episode's content, a supersession marker). Truncate, never drop |
| `MEMORY_BUNDLE_EPISODE_BUDGET` | `6` | the episode share of the fusion cut (P15 O-7): episodes take at most this many of the `TOPK` slots and keep that many against any fact wall. Whichever side cannot fill its share lets the other backfill, so the bundle is never shorter than a plain prefix would be |
| `MEMORY_TIER1_GRAPH_DEPTH` | `2` | `max_depth` of the graph leg's `traverse` (store cap: 5) |
| `MEMORY_TIER1_GRAPH_NODES` | `200` | `max_nodes` of the graph leg's `traverse` — the fan-out kill switch, so one hub entity cannot turn a recall into a walk of the whole graph |
| `MEMORY_TIER1_GRAPH_FACT_NODES` | `64` | how many distinct walked nodes go into the join's `in` filter (GH #520). The walk is already ranked when the cut is taken, so what falls off is the tail of the walk, never its front |
| `MEMORY_TIER1_GRAPH_FACT_LIMIT` | `100` | page bound of the join's `select facts` (GH #520). Generous on purpose: one popular subject carries a long version chain, and the leg's own `MEMORY_TIER1_LEG_LIMIT` is the cut that decides what votes. A full page marks the leg **capped**, exactly as a full traverse page does |
| `MEMORY_TIER1_SELF_LIMIT` | `20` | page bound of the **self** leg (GH #536): how many of the asker's own facts it NOMINATES, newest first. Generous, because a member's dossier is a small bounded set (21 live rows on the hive this was measured on) and the leg has no query signal to rank by: what it cannot rank it must not cut early |
| `MEMORY_TIER1_SELF_BUDGET` | `6` | how many of them may occupy a **bundle slot** while query-driven hits are waiting (GH #536). A different question from the one above: the leg nominates, the composition seats. Without it the dossier ate the fact half of every bundle — two different questions, one identical `FACTS` section. Leftover slots still fall back to the dossier, so it is a ceiling against competition and never a cut |
| `MEMORY_SELF_LEGACY_SUBJECT` | `user` | the pre-canonicalisation spelling of *the member whose hive this is* (GH #536). The extraction lane writes a PERSON NAME into `facts.subject` today; everything written before it did carries the literal `user` — 23 of 29 self facts on the measured hive — and those rows are about the asker exactly as the new ones are. A **migration artefact**, named as one: the empty string switches it off for a hive that never had them |
| `MEMORY_SEM_MAX_DISTANCE` | `0.5` | relevance floor of the **semantic** leg (#297), as a fraction of the embedding's BIT WIDTH: a hit at or beyond `0.5 × dim` differing bits is where a random binary vector sits, so at the default the cut removes coin flips and cannot cost a genuine hit. `similar` RANKS and never filters — without this every one of its `MEMORY_TIER1_LEG_LIMIT` rows votes in the fusion as loudly as a real hit. A missing, zero or non-numeric `dim` means no scale, and without a scale there is no cut |
| `MEMORY_KW_MIN_SCORE_RATIO` | `0.10` | relevance floor of the **keyword** leg (#297), as a fraction of that page's OWN best bm25 rank — so the fact search and the episode search are never measured against each other's scale. bm25 is smaller-is-better, so the floor is `best × ratio` and a row survives at or below it. `0` switches the cut off (the ablation knob), and a page whose best rank is not negative carries no usable signal, so it is left uncut |
| `MEMORY_RRF_AGREEMENT` | `0.5` | strength of the agreement factor in the fusion (#297): a candidate two VOTING legs found is multiplied once, three legs twice. `0` restores the plain rank sum |
| `MEMORY_RRF_K` | `60` | RRF constant |
| `MEMORY_RRF_W_KEYWORD` | `1.0` | fusion weight of the keyword (FTS) leg |
| `MEMORY_RRF_W_SEMANTIC` | `1.0` | fusion weight of the semantic (embedding) leg |
| `MEMORY_RRF_W_GRAPH` | `1.0` | fusion weight of the graph (traverse) leg |
| `MEMORY_QUERY_SAFE_CHARS` | `200` | a query at or below this length reaches the legs untouched. Above it the hygiene guard runs (GH #88) |
| `MEMORY_QUERY_MAX_CHARS` | `250` | hard clamp of the hygiene guard: whatever survived the question / tail-sentence steps is cut to its last this many characters, so the cost of a recall stops depending on how much context the caller pasted |
| `MEMORY_QUERY_TOKENS` | `24` | how many tokens of the sanitised query reach the FTS matcher, taken from the TAIL — after the hygiene guard the tail is the question (GH #88) |
| `MEMORY_RRF_W_TEMPORAL` | `1.0` | weight of the temporal leg **in window mode only** (P15 O-4) |
| `MEMORY_RRF_W_SELF` | `1.0` | weight of the **self** leg (GH #536). Its rank list is recency, not relevance — it is the one leg the query does not shape — so a hit two query-driven legs agree on outranks it through the agreement factor rather than through a special case. `0` leaves the leg as discovery only, which is the ablation the eval prices |
| `MEMORY_RRF_W_TEMPORAL_POINT` | `0.0` | weight of the temporal leg in **point** mode. Measured, not chosen: 50 identical LongMemEval extractions, paired — `0.0` gives R@1 **84.0 vs 74.0** and R@5 **98.0 vs 96.0**, flips **11:1** (sign test p=0.0063), and across 100 runs (2×50) the leg never carried a hit **alone**. Set it to `1.0` to restore the pre-O-4 fusion |
| `OPENROUTER_HTTP_REFERER` / `OPENROUTER_X_TITLE` | `https://meclaw.ai` / `MeClaw` | OpenRouter app attribution headers |

**Model recommendation (P15 R10): put the memory lane on a strong model.** Since 3.0.0 the model
that mints facts mid-conversation is the FRONT model, not one of this hive's slots (#298) — so
the recommendation applies one address over as well, and the hive's own four slots are the night,
the tier-2 answer, the identity judgement and the close pass. Every extraction defect found so far
hangs on a weak model: a small village name silently "corrected" into a spelling that does not
exist, English predicates (`was in`, `was with`) mixed into a German axis set, the same fact
written to three predicate axes, and — the one that P15 made measurable — belief SUMMARY
sentences instead of the change narrative the contract asks for. Evidenced 2026-08-10 against
`openai/gpt-5.6-luna`: the change narrative comes out exact
(`Alex preferred Helix until 2026-08-08, then switched to vscode.`, scenario T9, 5/5
assertions, ≈ 0.02 ¢ per run — names generalized from the original run) and the
collection-sentence failure class is gone. `openai/gpt-5.6-sol`
is the obvious judge-tier option for a case that needs one; P15 did not use it, and 0.2.0 P5 gave
it a slot of its own (`MODEL_JUDGE`, ruling Q1): the identity judgement is written into the store
as an alias and every later read consumes it, so it is the ONE place in the hive where the
strongest available model is the right answer rather than an indulgence. `MODEL_CLOSER` is the
second (ruling Q9): it sees a whole session at once, and it is the only party that can hand this
memory a replacement window.

**Two of the four lanes are cheap and one is not.** The night runs once per member per day, the
dialectic only on a tier-2 question, the judge once per night — all of them in cents. The close
pass runs once per closed SESSION on a top-tier model and is measured at ≈ 0.077 EUR a time; on a
26-turn conversation that was about 80 % of everything the colony spent. See
[What a close pass costs](#what-a-close-pass-costs-measured).

Numeric params (`query_timeout_ms`, `external_timeout_ms`, …) are **literals, never `${VAR}`** —
substitution yields strings and the parsers want integers. Tunables reach the `code` cells
through `${VAR:-default}` **inside the script literal** (`daily-digest` precedent) — a
colony-global route. Per-instance knobs go through the stdin `params` object instead
(`docs/cell-types.md` § `code`); the hive's migration onto that surface has started at the
embedder's timeouts (below) and continues one knob group at a time.

### `./embed` timeouts and the read-lane retry (params, GH #146)

These four are **params of `./embed`**, not environment variables: they ship with their default
in that cell's `config.json`, the script reads them off its stdin `params` object, and there is
**no environment fallback** — a `.env` line for the retired `MEMORY_EMBED_TIMEOUT_MS` is read by
nothing. Retune one per instance by editing the instantiated `config.json`.

| param | default | meaning |
|---|---|---|
| `timeout_ms` | `20000` | **write lane.** A-timeout of one bulk corpus embedding call. Throughput, not latency: a failed batch leaves its rows `status='queued'` and the nightly backfill picks them up, so a tight bound that frees the concurrency slot is the cheap answer. That backfill IS this lane's retry, which is why there is no in-process one |
| `query_timeout_ms` | `30000` | **read lane.** A-timeout of ONE query-embedding attempt. Deliberately more generous than the write lane: the query vector is what recall's semantic leg waits on, and losing it costs a whole leg of the five-leg fan |
| `query_retries` | `1` | **read lane.** Extra attempts after the first before the lane answers degraded. `0` switches the retry off; the fail-open contract is unaffected either way — a retry never replaces it |
| `query_retry_backoff_ms` | `250` | **read lane.** Pause between two attempts. Short by design: the measured failure was CPU contention on the box, not a rate limit |

The read lane bounds itself against the cell's own `external_timeout_ms` (`65000`) and keeps a
2 s reserve for spawn plus the final write, because a process killed mid-flight is **silence**,
and silence hangs recall's fan-in forever — strictly worse than the degraded answer the retry
exists to avoid. So the worst case has to fit: `(query_retries + 1) × query_timeout_ms +
query_retries × query_retry_backoff_ms + 2000 ≤ external_timeout_ms`, and
`cell.message_timeout` (`90000`) stays above that. Raise one of the four and raise the operation
timeout with it — a test pins the arithmetic, so getting it wrong is a red test rather than a
production hang.

## Two mechanisms worth knowing before you edit a lane

**1. State lives in the store, never in a cell.** `code` cells have no `cell.db` (DEC-009).
Every lane is a state machine whose state is `context.mem_phase` plus rows in the store. The
phase is minted as `hop.phase` and promoted by the outgoing edge, so it survives the store
round trip and the whole dataflow is visible in `/colony/messages`.

**2. The store has no joins and returns one result set per message.** A lane that must diff set
A against set B parks both under one key in `scratch` and reads them back in a single select
(extract-glue: staged payload vs. known claim hashes, and the claimed batch vs. the predicate
axes already in use; dream-glue: verdicts vs. known belief ids and embedding queue vs. fact
claims). That is the join-less equivalent of the `collector` fan-in, and it is why `scratch`
exists.

**3. `operation` says which op answered; `error_code` says whether it worked.** Both have to
be read. See the next section — it is the failure this mechanism produced when only the first
half of it was.

## When the store says no (GH #343, since 2.3.1)

Every lane in this hive dispatches on `(context.mem_phase, hop.operation)`. The store stamps
`hop.operation` on its **failing** replies too — it always did for SQL-level failures
(`unknown_table`, `unknown_column`, `constraint_violation`, `sql_error` travel through the
ordinary reply builder), and since
[#331](https://github.com/mmeyerlein/meclaw/issues/331) it does for `invalid_input`,
`query_timeout` and `write_denied` as well. So a refusal arrives looking **exactly** like an
answer: same phase in the context, same op in the hop, an error sentence where the rows should
be — and read as an answer, that sentence is zero rows.

Measured on all three glue lanes before the fix:

| lane | phase | what a refusal did |
|---|---|---|
| `recall` | `t1-kw-ep` | recorded the failed keyword leg as a leg with **zero hits**; the fused bundle then read "memory knows nothing about this", which a caller cannot tell apart from the truth. This is [#308](https://github.com/mmeyerlein/meclaw/issues/308) exactly, one hive over |
| `extract-glue` | `vocab`, `known` | walked on and prompted the extractor with an **empty** known-predicate vocabulary and an **empty** dedup set — the model then mints spellings the hive already had, and the duplicates it writes cannot be taken back |
| `dream-glue` | `scope` | booked the run `status: "done", facts_in_window: 0`. Every later night derives `delta_from` from the newest *done* row, so the window nothing ever looked at is **skipped forever**. The only one of the three that a later run cannot repair by itself |

Since 2.3.1 every branch reads both fields, and a refusal is terminal: no further store op
leaves, the phase does not advance, and the lane says so on the `reject` lane with
`hop.reject_reason == 'store_refused'`, `hop.store_error` (the store's own `error_code`) and
`hop.store_operation` (the op it refused). `store_error` is a free string rather than part of
the `reject_reason` enum, because the store's code list is open and a declaration that had to
grow with it would turn the next new code into a failed emit.

The nightly consolidation got its own door to that lane in the same change
(`./dream-glue -> .` on `hop.route == 'reject'`). It is the one lane with no caller of its
own, so drain `reject` and you will hear about a night that could not run — which is the
alternative to hearing about it never.

## The read path (tiers 0, 1, 2)

| Tier | Cost | What it does |
|---|---|---|
| 0 | no LLM, no embedding, ONE store round trip | three fixed legs (session episodes, active beliefs, open foresight) → one token-budgeted bundle |
| 1 | no LLM, one embedding call | four retrieval legs → RRF fusion → hydration → ranked candidates |
| 2 | one LLM call on top of tier 1 | `dialectic` synthesises an answer with a mandatory gap statement |

### One round trip for tier 0 (GH #295)

A tier-0 recall costs the store **one message and one reply**, whatever its three legs find.

Until 2.3.1 — the last version shipped before this one — it cost nine. The three legs left as
three messages, each answer was parked as a `recall_scratch` row, a select asked whether all
three had landed, a guarded update elected the one hop allowed to build the bundle, and a last
select read the parked payloads back: three selects, three inserts, two selects and an update
for a question whose legs never read each other's rows. That machinery is the fan-in of a
fan-out, and since the `store` cell answers **N ops in one bundle**
([#295](https://github.com/mmeyerlein/meclaw/issues/295)) there is no fan-out left to fan in.

So the three legs travel as three `tool_call` turns in ONE message (`hop.phase == 'legs'`, ids
`r-leg-episodes`, `r-leg-beliefs`, `r-leg-foresight`) and come back as one bundle
(`hop.operation == 'bundle'`): schema-pure `tool_result` turns in call order plus a top-level
`results[]` slot holding each leg's own `operation`, `rows_affected`, `duration_ms` and, if it
failed, `error_code` — correlated by `tool_call_id`, because the UBF turn schema is closed and a
turn cannot carry metadata of its own. That single hop gates the rows, projects them, folds the
token budget and emits the bundle.

Nothing about **what** the bundle says moved with it: the audience gate, the per-leg caps, the
`supersession_unknown` marker and the fixed leg priority (beliefs → open foresight → recent
episodes) are the same code they were, and a tier-0 round now writes **no `recall_scratch` row
at all**.

### And six for tier 1 (GH #418), seven when the graph leg joins (GH #520)

A tier-1 recall costs the store **six messages**, and a seventh exactly when the graph walk
reached a node (S4a below). Until 3.0.1 it cost 47 — measured, not
estimated, out of the `message_log` of a scenario colony — of which twenty were the bookkeeping
of a fan-in: an insert per leg, a select to ask whether they had all landed, a guarded update to
elect the one hop allowed to carry on, and a select to read back what had just been written.

The chain, in the order it happens:

| # | Phase | The ops of that ONE message | What the answer carries |
|---|---|---|---|
| S1 | `t1-fan` | `search episodes` · `search facts` · `select facts` (as-of) · `select facts` (the asker's own — GH #536) · `select emb_models` · *(tier 2:* `select beliefs`*)* | every leg that needs no query vector, at once; beside it (**not** a store message) the `embed` request |
| S2 | `t1-park` | `insert recall_scratch 'legs'` · `select recall_scratch` | parks the five legs, the raw counts, the model, the anchors and the axis map — **and** asks in the same message whether the vector has landed |
| Sq | `t1-qvec-park` | `insert recall_scratch 'qvec'` · `select recall_scratch` | the other half of the same question |
| S3 | `t1-join` | `select entities` (anchors) · `similar embeddings` | both are free from the rendezvous on; the vector comes from `'qvec'`, the anchors and the `model_id` from `'legs'` |
| S4 | `t1-legs` | `insert 'sem'` · `traverse entity_edges` · `select facts` (the semantic companion) · `select recall_scratch` | the walk, the audience of the semantic leg's owning facts and everything parked, in ONE answer — **the fusion runs here** unless the walk reached a node |
| S4a | `t1-graph` | `insert 'graph-walk'` · `insert 'sem-aud'` · `select facts` (the walked nodes) · `select recall_scratch` | GH #520 — the facts the walk's nodes are ABOUT. **Conditional**: the nodes a walk passed through are only known after the walk, so this is the one message a recall pays only when there is something to join; **the fusion runs here when it does** |
| S5 | `t1-emit` | `insert 'fused'` · `select facts` (hydration) · `select episodes` (hydration) · `select facts` (the axis page) · `select predicate_cardinality` · `select recall_scratch` | hydration, axis expansion and the judged verdicts at once — **the reply to this renders and emits, and asks nothing more** |

**Why there is no gate any more.** The `store` is a **stateful** cell (`docs/cell-types.md`,
concurrency note): one task, one connection, one message at a time. A bundle is ONE message, and
its ops run in call order over that one connection — so a `select` at the END of a bundle sees
the `insert`s in front of it, and **of two hops that park concurrently exactly one reads a
complete set**. That is precisely what `update … set fired=1 where fired=0` plus its
`rows_affected` used to buy, and it costs no message of its own. Pinned in
`crates/meclaw-cells/tests/gh418_a_bundle_sees_its_own_writes.rs`; the count is pinned in
`gh418_tier1_is_one_bundle_per_step.rs::a_tier1_recall_costs_six_store_messages`, and the
conditional seventh in `gh520_from_an_entity_to_a_fact.rs`.

**What `recall_scratch` still is on the tier-1 path.** The **wait for the embedder** — the
rendezvous between the parked `legs` row and the parked `qvec` row — plus two carrier rows
(`sem`, `fused`) between two hops, and, at tier 2 only, the rendered bundle (`rendered`) that a
provider failure downgrades to. **Four rows, where there were twenty.** No row on that path
carries a `fired` flag that anything reads any more.

What does **not** get smaller, and why: `entities` → `traverse` is a real data dependency (the
anchor names come out of the select), and `similar` → the semantic companion is another (the
owner ids come out of the ranking). Both are therefore laid **pairwise across two bundles**
rather than one behind the other in four messages: S3 carries the first half of both chains, S4
the second. That is the whole saving at that point, and it is behaviour-neutral.

**A refused leg is still terminal** ([#343](https://github.com/mmeyerlein/meclaw/issues/343)). A
bundle is explicitly not a transaction, so one leg can fail while its siblings carry rows — and
`hop.bundle_errors > 0` is exactly the "memory knows nothing" bundle #343 exists to prevent.
Tier 0 keeps the strictness it had: any refused leg stops the round on the `reject` lane with
`reject_reason: "store_refused"`, `hop.store_error` = that leg's `error_code` and
`hop.store_operation` = the op it refused, read off `results[]`.

### One tier-1 message, two documents (GH #296)

A tier-1 recall leaves on the `bundle` lane as ONE message written for **two** readers, and the
split is the whole point of the shape:

| slot | reader | what is in it |
|---|---|---|
| `system.memory.bundle` (a `json` document) plus the `tool_result` turn rendered from it | the **model** that has to answer the question | what answers the question, and nothing else |
| `recall_diagnostic` — a top-level body slot beside `system` and `messages` | the **person** who has to explain an answer afterwards | the run's own bookkeeping: the candidate records whole, the flat ranking in its old wording, the leg sizes before and after the relevance floors |

`recall_diagnostic` is declared in `contract.emits.body` and is a body slot on purpose: the
`collector`'s `in_bundle` lane keeps `system` and `messages` and nothing else, so the diagnostic
is dropped before the next prompt is assembled while `/colony/messages` — which stores whole
bodies — keeps it. The trace survives without ever being paid for in prompt budget. The tier-2
`dialectic` call is the one consumer that gets the **full internal records** (ids, scores,
`agreement`, sessions and all): it is a reasoning step over the retrieval, not the answer, and
#296 draws its line at what reaches the ANSWERING model, not at what reaches every model.

Tier 0 emits neither slot — no `recall_diagnostic`, no `answers` key — and neither does the
tier-2 final answer on `system.memory.answer`: an absent `answers` is not a relevance judgement
of `"none"`, it is the absence of one.

**Explicit retraction** (`docs/development-rules.md` § 3). Up to `memory-hive@2.2.1` the payload
candidate carried the retrieval's own bookkeeping and this README said so. It no longer does.
Nothing was deleted: every field below is reachable in the SAME message, which is the only reason
it was allowed to leave the bundle at all.

| gone from the payload candidate | where to read it now |
|---|---|
| `id` | `recall_diagnostic.candidates[].id` — and in the dialectic payload |
| `session_id`, `episode_id` | ditto; a fact carries both since #148 |
| `rank`, `score` | ditto, `score` next to the `agreement` it was multiplied by |
| `legs` | ditto — leg attribution answers "which leg found this", which is a question about the RUN |
| `superseded_by` (the successor's row id) | ditto; the payload answers the reader's question instead, with `until` — the DAY the statement stopped |
| the full `history` chain | ditto; both halves of the bundle carry at most ONE `previously` entry |
| exact instants | ditto; the payload carries days (below) |
| `legs_present`, `leg_sizes`, `semantic_degraded` (bundle level) | `recall_diagnostic`, at bundle level — they describe the RUN, never the answer. `leg_sizes_raw` and `leg_capped` are new in 2.3.0 and were never in the payload |

**What a payload candidate carries, per kind.** Every key is absent unless known, so a candidate
that knows nothing pays no byte for saying so:

| kind | keys |
|---|---|
| `fact` | `kind`, `subject`, `predicate` (the axis, either half may be missing), `text` (the claim), `since`, `until`, `confidence`, `intent`, `supersession_unknown`, `superseded`, `span`, `previously` |
| episode | `kind`, `text`, `who` (the sender), `when`, `seen` (only above 1) |

Two of them are provenance the store always held and the bundle used to throw away (#280):
`confidence` — the store keeps one per fact and the hydration selects it, so a hedged claim stops
reading like a certain one — and `until`, the day the statement stopped, read off the store's own
`valid_until`/`expired_at` rather than derived a second time. The other two qualifiers are older
and unchanged: `intent` marks a plan as a plan rather than as an accomplished thing (GH #67), and
`supersession_unknown` says the audience gate cannot vouch for this statement's currency.

**Instants arrive as DAYS.** `since`, `until`, `when`, `span` and the day inside `previously` all
go through the same `fmt_day` the rendered line uses. A "since when?" question is answered at day
resolution, and the two halves of the message answer it identically. The exact timestamps are in
the diagnostic.

**`previously` is one entry, never a chain — in BOTH halves of the message.** The version chain
is sorted by start, so the last entry is the claim this one immediately replaced — everything
older is the history OF that history and answers a question nobody asked this round. In the JSON
payload it renders as `[{"claim": …, "until": …}]` (the `until` absent when the predecessor's end
is unknown); in the rendered text block beside it, as `(previously: kakoune until 2026-04-01)`.
Both halves ask the same `history_entries` helper, which is what keeps them from parting again —
and they had, for two releases: the text ended a superseded line with the FULL chain while the
JSON carried one entry, and since both travel in the SAME prompt a long-lived axis handed part
of the saving back one slot over
([#296](https://github.com/mmeyerlein/meclaw/issues/296), ruling S6). The whole chain stays in
the diagnostic — as a RECORD (`recall_diagnostic.candidates[].history`), not as prose, which is
exactly why a rendering decision cannot shorten it.

**`superseded` is copied by PRESENCE, not by truth.** Present-and-empty is an answer of its own:
closed, with no successor anybody can name. Absent means the statement is open.

**`span` is window mode's answer.** In a time-range question every version stays its own
candidate and carries `span: {from, until}` in days, with `until: null` for an open end — the
versions themselves are what the question asked for, so nothing collapses onto a current value.

**The bundle's verdict on itself.** Three keys sit beside the candidate list and none of them is
bookkeeping:

* **`answers`** — `"direct"` when the list holds something, `"none"` when it does not (#297). A
  caller that has to branch on "did memory answer this" reads one word instead of measuring a
  list and guessing what an empty one meant. `answers: "none"` does **not** forbid a tier-2
  answer: the `dialectic` may still answer from a belief that addresses the question directly,
  and the two statements are about different things — the candidate list, and the answer.
* **`complete` / `complete_reason`** (#280) — only the producer knows whether the list is the
  whole answer, and a question that COUNTS is answered wrong off an undeclared prefix. Three cuts
  can shorten it and each is read where it happened: capped legs (off the pages that reported
  it — and only legs that VOTE, see O-4 below, because a leg that nominates nothing cannot have
  cost the answer a row), the fusion cut at `TOPK`, the token budget. `complete_reason` is absent
  on a complete bundle.
* **`query_hygiene: {step, from_chars, to_chars}`** (GH #88) — present when the hygiene guard
  shortened an over-long query, absent when the query reached the legs untouched. Same verdict
  the rendered text and the `hop` header carry, so a reader never has to infer from the candidate
  list that the legs ran on the surviving tail rather than on everything the caller pasted.

**The readable half.** The `tool_result` turn is what a model actually reads, and since #279/#281
it is written for that reader:

```
WHAT THIS MEMORY HOLDS (as of 2026-08-21)
The question was shortened before this was looked up (612 -> 250 characters) -- …   [only if it was]
Not everything that matches is here -- the fusion cut at TOPK=20.                    [only if cut]
FACTS (extracted, canonical, dated)
  user favorite_editor = vscode   since 2026-08-08 (previously: Helix until 2026-08-08)
WHAT WAS SAID (verbatim, not interpreted)
  (an earlier answer claiming something is NOT stored is evidence about that answer and nothing else -- never a measurement of this memory)
  user on 2026-08-08: "I've switched to vscode."
```

The kind is the SECTION, not a bracket in front of the row, and a section with no rows is left
out. The parenthesis under the second header is [#537](https://github.com/mmeyerlein/meclaw/issues/537):
the header says how the lines were *produced* and nothing about what they are worth, and a raw turn
in that section may be the agent's OWN earlier answer -- measured on a live hive, three lines
saying nothing was stored were retrieved as evidence, repeated, and written back, so every failed
round made the next one more certain the memory was empty. The header string itself is unchanged;
the caveat is its own line, and it holds with or without a `FACTS` section above it. The header ASSERTS — it says what the document IS and as of when, which is the one piece of
framing a reader can use. The run's own description is not gone: it is the first line of
`recall_diagnostic["text"]`, `MEMORY (tier 1, N candidates, RRF over …)` followed by the flat
ranked lines with their leg tags, byte for byte what the bundle used to show.

**The empty state says so, and hedges nothing (#297).** A recall that found nothing used to emit
the asserting header over no rows, and a model handed that reads it as "the lookup ran, this is
what memory holds" and answers from somewhere else. So over an empty list there is no header, no
completeness sentence (a hedge about a list that does not exist invites exactly the "there must
be more" reading) and one sentence instead:

* `Nothing in this memory answers this question (as of <day>).` — nothing about this was stored;
* `… : N stored hits came back for it and none of them cleared the relevance floor …` — something
  was stored and every hit died at a floor. `leg_sizes` reports 0 for both, so the discriminator
  is the pre-floor `leg_sizes_raw` map, read defensively (a missing raw count is never a floor
  claim).

The JSON says the same thing with `answers: "none"`, and the hop carries `recall_empty: "1"` —
absent unless true, like every other marker on that header, so a router can branch without
parsing the bundle it was handed on. `semantic_degraded` is deliberately NOT what this sentence
reads: it means the embedder produced no vector, and the floor empties the semantic list exactly
as a dead embedder does. "Hits came back and were floored" is visible as `leg_sizes` against
`leg_sizes_raw` in the diagnostic; the flag stays there, for whoever debugs the embedder.

**The five legs (tier 1).** Each leg returns a *ranked id list*; content is fetched once, at the
end, in a single hydration round. That is what keeps the fusion cheap and the scratch payloads
small.

| Leg | Store op | Ranked by | Yields |
|---|---|---|---|
| keyword | `search episodes` + `search facts` (bm25) | `rank`, merged facts-before-episodes on a tie | facts + episodes |
| semantic | `similar embeddings` | hamming `distance` | facts + episodes (via `owner_id`, kind from `owner_table` — GH #519) |
| graph | `select entities` → `traverse entity_edges` → `select facts` (the walked nodes) | depth, then accumulated weight — each node's episode, then that node's facts | episodes (via the edge's `episode_id`) + facts (via the node's `canonical_subject` — GH #520) |
| temporal | point mode: as-of `select facts` · window mode: a generous interval pre-filter, exact cut in code | point mode: `recorded_at` desc · window mode: `valid_from` desc, `recorded_at` desc, `id` asc | facts |
| self | as-of `select facts` over the ASKER'S OWN subjects (GH #536) | `recorded_at` desc, `id` asc — the one leg the question does not shape | facts |

**The self leg (GH #536).** A question in the first person — *"what are my sons called?"* — names
nobody, and every other leg is built from what the question named. The keyword leg has no lexical
overlap between *sons* and `has_child`; the graph anchors resolve the interrogative words against
`entities` and come back empty, so the walk never starts; the semantic leg competes with a corpus
of episodes; and the temporal leg has no vote in point mode. Measured on a two-week-old hive that
HELD the answer: `leg_sizes {keyword: 20, semantic: 20, graph: 0, temporal: 20}` and **not one
fact** among the 20 fused candidates.

The asker was never missing from the request. `audience_now` carries the participant set, and its
`member:`/`person:` tokens are people — the read path just never read it as an *identity*, only as
the audience gate's yardstick. The self leg reads it as one: the asker's subjects are
`memory_holder`, `user_id` and the member tokens of the asking round, plus
`MEMORY_SELF_LEGACY_SUBJECT` (default `user`), which is what the extraction lane wrote into
`facts.subject` before it wrote person names there. `agent:` tokens are deliberately not people:
the hive belongs to a MEMBER (ADR-0002 E1) and an agent is a lens on it.

Three properties are load-bearing. It rides the **fan's own bundle** — the identity is known at
request entry, so the leg waits on nothing and costs no round trip. It is a **leg of its own**
rather than an extension of the graph leg: a member's dossier would otherwise spend the graph
leg's whole `LEG_LIMIT` and starve the walk of every slot it has, and a wide walk would do the
same to the dossier. And it has a **budget** — see below. What it does *not* do is widen
disclosure: an anchor decides what is looked up, never what is seen, and every row it nominates is
measured against `audience_now` by the same gate as every other row.

**The dossier budget (`MEMORY_TIER1_SELF_BUDGET`, default 6).** The leg answers *who is asking*,
which is the same answer whatever is asked — so its rows are not competing on relevance and must
not be seated as though they were. DOSSIER FLOOD, measured live on the first build of this leg:
two different questions came back with a **byte-identical `FACTS` section**, because twenty
dossier rows had taken every slot the query-driven legs were competing for, and a weather fact
about a city outranked the fact that answered the question. So `fuse_rank` composes **three**
classes, not two: a fact only the `self` leg nominated gets at most `SELF_BUDGET` slots while
query hits are waiting; a fact a query leg *also* found is not a dossier row at all and is seated
with the rest, agreement bonus and everything. Leftover slots fall back to the dossier, so a
question nothing else answered — the case this leg exists for — is still answered by it.

**What was tried and rejected: the asker as a graph anchor.** Anchoring the walk on the asker as
well *does* reach a subject spelling the audience token does not carry (`member:alex` → the edge
`alex → Alex Example` → the facts written under the fuller name). But an asker is a **hub**, and a
hub anchor is true of every question: the walk came back full on every one of them, and the graph
leg then voted at full weight for twenty rows nobody had asked about. A leg whose rank list is the
same for every question is a constant, not a retrieval. The graph leg keeps the one property it
has — that the question named where it starts — and the asker lives in the leg with the budget.
The cost of that decision is named rather than hidden: a hive that holds one person under two
subject spellings (`user` and `Alex Example`) reaches only the spellings `self_subjects()` knows.
Joining those two is the identity axis's job (`subject_aliases`, the nightly identity round), not
the retrieval's.

**RRF.** `score(d) = (Σ_legs w_leg / (K + rank_leg(d))) × (1 + A × (agreement(d) − 1))`, `K = 60`,
weights 1.0 — except the temporal leg, whose weight depends on the mode (O-4, below) — and
`A = MEMORY_RRF_AGREEMENT`, default **0.5**, `0` restoring the plain sum. A candidate found by two
**voting** legs adds both terms *and* is multiplied once for the agreement: two legs that
independently found the same row is a different kind of evidence than one leg finding it a little
higher, and the plain sum leaves that difference in the leg tags, which nobody sums. `agreement`
counts only legs that VOTE. A leg with weight 0 contributes a zero term, breaks ties (O-2, below),
performs its own cut — and **nominates nothing**: a candidate no voting leg found does not reach
the ranking at all, and one that a voting and a non-voting leg both found has an agreement of one.
Every fused candidate carries the count in `recall_diagnostic`; `legs` keeps meaning discovery
(O-4b, below) and keeps listing the non-voting leg. Ties break by best rank, then leg priority (keyword,
semantic, graph, temporal, self), then — on an **exact** score tie — the temporal leg's own rank, and
only then kind and id: two identical requests produce byte-identical candidate lists.

**The temporal leg does not vote in point mode (P15 O-4).** In a point query the leg runs exactly
as before — it performs the as-of cut, and that cut is its whole value — but its term enters the
sum with weight `MEMORY_RRF_W_TEMPORAL_POINT`, default **0.0**. The evidence is a paired ablation
over 50 identical LongMemEval extractions: R@1 **84.0 against 74.0**, R@5 **98.0 against 96.0**,
**11 flips to 1** in favour of the unweighted arm (sign test p=0.0063), and in 100 runs (2×50) the
temporal leg was **never** the only leg that found a hit. What it did instead was crowd: an
as-of-recent fact rode into the top slot on the third leg while a two-leg keyword+semantic hit
sat below it. In **window** mode the weight stays at `MEMORY_RRF_W_TEMPORAL` — the single paired
win of the weighted arm was a temporal-reasoning question, and window mode is where the temporal
ordering is the answer rather than a vote (O-2). The tie-break is untouched by all of this: it
reads the temporal **position**, not the temporal score, so it still decides an exact tie at
weight 0.

**A weight of 0 is three things and not a fourth (Q8, #297).** A zero-weight leg still runs, so
it still performs its own cut — the as-of trim is the point of the temporal leg and survives the
weight. It still breaks an exact tie by position (O-2). It still appears in a candidate's `legs`,
because that field is discovery (O-4b). What it does **not** do is nominate: a candidate no
VOTING leg found never reaches the ranking, and a candidate a voting and a non-voting leg both
found has an `agreement` of one. Two consequences worth writing down, because both were wrong
once: `capped_legs` is derived from the voting legs only (a full page on a leg that nominates
nothing cannot have shortened the answer, and in point mode the temporal page is full almost
every time), and `agreement` in the diagnostic is the count of VOTING legs — never
`len(legs)`, which counts discovery and grows again when several hits of one axis collapse into
one candidate (O-4b).

**Why the temporal leg breaks an exact tie (P15 O-2).** Symmetric ranks across two legs (keyword
1/2 against temporal 2/1) produce bit-identical sums — `1/61 + 1/62` either way — so the tie is
not an edge case, it is arithmetic. Before the ruling the smaller **uuid** decided, and a uuid is
minted per run: the order was a coin flip between runs of the same corpus. A candidate the
temporal leg never saw now sorts behind one it did.

**Leg attribution belongs to the axis, not to the first winner (P15 O-4b).** When several hits of
one `(subject, predicate)` axis collapse onto a single candidate (below), the survivor carries the
**union** of their `legs`, deduplicated and in leg order. `found by leg X` is a statement about
**discovery**, not about vote weight, so dropping the legs of the swallowed hits would make the
field depend on which hit happened to rank first — it read correctly for years only by accident.
Score, rank and the fusion order stay the first winner's: this is attribution, never re-ranking.

**The candidate is a projection onto the version chain, not the raw hit (P15 R2/R4).** Facts of
one `(subject, predicate)` axis form a chain ordered by `(valid_from asc, recorded_at asc,
id asc)`. A fact's validity ends at its own `valid_until`, else where a materialised closure
(`expired_at`) says so, else at the `valid_from` of the next **strictly later assertion of the
same statement**, else it is open. The chain itself is not stored — it is recomputed per request,
from rows trimmed to `recall_as_of` first, so a question about the past is never answered with a
fact that did not exist yet.

| Mode | What a fact hit becomes |
|---|---|
| point (no window) | the **current** statement of the hit's axis, i.e. the one its closures lead to, carrying its predecessors: the whole chain as `history: [{id, claim, from, until}]` on the record in `recall_diagnostic` and in the dialectic payload, its LAST entry as `previously` in the bundle — in the JSON payload slot and in the rendered text block alike, out of one `history_entries` helper (#296, ruling S6). A CLOSED hit is therefore never a candidate of its own — it is a field on the statement that closed it. Two hits landing on one statement collapse into one candidate, and that candidate carries the **union** of both hits' `legs` (P15 O-4b) |
| window | **every** version stays its own candidate and carries `span: {from, until}` — in a time-range question the versions themselves are the answer, so nothing collapses |
| multivalued axis | untouched: no predecessors at all — no `history` on the record, no `previously` in the payload — every value stands (see below) |

**The supersession unit is the STATEMENT, not the axis (statement identity W2, GitHub #13,
ruling Q1).** A statement is `(canonical_subject, canonical_predicate, canonical_claim)` — the
value is part of the identity, derived by the same generic `params.canonical` binding the other
two dimensions use, with byte identity of the claim as the day-one canonical value. The axis stays
what `build_chains` returns and what the bundle renders: retrieval stays coarse, correctness moves
down. Two rules follow, and everything else about supersession is gone:

* **Order decides only within one statement.** A statement asserted again strictly later closes
  its own earlier assertion — one statement, two assertions, the newer one carries the value.
* **Across statement boundaries only an explicit closure closes anything** (ruling Q2): a write
  that sets `expired_at`/`superseded_by`, produced by the nightly judge (W3, below). `span_end`
  READS that write instead of computing a second answer next to it. Nothing the store can derive
  by itself closes across two values — the bundle shows coexistence until a judgement links two
  statements, which is the harmless direction, and the currency marker below names what IS
  closed.

The measurement behind the change: W1's session guard thawed 109 axes on the eight benchmark
stores, and a dream run under the old rule would then have materialised **928 closures instead of
49**, 636 of them on foresight facts (73 % of all plans), because a bucket axis such as
`planned_activity` (71 values on one axis) counted as one functional relation. The tier-0
foresight leg filters `expired_at is_null`, so each of those closures was an answer the store
would have stopped giving. Scenarios `C9` (replacement), `C10` (re-assertion plus change) and
`C11` (bucket stays open) pin the three relations separately.

**A closure names its author, or it is arithmetic (ruling Q2 guard rail 2).** `facts.closure_source`
is empty on a closure the chain re-derives by itself (a re-assertion) and carries the producer on
an explicit judgement. That distinction is what makes the re-derive safe in both directions: an
unattributed closure — everything the old axis arithmetic left behind — is **withdrawn back to
NULL** by the next re-derive (ruling Q4, the revert path for judgements), and an attributed one is
carried through untouched. Reading a closure needs no author, withdrawing one does.

**The nightly judge is the PRODUCER of those closures (statement identity W3, ruling Q2
option B).** The canonicalisation round carries a third section: every axis with more than one
OPEN statement is laid out with its values and their instants, and the judge answers which of
them are still true and which was replaced by which. Shape and guard rails:

* **Selection.** One entry per STATEMENT, never per row (two assertions of one claim are one
  statement, and the id offered is the assertion still standing). An axis carrying more open
  statements than one page is never shown as a whole axis -- a judge that sees six of seventy
  plans cannot tell it is a bucket -- and since GitHub #66 it is not dropped either but
  **triaged** (next section). Busiest axis first, capped by `MEMORY_CANON_MAX_AXES`.
* **Write surface.** Per YES verdict exactly one `update` on `facts`, setting `expired_at`,
  `superseded_by` and `closure_source = judge:<run_id>`. The `where` pins the row to the axis
  the verdict named AND to `expired_at is_null`: a verdict about an axis it was not shown finds
  no row, and an existing closure is never written over. The judge may only CLOSE — no delete,
  no edit of `claim`, `valid_from` or `valid_until`.
* **Refusals.** A verdict without a reason, without a successor, closing a statement with
  itself or naming no axis is dropped. The instant is COPIED by the judge from the payload;
  a garbled copy falls back to the night's own `delta_to`, which keeps a re-run identical.
* **Receipt and revert.** The reason of every closure lands in the `verdicts` payload of the
  run's `consolidation_log` row — the one place this lane quits a dream artefact — and
  `closure_source` names that run id, so a closed fact can always be read back to the sentence
  behind it. The revert is the alias one a dimension over: there is no closure table to delete
  from, so clearing the attribution (`update facts set closure_source = '' where closure_source
  = 'judge:<run_id>'`) plus the next re-derive withdraws exactly one night's judgement.

Pinned by `crates/meclaw-cells/tests/w3_judge_closures.rs` (free) and the scenarios C12 (free,
judgement handed in, both gate halves) / C13 (`--with llm`, a real judge on one replacement and
one enumeration) / C1 (`--with llm`, the closure on a corpus the extractor minted).

Since W4 the same section is also the REVIEW of the other producer: an axis whose only open
statement sits next to a recently extract-closed one is offered too, with a `closed` list naming
the value, the instant and the `closed_by` batch. The judge may answer `reopenings`, and that
verdict clears the attribution so the re-derive of the same night withdraws the closure. A night
on a store the extractor never closed on builds byte for byte the payload W3 built.

**The axes one page cannot hold are triaged, not skipped (GitHub #66).** The page rule above is
right about the danger and was wrong about the answer: on the track-end corpus it removed 72 of
185 multi-statement axes (39 %) from the round by construction, and those 72 were precisely the
bucket axes the currency question was opened for (`planned_activity`, `plans_to_*`,
`interested_in`, `has_experience`, `uses`, `practices`). Such an axis now leaves the scan as a
candidate in a `paged` section of its own, carrying the number of open statements it holds, and
the ask phase -- where the judged cardinality map is in hand -- decides what it gets asked.
Cardinality first, three outcomes:

| verdict of the RELATION (seed > judged) | what the axis gets |
|---|---|
| `multi`, enumerating | nothing, ever. Values coexist, so there is no closure to make; the constructional hole becomes an explicit judged answer |
| `single`, functional | one PAGE of the currency question a night: the most recent statements, with a `page` object naming `open_statements` and `shown` |
| none yet | no page tonight. The relation is already at the head of the cardinality question (that list is ranked by the busiest axis of a relation, and an over-cap axis is the busiest there is), so the next round knows which of the two rows above it owes this axis |

Three properties make the paging sound without a cursor table:

* **The page is the recency prefix of the OPEN statements**, and closing is the only way to leave
  that set. So every statement that survives a page is still among the most recent open ones and
  is on the next page too -- the current value is carried between pages by the same rule instead
  of by a second mechanism, and a judgement never compares two statements that were not in one
  prompt together. The closures are what advance the window; an axis worn down under the page
  bound becomes an ordinary entry again, with no rule anywhere noticing the transition.
* **The pages come OUT of `MEMORY_CANON_MAX_AXES`, never on top of it** (at most
  `MEMORY_CANON_MAX_PAGED_AXES` of the slots), so the night that finally reaches a bucket axis is
  not also the night every payload grows.
* **The judge is told it is looking at a page**, in a paragraph that renders only on the nights
  that carry one (the GitHub #69 section rule) and declares no new answer key. Without that
  sentence the page would be exactly the blind truncation the old rule refused to perform.

The coverage is stated in the run receipt, in the same `verdicts` payload the closures land in:
`pages` names how many over-cap axes the night saw, how many enumerate (answered for good), how
many still owe a cardinality verdict, and per page what it showed and what it left behind. It is
written by the ASK rather than by a verdict, so a night whose judge never answered still says
what it put in front of it. Pinned by `crates/meclaw-cells/tests/f5_bucket_axes.rs` (free, 16
cases) and the scenario C21 (free, four over-cap axes and two rounds).

**The round is a SECOND model call, and the books say so (GitHub #64).** A night that runs the
canonicalisation round calls two models -- the dreamer and the judge -- and until 0.3.1
`consolidation_log.llm_calls` reported 1 for both of them together. The lane now writes one
receipt per model call into `scratch` under the run key (`kind = 'llm-call'`, the usage read off
`hop.tokens_prompt` / `hop.tokens_completion` of the reply that carried it), and the close counts
them into `llm_calls` and sums them into `tokens_prompt` / `tokens_completion` on the same row.
Three properties keep the number honest:

* **Every exit of the round books its call**, the two that skip it included: a judge that errored
  or answered in prose was asked, answered and billed.
* **Tokens nobody reported stay NULL**, never 0 -- the rule GH #9 wrote for the embedding lane one
  lane over. A booked zero cannot be told apart from a call that really was free, and in the free
  scenario suite every model reply is injected and reports nothing at all.
* **One row per call, never an incremented counter**, so a crashed night that is re-run books what
  it really cost instead of what one pass costs. Embeddings are not in this row on purpose: the
  backfill leaves the lane *after* the run closes, and its tokens are accounted in the message log
  where GH #9 put them.

Pinned by `crates/meclaw-cells/tests/f3_run_books.rs` (free) and the scenario `C5`.

**And the same closing phase sweeps the lane scratch (GH #375).** Its op list is five entries
long: the supersession the arithmetic already emitted, the belief upserts, the embedding
backfill, **the sweep**, the books. `scratch` and `recall_scratch` are the parking places every
glue lane carries state across a store round trip in, and until now nothing ever removed a row
from either — the only `delete` in the whole hive stood on `predicate_cardinality`, which is a
re-judgement and not lane state. Both grew monotonically with the traffic, and since `3.0.0` one
closed **session** parks four rows plus the meeting that reads them back, so the growth was per
conversation rather than per batch. The night is the sweeper because the night is the one lane
with a clock, a window and a receipt.

* **Two ops, both bounded**: `delete from scratch where created_at < cutoff` and the same on
  `recall_scratch`, with `cutoff = delta_to − MEMORY_SCRATCH_TTL_DAYS` days. Nothing else is ever
  named — no memory table, no provenance, no durable row. The No-Delete policy keeps standing
  where it belongs, and these two tables are transient by their own definition.
* **The cutoff comes from `delta_to`, never from the clock**, like every other value a dream run
  writes — so a replayed window deletes the same rows. A window end this lane cannot parse sweeps
  **nothing**: an empty cutoff would render as an empty `where`, and a delete without a `where` is
  a table drop with extra steps.
* **It cannot break the close pass's meeting**, which is the one read that looks like it might.
  That read orders `created_at desc` and takes the FIRST row it sees per kind, so it consumes the
  **newest** parking — the head of an ordering a cutoff only ever cuts the tail of. Everything
  still in reach of a lane is younger than the cutoff by orders of magnitude.
* **It runs where the run closes**, so a night that never reached its apply phase (a dreamer that
  failed, a window already consolidated) sweeps nothing and the next night does it instead. The
  window is generous enough that a skipped night costs a day of rows, not a lane.

Pinned by `crates/meclaw-cells/tests/gh375_the_night_sweeps_the_scratch.rs` (free).

**A night describes the questions it HAS (GitHub #69).** The round stays one call carrying every
question the night asks -- the sections give each other context and splitting them would buy the
same answers twice. What the night no longer does is describe a question whose data section is
empty. The payload has left an empty section out since W5; the instructions were rendered whole
regardless, so a store with nothing to canonicalise still bought the full block (8.3 kB, about
9.9 k prompt tokens on the busiest night of the statement-identity track) every single night. Now
one set of question names is derived once in `canon-ask` and decides three things at the same
time: whether there is a call at all, which paragraphs are rendered, and which keys the answer
shape declares. A night whose only open question is a relation's cardinality carries 1.8 kB.

| instruction section | data section | answer keys |
|---|---|---|
| `1. predicates` + core vocabulary | `predicates` (two or more relations) | `predicates` |
| `2. entity_pairs` | `entity_pairs` | `entities`, `different` (dimension `subject`) |
| `3. axes` | `axes` | `closures`, `reopenings` |
| `5. same_value` | `axes` | `same_value`, `different` (dimension `claim`) |
| `4. cardinality` | `cardinality` | `cardinality` |

Three rules keep the shrinking honest. **A constraint lives with the question whose answers it
guards** — "do not merge two quantities" with the rewordings, "do not close an enumeration" with
the currency question, "names are verbatim" with the entity pairs; the two questions that
constrain each other across the boundary read the SAME section (`axes` carries 3 and 5), so they
are never rendered apart. **The numbering is fixed** — a question is `3.` on every night it is
asked, so no cross reference between the questions can go stale and a shrunken block describes the
same five questions the full one does. **A night with all five sections renders byte for byte what
it rendered before**, which is what lets "same verdicts as before" be pinned without buying a
model. Pinned by `crates/meclaw-cells/tests/f4_night_questions.rs` (free) and the scenario pair
`C19` (full night, invariance) / `C20` (quiet night).

**Coexistence makes an axis multivalued, for good (P15 O-1 + O-3, guarded since W1).** Two facts
of one axis that start at the *identical* `valid_from` **and arrived in two different sessions**
are coexisting values, not a change: `has_child Robin` does not end because `has_child Sam` was
stated in another conversation on the same day (names generalized from the original run).
Cardinality is therefore LEARNED from the data and
the verdict is monotone: once an axis has shown coexistence anywhere in its chain, it counts as an
**enumeration** from then on (`axis_is_multivalued`). Since W2 that verdict is a PRESENTATION
tie-breaker and nothing more — it answers whether an axis has a "current value" to collapse a hit
onto at all, and an enumeration has none, so every hit of one stays its own answer with no
predecessors. It no longer derives or blocks a single closure, which is what makes a wrong
cardinality verdict harmless: it can fail to mark an outdated value, it can no longer end a true
one.

**The session guard is the half the data forced (statement identity W1, GitHub #13, ruling Q3).**
Until 0.2.0 the identical instant was enough on its own, and the 0.2.0 P8a run measured what that
costs: a benchmark corpus stamps one instant per *session* and a live lane stamps one per *turn*,
so two facts extracted from ONE conversation are indistinguishable from two values that genuinely
started together. 143 of 187 multi-version axes were classified as enumerations that way, at least
103 of them by this rule alone, and their version chains never fired again. Since W1 the evidence
must cross a session boundary. The origin is read from `facts.session_id`, a column the write path
stamps from the episode a fact was extracted from: the store has no joins, and asking `episodes`
per axis would put a round trip on every recall. A fact without a session (a row from before the
column) proves nothing, which is the conservative direction: the error moves from "keeps an
outdated value" to "fails to mark one", and the seeded list stands in front of the rule for the
cases learning structurally cannot see. Scenario `C7` pins both directions in one store.

**Since 0.2.0 P2 cardinality is also SEEDED** (ruling Q4, the interim for the statement-identity
issue): a canonical predicate on the `CORE_MULTI` half of `predicate-core.json` counts as an
enumeration from its FIRST value, before two values ever coexisted in a write. That is the case
learning structurally cannot see — `has_child` whose second value arrives strictly later — and
before the seed the second child ENDED the first, which is a wrong answer rather than a missing
one. The list is keyed on the canonical predicate, which is why canonicalisation had to land
first: cardinality is a property of the relation, not of a spelling. Scenario `C3` pins both
directions in one store (enumeration coexists, judged closure on the single-valued axis still
fires). The residual gap the seed was an interim for — a multivalued predicate that is NOT on the
list and never coexisted used to supersede — is closed by W2: a different value is a different
statement on any axis, listed or not.

**And since W5 it is also JUDGED (GitHub #13, ruling Q3 option C).** The read stack has three
sources with a fixed precedence — **seed > judged > learned-with-session-guard** — and the middle
one is a small table, `predicate_cardinality (canonical_predicate, verdict, source, decided_at)`.
The nightly round offers the relations whose axes carry more than one open value, leaving out the
ones the seed list owns and the ones it has already decided (so the budget always reaches
something new); a verdict becomes one attributed row, `source = judge:<run_id>`, and the sentence
behind it lives in the run receipt under the same run id. `source` is not decoration: ruling Q3
asks that "why does this axis enumerate" be answerable from the data, and one night is reverted
with one `delete` on that string plus the next re-derive. The seed precedence is enforced at the
WRITE end as well — a verdict about a seeded relation is refused, so a store can never hold a row
that contradicts the authority. The read path fetches the map as a LEG of the hydration fan
(`t1-hyd-card`), fired together with the axis page, so the verdict costs an op and not a hop on
the lane that runs while somebody is waiting. What it decides is still only presentation (W2's
rule stands: no closure is derived from it), which is exactly why it may be a judgement at all.
Pinned by `crates/meclaw-cells/tests/w5_judged_cardinality.rs` and the scenario `C16` (free): two
identically built axes, one closure each, and the cardinality verdict as the only difference
between them.

**The fallback is a candidate without predecessors, never a lost candidate.** If the axis select
was truncated (`MEMORY_TIER1_AXIS_LIMIT`) or carried no matching `(subject, predicate)` pair, the
hit is delivered as it came — `history: []` on the record, and therefore no `previously` key in
the payload. Losing a candidate would be the worse failure.

**A superseded candidate says so on its own line (statement identity W1, GitHub #13, ruling Q6).**
`previously:` says what a candidate REPLACED; the inverse was missing, and the P8a run measured
the price: its one wrong judged answer had both values in the bundle, the superseded one ranked
first, and nothing marking it. A candidate whose statement the store has closed therefore carries
`superseded: <newer claim>` in the JSON and ` (superseded by: <newer claim>)` in the rendered text
and ` (superseded: yes)` when the axis page does not carry the successor, which is the honest form
for a closure whose replacement is out of reach. Three properties keep it narrow:

* **It reads the STORED columns**, `expired_at`/`superseded_by`, i.e. what a dream run
  materialised. That is what makes it the inverse of the read-time `previously:`, and what will
  let a judged closure (W3) show up here without a second mechanism.
* **A closure in the future of the read instant has not happened yet.** As-of questions keep spec
  D.5: the annotation never leaks the existence of a value that did not exist at the time asked.
* **Nothing is dropped and nothing is re-ranked.** A superseded value stays a candidate of the
  bundle (a question about what changed needs it, and hiding it would hide the store's own
  uncertainty), the `superseded` key is absent on an open statement, and a line without it renders
  byte for byte as before. Demoting closed
  statements below open ones within an axis is a *ranking* change and a separate package.
  Scenario `C8` pins both modes of one axis.

**Verbatim repetition is one line, not N slots (0.2.0 P6, GitHub #15).** The fusion cut gives
episodes a bounded share of the bundle; inside that share a question asked five times used to
take five slots holding one string. Episode candidates whose content is EXACTLY equal after
normalisation therefore collapse into a single line at emit time. Three properties make that
safe to read:

* **The key is the store's own normal form** (`normalize_text`, the pinned twin of
  `crates/meclaw-cells/src/store/query/normalize.rs`): case fold, Latin-1 composition,
  whitespace collapse — the same statement about identity the canonicalisation round makes, and
  nothing beyond it. Two questions that merely look alike stay two lines.
* **The slot is the best-ranked copy's, the wording is the newest copy's.** Rank, score and the
  fusion order are untouched (this is presentation, never re-ranking), and the legs of the
  swallowed copies are merged into the survivor exactly like the axis collapse does (O-4b). The
  line then carries `seen: N` in the JSON and ` (seen: N)` in the rendered text; a line that
  never repeated renders byte for byte as before.
* **Nothing is deleted.** Level 0 stays append-only — all N rows remain in `episodes` and remain
  retrievable. The collapse lives in the bundle and nowhere else, which is why it needs no
  identity judgement about the episodes themselves.

**Degradation is arithmetic, not a special case.** An empty leg contributes no term, so a dead
embedder makes the fusion mathematically identical to a three-leg fusion. The query lane of
`embed` therefore *always answers* — with a vector or with `degraded: true`. Silence would hang
the fan-in forever, which is exactly why the write lane's "stay silent on failure" rule is NOT
mirrored on the read side.

**Known limitation — the byte-identical promise holds only modulo the remote embedder.** The read
path is deterministic end to end, but the query VECTOR is not: measured on the LongMemEval mid
stage (2026-08-11), one identical query text produced **2 distinct vectors and 3 distinct semantic
orderings across 8 recalls**. The provider is the source (batching / hardware non-determinism), so
"same `cell.db` + same request ⇒ byte-identical bundle" is exact with a dead or local embedder and
approximate with a remote one. Registered in `docs/defer-register.md`, not worked around here.

**Cross-model invariant.** `similar` is never called without a `model_id` filter, and the value
comes from `emb_models` where `active = 1`. No active generation, or no query vector, means an
**empty** semantic leg — never an unfiltered ranking across embedding generations. Proven with
two generations of different vector length in one `cell.db`.

### Rotating the embedding model is an OPERATOR job, and the template will not do it for you

`emb_models` is a **seed** table: `seed/emb_models.jsonl` is loaded once, when the store cell
first creates its `cell.db`, and never again. Changing `MEMORY_EMBED_MODEL` on a running colony
therefore changes which model new vectors are COMPUTED with and changes nothing about which
generation the read path SELECTS: `active = 1` still names the old `model_id`, so the semantic leg
goes on filtering for the old generation and never sees a single new fact. There is no error and
no log line — the leg simply answers with the vectors it is allowed to see, and keyword, graph and
temporal quietly carry the recall. The nightly backfill does not rescue it either: it re-embeds
rows `where status: "queued"`, and the existing rows are `ready`.

Three store ops, in this order, are the rotation:

```jsonc
// 1. register the new generation (dim MUST match MEMORY_EMBED_DIM)
{"operation": "insert", "table": "emb_models",
 "row": {"model_id": "<new>", "provider": "openai-compatible",
         "endpoint_ref": "MEMORY_EMBED_ENDPOINT", "dim": 1024, "active": 1,
         "created_at": "<now>"}}

// 2. retire the old one -- exactly one generation is active at a time, because
//    the read path filters on `active = 1` and two actives is an unfiltered
//    ranking across generations, the one thing the invariant above forbids
{"operation": "update", "table": "emb_models",
 "set": {"active": 0}, "where": {"model_id": "<old>"}}

// 3. hand the whole standing corpus back to the backfill
{"operation": "update", "table": "embeddings",
 "set": {"status": "queued"}, "where": {"model_id": "<old>"}}
```

Between step 2 and the end of the backfill the semantic leg is **empty**, not wrong — the other
three legs answer, which is the same degradation as a dead embedder. Order matters: retiring the
old generation before registering the new one leaves the memory with no active generation at all
for as long as the gap lasts.

`embeddings.binarization_version` is written on every row and **read nowhere** today. It exists so
that a future change to the packing can be told apart from the vectors already stored; a filter on
it would be additive, and until one exists a rotation is identified by `model_id` alone.

### Backfilling the episodes of a hive older than GH #519

Until `memory-hive@3.1.0` only facts were embedded, so the semantic leg of the tier-1 fan could
only ever return facts — and an episode is raw conversational text, exactly the material a lexical
index is worst at. `./writer` now mints a queued `embeddings` row beside every episode it writes
and sends the turn's text straight to `./embed`, so a hive grown from 3.1.0 needs nothing here.

A hive that already holds episodes has to mint the missing rows once. That is a **store op**, not
a script: the nightly chain in `./dream-glue` fills every `status: "queued"` row it finds, of
either owner kind, so minting the rows IS the backfill and the night is what pays for it.

```jsonc
// one row per episode that has none, minted through the hive's own store lane.
// `blob` stays NULL and `status` stays `queued`: that pair is precisely what the
// nightly backfill selects, and what makes a repeat run a no-op rather than a
// second charge.
{"operation": "insert", "table": "embeddings",
 "row": {"id": "<uuid>", "owner_table": "episodes", "owner_id": "<episode id>",
         "model_id": "", "dim": 0, "binarization_version": "", "blob": null,
         "status": "queued", "created_at": "<now>"}}
```

Two properties are worth stating, because both are what keeps this safe to run against a live
memory. The insert touches no episode, no fact and no index — it adds rows to a table whose only
reader is the semantic leg, and a queued row with a null blob is invisible to that leg
(`status: "ready"` is part of its filter). And it is idempotent by *selection*, not by the store:
minting a second row for an episode that already has one costs a second embedding, so the id set
is taken from `episodes` minus the `owner_id`s already in `embeddings`.

The cost is one embedding call per episode, once. The fill then runs on the next nightly firing,
along with every fact whose own embedding never landed.

**The gap statement is enforced, not hoped for.** `recall` parses the dialectic answer; if the
`gap` field is missing or the answer is not JSON, the answer is still delivered but carries
`gap_missing: true` in the body and `hop.gap_missing='1'`. A provider error downgrades to the
tier-1 candidates with `hop.tier_downgraded='1'` — never silence.

**No gates, no guard rows** (GH #418). There were three — the semantic join, the main fan-in and
the hydration fan-in, each running the P2 collector protocol
(`insert → select → guarded update → select → fire`) on its own guard row (`qvec`, `kw-ep`,
`fused`). All three are gone, and so are the rows: the trailing `select` of a bundle elects the
same hop for free, because the `store` runs a bundle's ops in order over its one connection.
Explicitly withdrawn: **nothing on the tier-1 read path writes or reads `fired` any more.**

Cost: a tier-1 recall is **six** store round trips, seven when the graph leg has nodes to join
(GH #520) — see [And six for tier 1](#and-six-for-tier-1-gh-418-seven-when-the-graph-leg-joins-gh-520).

## Temporal questions — what is answerable, and what is not

Three shapes are answerable, and they differ only in what the caller puts in the hop:

| Question | Hop | What comes back |
|---|---|---|
| "what does Alex use?" (now) | `recall_as_of: ""`, both window keys `""` | the current fact of each axis, with the claim it replaced attached — one entry as `previously` in the JSON payload slot and the same one entry in the rendered `(previously: …)` annotation beside it, and the full `history` records in `recall_diagnostic` |
| "what did he use in May?" (an instant) | `recall_as_of: "2026-05-01T00:00:00Z"`, both window keys `""` | the chain trimmed to that instant first — the answer is what was true THEN, not what is true now with a date attached |
| "what did he use between March and now?" (a range) | `recall_window_from` + `recall_window_to` both set | every version whose derived span intersects the window, newest validity first, each with its own `span` |

**"And what was it before?" is a field, not a second query.** The predecessor travels with the
current candidate — one entry as `previously` in the JSON payload slot and the same one in the
rendered text block beside it, the full `history` records in `recall_diagnostic` — so
the change is answerable from one recall, which is the whole reason supersession moved to read
time.

What is deliberately **not** answerable:

- **Ablaut and other irregular morphology.** `ging`/`gehen` still do not meet. Since 0.2.0 P3 the
  index and the query both run through the `meclaw_stem_v1` tokenizer, so regular inflection DOES
  meet in both directions (`lieblingseditoren` reaches `lieblingseditor`, `editors` reaches
  `editor`) — but the stemmer is a conservative SUFFIX stripper by ruling Q6, and a vowel change
  inside the stem is out of its reach. A lexicon or a full Snowball would be the next step, and
  the ruling deliberately did not take it: over-stemming German compounds costs more than these
  forms are worth.
- **A window in tier 0** — but no longer in silence (0.2.0 P7, GitHub #16). Tier 0 has fixed legs
  (session episodes, active beliefs, open foresight) and no temporal leg to cut, so a COMPLETE
  window cannot be honoured there. It is now stated instead of dropped: the bundle carries
  `window_ignored: {from, until}`, the rendered text block carries a
  `- window_ignored: <from> -> <until>` line, and the hop carries `window_ignored = "1"`. Without
  a window none of the three appears. Deliberately NOT a reject (callers send the keys on every
  hop by contract) and NOT an auto-upgrade to tier 1 (that would change the cost of a request
  unannounced). If the question has a time range, ask tier 1 or 2.

## Idempotency

| Mechanism | Where |
|---|---|
| `claim_hash = sha256(episode_id\|subject\|predicate\|claim)[:16]`, filtered before insert | inline extraction |
| guarded `update … set {status:'inline'\|'nothing'\|'close'} where {episode_id in …, status:'pending'}` | the coverage guard (#52, #298, #300) -- the annotation of a turn settles that turn's row, and the value names the reader: `inline` when the front model's block carried content, `nothing` when its verdict was an honest empty one, `close` when the annotation came from the close pass. Guarded on `pending`, so re-running the same annotation moves nothing a second time and a settled row keeps the verdict that settled it |
| guarded `update … set {status:'close'} where {session_id, status:'pending'}` | the close pass's sweep (#300) -- the second writer of `status`, and the last one: when a pass finishes a session it settles the rows of that session nobody ever annotated. Both writers guard on `status:'pending'`, so whichever lands first wins and neither overwrites a settled row; the two are the ONLY writers besides the enqueue (no claim, no gate, no recovery sweep survived #298), which is what keeps `pending` readable as exactly one thing -- a turn nobody has answered for yet |
| `select episodes where {session_id, sender:'user'} order by recorded_at desc limit 1` | the inline BIND -- the turn a block that names none is speaking for. `sender` is what makes it deterministic: the answer's own episode is written by the same per-turn lane, concurrently, so "newest episode" would be a race and "newest user turn" is not |
| the trailing `select recall_scratch` of a parking bundle | the exactly-once election of the tier-1 read path. Of two hops that park concurrently exactly ONE reads a complete set, because the `store` is stateful and a bundle is one message — and a leg that arrives TWICE is a duplicate, not a complete set, so it parks (loudly, on stderr) instead of emitting a second time. **Explicit withdrawal (GH #418):** the guarded `update recall_scratch set fired=1 where {request_id, leg, fired:0}` this row used to describe no longer exists, and no read path writes or reads `fired`. Tier 0 has had no gate to guard since 2.3.4 — see [One round trip for tier 0](#one-round-trip-for-tier-0-gh-295) |
| window guard on `(delta_from, delta_to)` and on `run_id` | dream lane, stage 2 |
| `set_alias` upserts on the alias, `reject_pair` upserts on the ordered pair, `canonicalize` reports only the rows that MOVED | canonicalisation round — re-judging a pair writes no second row, and a second run over unchanged data reports 0 |
| every dream write derives its timestamp from `delta_to`, belief ids from `sha256(holder\|statement)` | dream lane, stage 3 — replay is byte-identical |
| `max_concurrency: 1` on `extract-glue`, `dream-glue`, `recall`, `porter` | serialises the read-modify-write handlers (a `code` cell is a stateless dispatcher and would otherwise run them in parallel). `embed` is the one glue cell that is deliberately **not** serialised (`max_concurrency: 4`): it writes one column of one row per message and holds no chain state between them |

Missed timer firings are **never** replayed. They do not need to be: the next run's window
starts at the last completed `delta_to`, so a skipped night is covered automatically.

## Gate runbook

All commands from the repo root, binary `target/debug/meclaw`. The probe harness lives in the
maintainers' workshop tree (`workshop/fixtures/positive/memory-hive-probe`, not part of the
published repository) and is two cells around the hive: `anchor` mints the hop headers of every
port, `capture` is the terminal receipt sink. Rebuild it from the mutation below — the runbook
is reproducible against any colony root that carries those two cells.

```bash
# --- G1: validate (against a root no daemon has touched) -----------------
target/debug/meclaw --validate --root workshop/fixtures/positive/memory-hive-probe \
  --templates templates ; echo "exit: $?"        # 0

# --- Prepare a run colony ------------------------------------------------
RUN=workshop/workspace/p2-run
rm -rf $RUN && cp -r workshop/fixtures/positive/memory-hive-probe $RUN
# FOUR variables have no default and must be present, or the instantiation rejects
# env_var_missing (negative fixture memory_hive_env_missing pins exactly that).
printf 'OPENROUTER_API_KEY=dummy\nMODEL_CLOSER=dummy/close\nMODEL_DREAMER=dummy/dream\nMODEL_DIALECTIC=dummy/dialectic\n' > $RUN/.env
target/debug/meclaw --root $RUN --env $RUN/.env --templates templates \
  --daemon --api 127.0.0.1:7792 &

# --- G2: boot smoke ------------------------------------------------------
curl -s http://127.0.0.1:7792/health                      # ok
curl -s http://127.0.0.1:7792/colony/templates | grep -o memory-hive

# --- G3: instantiation + liveness, ONE mutation --------------------------
curl -s -X POST http://127.0.0.1:7792/colony/mutations -H 'Content-Type: application/json' -d '{
 "scope":"/","ctx":{},"diff":{
  "add_nodes":[{"name":"memory","template":"memory-hive"}],
  "add_edges":[
   {"from":"./anchor","to":"./memory","condition":"has(hop.route) && hop.route == \"write\"",
    "modifier":{"set_hop":{"route":"'"'"'in_episode'"'"'"}}},
   {"from":"./anchor","to":"./memory","condition":"has(hop.route) && hop.route == \"recall\"",
    "modifier":{"set_hop":{"route":"'"'"'in_query'"'"'"},
     "set_context":{"recall_query":"hop.recall_query","memory_tier":"hop.memory_tier",
     "recall_as_of":"hop.recall_as_of","recall_window_from":"hop.recall_window_from",
     "recall_window_to":"hop.recall_window_to"}}},
   {"from":"./anchor","to":"./memory","condition":"has(hop.route) && hop.route == \"inline\"",
    "modifier":{"set_hop":{"route":"'"'"'in_remember'"'"'"}}},
   {"from":"./anchor","to":"./memory","condition":"has(hop.route) && hop.route == \"close\"",
    "modifier":{"set_hop":{"route":"'"'"'in_remember'"'"'"},
     "set_context":{"close_pass":"'"'"'1'"'"'"}}},
   {"from":"./memory","to":"./capture","condition":"has(hop.route) && hop.route == \"bundle\""},
   {"from":"./memory","to":"./capture","condition":"has(hop.route) && hop.route == \"reject\""}]}}'
curl -s http://127.0.0.1:7792/colony/registry     # 13 hive cells active; cron Awake
```

`store`, `closer`, `dreamer` and `dialectic` show `active=true` + `NotYetSpawned` — that is
the correct hot/cold PASS form for stateful cells, they wake on first delivery. Only the
long-running `cron` must be `Awake`.

**G4** — two negative fixtures in the same workshop tree pin the rejection paths:
`memory_hive_env_missing` (`env_var_missing`; the four no-default variables) and
`memory_hive_unknown_port` (guards against naming a cell of the hive from outside — `./memory/decay`;
under the seal every such endpoint is refused, whether the cell exists or not). Both are one-line
variations of the positive root above.

## Probe sequences

The harness drives every seam with one control JSON posted as a user turn, so **no probe needs
a live LLM**. The full build receipt of the template (commands + observed output of every probe)
is a maintainers' process paper and is not part of the published repository.

```bash
POST() { curl -s -X POST http://127.0.0.1:7792/messages -H 'Content-Type: application/json' -d "$1" >/dev/null; }
DB=workshop/workspace/p2-run/main/memory/store/cell.db
DUMP() { python3 - "$DB" <<'PY'
import json,sqlite3,sys
c=sqlite3.connect("file:%s?mode=ro"%sys.argv[1],uri=True); c.row_factory=sqlite3.Row
MEM=("episodes","facts","topics","entities","entity_edges","beliefs","embeddings",
     "emb_models","consolidation_log","skills")
print(json.dumps({t:sorted(json.dumps(dict(r),sort_keys=True) for r in c.execute('select * from "%s"'%t))
                  for t in MEM}, indent=1, sort_keys=True))
PY
}

# write a turn, then recall it
POST '{"target":"/anchor","headers":{"session_id":"s1","turn_id":"t1"},"body":{"messages":[{"origin":"user","type":"text","text":"{\"probe\":\"write\",\"content\":\"Alex isst ketogen.\"}"}]}}'
POST '{"target":"/anchor","headers":{"session_id":"s1"},"body":{"messages":[{"origin":"user","type":"text","text":"{\"probe\":\"recall\",\"query\":\"was isst alex\",\"tier\":\"0\"}"}]}}'
curl -s 'http://127.0.0.1:7792/colony/messages?to_path_prefix=/capture'   # the bundle

# inline ingress: valid / garbage / duplicate
POST '{"target":"/anchor","body":{"messages":[{"origin":"user","type":"text","text":"{\"probe\":\"inline\",\"payload\":{\"facts\":[{\"episode_id\":\"<id>\",\"subject\":\"user:alex\",\"predicate\":\"diet\",\"claim\":\"isst ketogen\",\"fact_kind\":\"world\",\"confidence\":85}]}}"}]}}'
POST '{"target":"/anchor","body":{"messages":[{"origin":"user","type":"text","text":"{\"probe\":\"inline\",\"payload\":{\"facts\":[{\"nonsense\":true}]}}"}]}}'

# canonicalisation round: hand a judgement in instead of buying one (0.2.0 P5)
POST '{"target":"/anchor","body":{"messages":[{"origin":"user","type":"text","text":"{\"probe\":\"judged\",\"run_id\":\"<uuid>\",\"to\":\"2026-08-12T03:00:00Z\",\"judgement\":{\"predicates\":[{\"alias\":\"Lieblingseditor\",\"canonical\":\"favorite_editor\"}],\"entities\":[{\"alias\":\"user:u1\",\"canonical\":\"user\"}],\"different\":[{\"dimension\":\"subject\",\"left\":\"site:alpha1\",\"right\":\"site:alpha2\"}]}}"}]}}'

# close pass: hand the verdict in instead of buying one (W5, #300)
# The port edge stamps close_pass='1', which is the ONE thing that lets `shown`
# park the window every `replaces` is checked against -- edge truth, never a body
# claim. Same validator and same apply phase as any annotation block.
POST '{"target":"/anchor","headers":{"session_id":"s1"},"body":{"messages":[{"origin":"user","type":"text","text":"{\"probe\":\"close\",\"payload\":{\"facts\":[{\"episode_id\":\"<id>\",\"subject\":\"user:alex\",\"predicate\":\"diet\",\"claim\":\"isst seit Juli ketogen\",\"fact_kind\":\"world\",\"confidence\":90,\"replaces\":\"<fid>\"}],\"shown\":[{\"subject\":\"user:alex\",\"predicate\":\"diet\",\"statements\":[{\"id\":\"<fid>\",\"claim\":\"isst ketogen\",\"since\":\"<ts>\",\"last_asserted\":\"<ts>\"}]}]}}"}]}}'
# the same block WITHOUT `shown`: the closure is dropped, the fact is still minted
POST '{"target":"/anchor","headers":{"session_id":"s1"},"body":{"messages":[{"origin":"user","type":"text","text":"{\"probe\":\"inline\",\"payload\":{\"facts\":[{\"episode_id\":\"<id>\",\"subject\":\"user:alex\",\"predicate\":\"diet\",\"claim\":\"isst seit Juli ketogen\",\"fact_kind\":\"world\",\"confidence\":90,\"replaces\":\"<fid>\"}]}}"}]}}'

# dream lane: same window twice -> identical memory state
POST '{"target":"/anchor","body":{"messages":[{"origin":"user","type":"text","text":"{\"probe\":\"dream\",\"run_id\":\"<uuid>\",\"to\":\"2026-08-07T20:00:00.000000Z\"}"}]}}'
POST '{"target":"/anchor","body":{"messages":[{"origin":"user","type":"text","text":"{\"probe\":\"verdict\",\"run_id\":\"<uuid>\",\"to\":\"2026-08-07T20:00:00.000000Z\",\"verdicts\":{\"supersede\":[{\"old_fact_id\":\"<fid>\"}],\"beliefs\":[{\"holder\":\"self\",\"statement\":\"...\",\"confidence\":75}]}}"}]}}'
DUMP > /tmp/dump1.json   # replay both, then DUMP > /tmp/dump2.json; diff must be empty
```

`DUMP` covers **memory state**. The lane bookkeeping tables (`scratch`, `recall_scratch`,
`pending_extraction`) are append-mostly *inside* their retention window — a replayed run stages
its payload again — so they are explicitly *not* part of the replay claim. The claim is: **a
replay changes no memory.** Since GH #375 the first two are also not append-*only*: the nightly
run deletes what is older than `MEMORY_SCRATCH_TTL_DAYS`, which is lane state and never memory.
`pending_extraction` is deliberately left alone — it is an exception list with a defined meaning
per row, not lane scratch.

## P3 migration (store query layer)

Three reads moved from "fetch everything, filter in python" to store predicates:

| Lane | Phase | Now |
|---|---|---|
| `recall` | `leg-episodes` | `order_by recorded_at desc, id asc` + `limit` |
| `recall` | `leg-beliefs` | `where {holder, active: 1}` + `order_by updated_at desc, id asc` + `limit` |
| `recall` | `leg-foresight` | the as-of predicate: `expired_at is_null` + `valid_until or_null gt now` + `order_by/limit` |
| `dream-glue` | `scope` (both call sites) | `expired_at is_null` + `recorded_at lte delta_to` |
| `dream-glue` | `sup-scope` → `sup-axes` | live axes, then those axes WHOLE via `canonical_subject in [...] + canonical_predicate in [...]` (0.2.0 P2/P4) |
| `dream-glue` | `embed-facts` → `embed-owners` | reads the queue back and looks up only its owners via `id in [...]` |
| `extract-glue` | `vocab-fetch` → `vocab` (GH #68) | the axis hint as a store-side `distinct` + `order_by`/`limit`; the window as a recency page that picks the axes and a second, axis-major read that fetches them whole |

**Supersession is materialised, not judged here (P15 R9, narrowed by W2).** `dream-glue` does not
read `supersede` from the dreamer's verdict; it rebuilds the `(canonical_subject,
canonical_predicate)` chains itself and writes the AUTHORITATIVE state of the two closure columns
per fact, from two sources in this precedence: an explicit closure that names its author (carried
through unchanged -- a judgement is reverted by withdrawing it, never by out-computing it), then a
strictly later re-assertion of the same statement.
Everything else becomes `(NULL, NULL)`, which is how ruling Q4's withdrawal works: a triple is
emitted for EVERY fact, only differences are written, so a closure the old axis arithmetic left
behind is cleared and an attributed one is a no-op. Two selects, not one: the first asks which axes
are live, the second loads those axes **whole**, because only a full chain can tell a re-assertion
from a new statement. The second select carries no delta bound on purpose: the window bounds the
dreamer's LLM payload, not arithmetic. The page bound is `${MEMORY_DREAM_AXIS_LIMIT:-5000}`, and a
**full** page skips the derivation instead of guessing from half a chain. A second run over
unchanged data emits nothing at all. What still needs the model: the change narrative and the dedup
verdicts — and, from W3 onwards, the closures themselves.

**`valid_until` and `expired_at` are two columns because they are two questions (GH #65).**
`valid_until` is a DECLARED end of validity, written by whoever minted the fact; `expired_at` is an
explicit closure, written by a judge, by the extraction lane's `replaces`, or by the re-assertion
the chain derives. Until 0.3.1 the night mirrored the first into the second, and the tier-0
foresight leg asks both questions at once (`expired_at is_null` **and** `valid_until or_null gt
<recall instant>`), so a plan whose deadline had NOT passed yet fell out of the foresight bundle on
the first night after it was written. The deadline comparison stays where it already was, at read
time against the recall instant; the write-time copy is gone. Nothing on the read path moved with
it: `span_end` reads `valid_until` first and always did, so it answers exactly what it answered
before while the cache column stops carrying a second copy of the same instant. A store that still
carries mirrored values heals itself on the next round through ruling Q4's withdrawal -- the
mirrors carry no `closure_source`, so the re-derive clears them back to NULL.

**The python filters and sorts stay.** They are residual guards, not leftovers: they are broader
than SQL can express (e.g. an empty string counts as "open", not just NULL), and keeping them makes
the migration result-identical by construction — the store only stops shipping rows that would have
been dropped anyway. Receipt: three tier-0 bundles before and three after the migration, against the
same `cell.db`, are byte-identical down to the UUIDs.

Two reads stay unfiltered on purpose: the `consolidation_log` scan in `window-eval` (the re-run
guard needs the complete log) and the known-belief id set in `merge-beliefs` (its candidates only
exist after the verdict phase). Both are P5 candidates.

**FTS**: `store` declares `params.fts` (`episodes.content`, `facts.claim`, and since P15 the
predicate — since 0.2.0 P2 as `facts.canonical_predicate` instead of the written one, ruling Q3,
plus `facts.canonical_subject` since 0.2.0 P4, so an entity name is searchable under the identity
the store owns rather than under whichever spelling a turn happened to use).
The index is created once per `cell.db` and rebuilt exactly once at creation,
so a store that has been running without it catches up on the next spawn — rows written earlier
become searchable. Since P5 the tier-1 keyword leg consumes it. Caller text never reaches the
matcher unquoted: the query is tokenised, stop-worded and re-assembled as `"tok"* OR "tok"*` —
the trailing star is P15 R7, and it is what lets a singular query token reach a plural index
term. Since 0.2.0 P3 the index declares the store's own tokenizer (`tokenize='meclaw_stem_v1'`), so
both sides of a search are folded onto one term and the OTHER direction works too — a plural
question reaches a singular index term, which is what issue #14 was about. Known limit: only
tables from `params.schema` can carry an index, never one created at runtime via `create_table`.

**The recall lane cuts words, it does not fold them** — folding lives in exactly one place, and
that place is the store's tokenizer above. `tokens_of` in `./recall` splits a question into words
on the Unicode-aware word class (a letter of any script, a digit, the underscore, plus the hyphen
of a compound name) and hands them over as written; `unicode61` then case-folds and strips the
diacritic on the query text exactly as it did on the indexed text, so the two meet. GH #518 is
what the other arrangement costs: the splitter used to keep only ASCII letters, so a German word
arrived at the matcher as a fragment (`Söhne` → `hne`) or, when every fragment fell under the
three-character floor, not at all (`Größe` → nothing) — and the graph anchors, which are matched
against entity names exactly, named nobody. The rule generalises past German: any script whose
letters are not ASCII is searchable for the same reason.

**Growing `params.fts` on an EXISTING `cell.db` is now a supported migration (P15 R8).** Adding
`predicate` to a declared index used to be a loud `fts column drift` failure at spawn. Since
`5326290` a **purely additive** drift — the existing columns are a true prefix of the declared
ones — drops and rebuilds the index (`DROP` + `CREATE` + one `rebuild`, plus the triggers, which
are external-content-index state and must be rebuilt with it). Any other drift (removal,
reordering) stays loud. An FTS index is a rebuildable projection over never-deleted source text,
so the drop destroys no truth — the same property the embedding generations already rely on.

**0.2.0 P2 added the second permitted drift class, `canonical`:** the existing list becomes the
declared one by replacing a binding's `source` column with its `target`. That is a same-length
SWAP, not an append, so the additive rule would have refused it and an existing store would have
come up without its index. Same treatment (drop + rebuild + triggers), same reasoning, and it is
tied to the DECLARATION — without the binding the very same swap is still loud. The column
migration underneath it is generic: `apply_schema_ddl` now grows an existing table into its
declaration via `ALTER TABLE ADD COLUMN`, which is how `canonical_predicate` reaches a `cell.db`
that has been running for weeks.

**0.2.0 P4 made the first two classes COMPOSE.** The substitution runs first, the additive rule
then over its result. Without that a store still carrying the P15 shape (`claim, predicate`)
could not reach the P4 declaration (`claim, canonical_predicate, canonical_subject`) at all: the
swap alone does not produce a three-column list, and the append alone does not rename a column.
Refusing it would have been a spawn failure for every store that skipped a release, which is the
opposite of a migration. Pinned end to end by
`an_existing_cell_db_gains_the_entity_dimension_in_one_wake` (`store/factory.rs`).

**0.2.0 P3 added the third class, `tokenizer`** (issue #14, ruling Q6): an existing index whose
declaration does not name `meclaw_stem_v1` is dropped and rebuilt through it. Neither of the other
two classes can see this case — the column list is IDENTICAL, only the tokenizer differs — so
without it a running store would keep an unstemmed index while every query arrived folded, which
is worse than the state before the package. Same treatment, same reasoning, and it happens on the
first spawn after the upgrade: **no tool, no manual step, nothing to schedule**. One consequence
worth knowing before you go poking at a live `cell.db`: the tokenizer lives on the SQLite
CONNECTION, so `sqlite3 cell.db 'select * from facts_fts'` now fails with `no such tokenizer`.
The base tables are unaffected — every `select` against `facts`, `episodes` and friends reads
exactly as before, which is also why the scenario runner's `db_assertions` never noticed.

## Not in scope (and where it lives)

| Missing | Package |
|---|---|
| skills population, decay/freshness scoring | spec phase 5 |
| entity dedup, fuzzy index on `entities.canonical_name` (graph anchors match EXACTLY today) — the predicate half is done (0.2.0 P1 + P2) | GitHub #23 |
| statement identity: W1 to W6 have shipped (session guard + currency marker, `canonical_claim` as the identity, judged closures, extractor `replaces`, judged cardinality, judged claim aliases). What is left of the track is the ONE paid run of ruling Q5 and the ranking demotion of closed statements (ruling Q6 option C) | GitHub #13 — the rulings of this track and its brief are recorded there |
| irregular morphology in the keyword leg (`ging`/`gehen`); a window in tier 0 — regular inflection is done (0.2.0 P3) | `docs/defer-register.md` § P15 |
| store constraints (UNIQUE), native BLOB write path, ANN index for `similar` | roadmap defers |
| **embedding generation rotation is manual.** Changing `MEMORY_EMBED_MODEL` does not move `emb_models.active`, because that table is seeded once at `cell.db` creation — the semantic leg keeps filtering for the retired generation and silently sees no new fact. The operator recipe is three store ops (see "Rotating the embedding model" above); automating it would mean a template that rewrites its own seed table | — |
| `embeddings.binarization_version` is written and read nowhere. A filter on it would be additive; until then a generation is identified by `model_id` alone | — |
