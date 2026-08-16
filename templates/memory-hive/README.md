# `memory-hive@1`

Agent memory as a hive of existing cell types — no new cell type, no Rust. Eleven cells:
`store` (all durable data), `writer`, `recall`, `extract-glue`, `extractor`, `dream-glue`,
`dreamer`, `judge`, `cron`, `embed`, `dialectic`.

What it delivers today (packages P2–P5 = spec phases 1–4, plus P15 = temporal truth):

- **Write path**: every turn becomes an append-only `episodes` row. LLM-free, immediate, the
  agent never waits.
- **Recall tier 0**: a deterministic, token-budgeted bundle (active beliefs → open foresight
  facts → recent episodes). No LLM, no embedding, fixed latency.
- **Recall tier 1**: four retrieval legs — keyword (`search`), semantic (`similar`), graph
  (`traverse`), temporal (as-of `select`) — fused by RRF in code. Still **LLM-free** and
  deterministic; every candidate says which leg found it.
- **Recall tier 2**: `dialectic` synthesises one answer over the tier-1 candidates with the
  source priority beliefs → facts → episodes and a **mandatory gap statement**.
- **As-of recall**: any tier can be evaluated at a past instant (`recall_as_of`) — "what was
  true in May" and "what did we believe in May" are parameters, not promises.
- **Supersession at read time (P15, narrowed by W2)**: which fact is in force is decided when the
  question is asked, on the version chain. Order alone decides only within ONE statement (a
  re-assertion of the same canonical claim); across two different values a span ends where an
  explicit closure says so, and `expired_at` is read rather than recomputed. A hit on a closed
  statement answers with the one that closed it and carries the predecessors as `history`. The
  invariant that buys: the same recall before and after a dream run returns the same candidates,
  byte for byte.
- **Time-range recall (P15)**: `recall_window_from`/`_to` turn the temporal leg from a point
  into an interval. Every version whose derived span intersects the window is a candidate of
  its own and carries the `span` it was valid for.
- **Extraction, two ingresses**: the batched `extractor` (gate at ~128 accumulated tokens or a
  2-minute-old item) and an **inline ingress** for front-line LLMs that emit their extraction
  in the answering turn. Both go through the same validator and the same
  `(episode_id, claim_hash)` dedup.
  * **The gate is RECOMMENDED, not mandatory, and it is tuned for freshness (GitHub #51).**
    Batching exists because one call per message is the worst cost constant in the field, and it
    buys no answer latency either way — extraction is off the answer path. What the gate's VALUES
    decide is how long a fact stays unqueryable after the message that carried it, and under the
    operating priority *quality first, time second, cost third* that is the number worth
    defending: the defaults (`MEMORY_BATCH_TOKENS` 128, `MEMORY_BATCH_MAX_AGE_MIN` 2) put a fact
    in the store about two minutes after its turn in the worst case. They are a starting point a
    colony may raise — a colony whose front model emits inline extractions uses the batch lane
    only as a gap filler and can afford a wider gate — but nothing in the hive requires the old
    cost-first values, and no lane reads them as a contract.
  * **One discipline, two ingresses (GitHub #53)**: the batched extractor's contract is
    rendered by `extract-glue`; the inline one lives in the front model's persona, outside this
    hive -- so the hive SHIPS it, in [`inline-contract.md`](inline-contract.md), the way it ships
    `predicate-core.json`. The block a persona pastes states the same five rules the batch
    prompt states: the assistant's own answer is not a fact, a question is not a fact, restating
    stored knowledge mints nothing, only new world state carried by the user's turn qualifies,
    and an empty facts block is a correct extraction. Measured without it: two history questions
    minted their own answers as facts on a fresh predicate spelling, each with a `valid_until`
    taken from the question's date range -- closed on arrival, so the as-of leg could not see
    them while keyword and semantic still could. Drift lock in both directions:
    `crates/meclaw-cells/tests/f9_inline_contract.rs`.
  * **One turn is extracted once (GitHub #52)**: an inline block takes the queue rows of the
    turns it named out of the queue, with a status of its own (`inline`). The batch lane then
    serves the purpose it exists for -- the turns of models that emit no block -- instead of
    buying a second opinion on a turn the front model already answered. The dedup cannot do
    this job: it compares claim bytes, and two models never phrase one claim identically, so a
    re-extraction lands on a NEW predicate spelling and the chain arithmetic runs on the wrong
    collective. An **empty** facts block covers its turn too: an empty list is the front
    model's verdict that nothing was memorable, not the absence of one. A block that names no
    episode, or that is not JSON at all, covers nothing -- a block that proves nothing about a
    turn leaves that turn to the batch.
- **Canonical predicates (0.2.0 P1)**: a predicate is a KEY, not prose — English, `snake_case`,
  the same key for the same relation whatever language the turn is in. Before it prompts, the
  batched lane reads the `(subject, predicate)` axes this memory already carries — one bounded,
  store-deduplicated read (`distinct`, `MEMORY_EXTRACT_VOCAB_ROWS`), because the hint asks which
  axes EXIST, not how often they were asserted — and hands them
  to the extractor together with the curated core vocabulary in
  [`predicate-core.json`](predicate-core.json) (29 entries, each marked `single` or `multi`).
  Reuse over minting is what makes the version chain fire at all. The counterpart rule is
  **entity fidelity**: only the predicate is canonicalised — subjects, objects, values and
  proper names are copied byte for byte, never translated and never spell-corrected, because a
  name the model "fixes" is a fact destroyed. Pinned by
  `crates/meclaw-cells/tests/extract_canonicalization.rs` (deterministic) and by the scenarios
  C1 / C2 (model, `--with llm`).
- **The extractor closes what it can SEE (statement identity W4, GitHub #13, ruling Q2 option
  C)**: next to the axis hint the lane builds a **replacement window** — the OPEN statements of
  the axes it touched most recently, each with its value, the instant it started and the id it
  carries. A fact may then come back with `replaces: <id>`, and that becomes exactly the closure
  the nightly judge writes: `expired_at`, `superseded_by`, `closure_source = extract:<batch_id>`,
  one `update` on `facts`. The point is the north star of this memory — resolve the conflict IN
  THE TURN — and the extractor is the only party present in it.
  * **The window is the guard rail, and it is mechanical.** A `replaces` naming anything the
    window did not contain is discarded and logged; the window is parked in `scratch` under the
    batch key and read back when the facts are written, so what is checked is what the model was
    provably shown (ruling Q2 rail 3). `expired_at is_null` in the `where` carries over from W3:
    an extractor closure never writes over a judged one. Budgets: `MEMORY_EXTRACT_WINDOW_AXES`
    over all axes, one page per axis, and an axis longer than that page is **skipped, not
    truncated** — a producer that sees six of seventy plans cannot tell it is a bucket.
  * **The window is PAGED, and that is why the skip rule still holds (GH #68).** The leg opens
    with a bounded recency page over the open facts (`MEMORY_EXTRACT_WINDOW_SCAN`) that picks the
    candidate axes, and then reads those axes WHOLE (`MEMORY_EXTRACT_WINDOW_ROWS`, axis-major).
    The skip rule is a count, so it can only be taken on a complete axis: a bucket seen through a
    cut-off scan looks like a short axis, and a replacement on it deletes an answer that is true.
    A page that comes back full has exactly one axis it cannot prove complete — its last — and
    that one is dropped rather than counted.
  * **The window shows the axes the TURNS are about (GH #67).** The paged read fetches a POOL of
    `2 × MEMORY_EXTRACT_WINDOW_AXES` axes by recency, and the prompt phase — the only phase that
    holds the turns — cuts it down to the offered budget by SUBJECT MATTER: the content words of
    the batch against the values and the relation of each axis, most overlap first, recency as
    the stable tie-break. Recency is a good guess about which axes matter and a bad one about
    which axis a given turn is ABOUT, and that difference is the whole of GH #67: an update whose
    sibling axis was not shown gets a private axis of its own. The window the prompt SHOWS is the
    one that gets parked, so guard rail 3 stays exactly as wide as the rendered block.
  * **A replacement points FORWARDS in time (GH #71).** The window says which statements may be
    closed and never said which of the two is newer, and the first live sample of three extractor
    closures contained one inversion: a statement ended by a fact three days its senior, which
    cost the run its only wrong answer. So the apply phase compares before it writes — the
    statement being closed must not have been asserted after the fact replacing it — and a
    refused closure is not a refused extraction: the fact is minted exactly as before, both
    statements stay open, and the pair is receipted in `scratch` under the batch key
    (`kind = extract-refusals`, with both values, both instants and the reason) so the night can
    still judge it. The comparison is this lane's own recency, `(valid_from, recorded_at, id)`,
    on the assertion still standing: two statements of ONE instant still replace one another,
    because the row being closed was read out of the store before this batch's clock.
  * **Prompt rules** (the P1 discipline one dimension over): replace only when the new value
    updates the SAME matter, never on an axis that ENUMERATES (a second child, one more language,
    another plan), never the statement the fact merely repeats, and when in doubt leave
    `replaces` out — a missed replacement costs one outdated line, a wrong one removes a value
    that was true. A turn that PLANS, WANTS or HOPES something about a shown statement lands on
    that same subject and predicate with `fact_kind: foresight` and an empty `replaces`: an
    intention stands next to the fact it is about, it does not end it.
  * **Receipt and revert.** The reasons of a batch are parked in `scratch` under the batch key —
    the extraction lane's equivalent of the run receipt the night folds its closures into — which
    is exactly what `closure_source` names, so a closed fact reads back to the turn that replaced
    it and one batch reverts with one `where` (`update facts set closure_source = '' where
    closure_source = 'extract:<batch_id>'`, plus the next re-derive). The extractor's reason is
    structural rather than prose: the replacing value, the value replaced and the episode both
    came from.
  * **The night validates it** (guard rail 3, second half): the canonicalisation round's axis
    pages also carry statements a RECENT extract closure ended (`MEMORY_CANON_EXTRACT_LOOKBACK_DAYS`),
    and the judge may contradict one. A contradiction clears the attribution and the re-derive of
    that same night withdraws the columns — the W3 revert, one producer over. Only the
    extractor's closures are reviewable there: a round that could revoke a JUDGEMENT would make
    "only close, never delete" revocable by the party that wrote it. **Direction needs no
    judgement (GH #71)**: a closure whose `expired_at` lies before the statement it ended was
    written the wrong way round, and the scan of the night takes it back itself — same revert,
    same receipt key in the run's books (`reopenings`), and the closure is not put in front of
    the judge at all, because the round does not buy an opinion about a row it just cleared.
  Pinned by `crates/meclaw-cells/tests/w4_extract_replaces.rs` (free) and the scenarios C14
  (free, the real window with the extractor's answer handed in, both gate halves) / C15
  (`--with llm`, a real extractor on one replacement and one enumeration).
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
    so the fix is the extractor prompt (`A PREDICATE NAMES THE SUBJECT MATTER, NEVER THE SPEECH
    ACT`, with the named shapes it refuses), the window visibility above, and the guidance in
    [`predicate-core.json`](predicate-core.json). No speech act is seeded and none ever will be;
    the subject-matter examples there are deliberately seeded with NO cardinality, because a
    seeded verdict outranks the night's own and an over-cap axis of a `multi` relation is
    answered for good (GH #66).
  Pinned by `crates/meclaw-cells/tests/f6_subject_matter_axis.rs` (free, 16) and the scenario C22
  (free, end to end on a real colony).
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

## Ports

The hive is an **island**: instantiate `add_nodes` and the port `add_edges` in ONE mutation,
otherwise the subtree stays inactive and `cron` never spawns (the island-activation pattern:
an island without a crossing edge is never woken, so the mutation that creates the subtree has
to be the mutation that connects it).

**The ports below are enforced, not advisory** (GH #133). `config.json` declares them:

```json
"params": { "ports": ["writer", "recall", "extract-glue"], "graph": { … } }
```

A mutation whose `add_edges` reaches any other cell of this hive from outside — `./memory/store`,
`./memory/extractor`, `./memory/dreamer`, … — is rejected with `error_code:
"hive_port_boundary"`, pre-destructively, in either direction. Inside the hive nothing changes:
the internal graph wires whatever it likes, at any depth. Wire the three endpoints below, or the
hive path itself.

**And the store is writable only from inside** (GH #132): `store/config.json` declares
`"write_surface": "internal"`, so a write op (`insert`/`update`/`delete`/`create_table`/
`set_alias`/`reject_pair`/`canonicalize`) whose sender sits outside this hive is refused with
`error_code: "write_denied"` before it reaches the database. **Reads stay free from anywhere** —
which is what keeps a debug probe straight into `./memory/store` a legitimate move. The five
writers (`writer`, `recall`, `extract-glue`, `dream-glue`, `embed`) all live in the hive, so
nothing about the shipped topology changes; what changes is that the memory can no longer be
edited past its own lanes.

| Port | Direction | Endpoint | The edge must carry |
|---|---|---|---|
| turn-write | in | `./memory/writer` | optionally `set_context: {happened_at: "hop.happened_at"}` for historical ingest; `session_id`/`turn_id` are ingress context keys and travel by themselves |
| recall-request | in | `./memory/recall` | `set_context: {recall_query: "hop.recall_query", memory_tier: "hop.memory_tier", recall_as_of: "hop.recall_as_of", recall_window_from: "hop.recall_window_from", recall_window_to: "hop.recall_window_to"}` — the caller must send all five keys on EVERY hop, empty string = unset (see the trap below) |
| inline-extraction | in | `./memory/extract-glue` | `set_context: {store_origin: "'inline'", mem_phase: "'inline'"}` -- and the front model's persona carries the block from [`inline-contract.md`](inline-contract.md). The caller's `session_id` must be in the context (it is, in the `talky` composite): a block that names no episode is BOUND to the newest `user` turn of that session, and one that arrives without a session cannot be bound and is rejected |
| extraction-flush | in | `./memory/extract-glue` | `set_context: {mem_phase: "'flush'"}` — drain the queue now, whatever the batch gate would say. Add `flush_reclaim: "'1'"` to also recover batches whose chain died; that sweep is **lease-gated** (GH #72) and never takes back a claim younger than `MEMORY_BATCH_CLAIM_LEASE_MIN` |
| recall-response | out | `./memory/recall` → your consumer | condition `hop.route == 'bundle'` |
| inline-reject | out | `./memory/extract-glue` → your drain | condition `hop.route == 'reject'` — **not optional once the inline ingress is wired, and since GH #147 that is enforced rather than asked for.** The hive declares the pairing in `params.required_drains`, so a mutation that wires the ingress alone comes back `required_drain_missing` and changes nothing; put both edges in the same mutation and it commits. A rejected block on an undrained egress is an unrouted dead end, so nobody ever learns the memory was not written — a colony that ran the inline lane for weeks with only `recall-reject` drained is where that lesson comes from |
| recall-reject | out | `./memory/recall` → your drain | condition `hop.route == 'reject'` — a HALF window (exactly one of `recall_window_from`/`_to` non-empty) is a caller bug and leaves here at request entry, before the leg fan. **Drain it**: unrouted it is a dead end, and the caller waits for a bundle that never comes. Declared in `params.required_drains` too, so wiring `recall` without it is refused the same way |

`recall_query`, `memory_tier`, `recall_as_of`, `recall_window_from`, `recall_window_to`,
`happened_at`, `store_origin` are **not** ingress context keys (that list is closed: `turn_id`,
`session_id`, `user_id`, `chat_id`, `locale`). They are promoted from `hop` by the port edge —
the `rag_question` pattern.

**A second consumer is where the hive's own bookkeeping starts to travel** (GH #152).
`mem_phase` and `recall_id` belong to this hive and are *persistent* context: once a consumer
has asked once, they ride along in everything that consumer emits afterwards — including an
errand it hands to a **second** agent, whose collector then asks this hive with a phase it never
set. The request entry recognises that case by the hop the port edge stamps (`phase: "recall"`)
and starts a fresh chain regardless of what the context carried, so a caller does not have to
know about keys it does not own. **Nothing is required of the caller here** — but if you write
an edge into this port by hand and want to be explicit, `delete_context: ["mem_phase",
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
`hop.route == 'reject'` out of the `recall-reject` port. Both filled **on a tier-0 request** is
the one case the lane cannot answer: the bundle comes back through `recall-response` as usual
and says so, with `hop.window_ignored = "1"` and a `window_ignored` block in the body (0.2.0 P7).

**Who derives the window is CURRENT DESIGN, not a settled boundary.** The rule above — the hive
does not guess, the consumer derives the window — is deliberate and it is also the reason the
window machinery has, so far, no caller: a consumer that pins both keys to the empty string runs
every question as a point recall, including the explicit time-range ones the window was built
for. Moving the derivation to the hive side is tracked in
[#55](https://github.com/mmeyerlein/meclaw/issues/55); the keys, the reject rule and the tier-0
notice are unaffected either way.

## Variables

Everything carries a `:-default` **except** `OPENROUTER_API_KEY` and the three per-turn `MODEL_*`
slots — those four must come from `.env` (see the negative fixture `memory_hive_env_missing`). A
model name has no defensible default: picking one silently is how a memory lane ends up on a weak
model without anybody deciding it (see the recommendation below).

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
| `OPENROUTER_API_KEY` | — (required) | api_key of `extractor`, `dreamer`, `judge` and `dialectic` |
| `MODEL_EXTRACTOR` | — (required) | extraction model |
| `MODEL_DREAMER` | — (required) | consolidation model (change narrative) |
| `MODEL_JUDGE` | `anthropic/claude-opus-5` | identity judgement of the nightly canonicalisation round — **the strongest model of the hive belongs here** |
| `MEMORY_LLM_BASE_URL` | `https://openrouter.ai/api/v1` | OpenAI-compatible endpoint of every llm cell |
| `MEMORY_BATCH_TOKENS` | `128` | accumulated token estimate that opens the extraction gate. A recommended default, not a mandate (GH #51) |
| `MEMORY_BATCH_MAX_AGE_MIN` | `2` | age of the oldest queued item that opens the gate anyway — the freshness ceiling of the batch lane (GH #51) |
| `MEMORY_TIER0_TOKENS` | `1200` | token budget of the tier-0 bundle |
| `MEMORY_TIER0_MAX_EPISODES` | `12` | item cap of the bundle's episode leg |
| `MEMORY_TIER0_MAX_BELIEFS` | `20` | item cap of the bundle's belief leg, and the `limit` of the belief select behind it |
| `MEMORY_TIER0_MAX_FORESIGHT` | `10` | item cap of the bundle's foresight leg (facts that are about a future the memory has been told about) |
| `MEMORY_TIER0_EPISODE_CHARS` | `400` | episode truncation inside the bundle (truncate, never delete) |
| `MEMORY_DREAM_CRON` | `0 0 3 * * *` | 6-field Quartz schedule of the nightly run |
| `MEMORY_EMBED_ENDPOINT` | `https://openrouter.ai/api/v1/embeddings` | OpenAI-compatible embeddings endpoint |
| `MEMORY_EMBED_MODEL` | `qwen/qwen3-embedding-8b` | must match the `model_id` in `seed/emb_models.jsonl` — the seed is NOT variable-substituted, so the two are coupled by hand |
| `MEMORY_EMBED_DIM` | `1024` | requested `dimensions`; must match `emb_models.dim` (1024 bits → 128 packed bytes) |
| `MEMORY_EMBED_API_KEY` | *(empty → falls back to `OPENROUTER_API_KEY`)* | bearer for the embedder |
| `MODEL_DIALECTIC` | — (required) | tier-2 synthesis model |
| `MEMORY_REASONING_EXTRACT` | `minimal` | `provider_extra.reasoning.effort` of the `extractor` cell — extraction is a shape-filling job, not a thinking one |
| `MEMORY_REASONING_DREAM` | `medium` | `provider_extra.reasoning.effort` of the `dreamer` cell (the nightly change narrative) |
| `MEMORY_REASONING_DIALECTIC` | `medium` | `provider_extra.reasoning.effort` of the `dialectic` cell (the tier-2 answer with its gap statement) |
| `MEMORY_REASONING_JUDGE` | `high` | `provider_extra.reasoning.effort` of the `judge` cell — the identity verdicts are written into the store and every later read consumes them, so this is the one lane where thinking is worth paying for |
| `MEMORY_CANON_JUDGE` | `1` | `0` switches the nightly canonicalisation round's ASK off (no scan, no candidate feed, no model call). A judgement handed to the lane from outside is still applied — applying is arithmetic, asking is what costs. The free scenario classes run with `0`, except the two that measure the QUESTION (`C19`/`C20`): those ask for real with the endpoint pointed at a dead port |
| `MEMORY_CANON_MAX_PREDICATES` | `60` | relation keys put to the judge per run, busiest first |
| `MEMORY_CANON_MAX_PAIRS` | `12` | entity candidate pairs put to the judge per run, best score first |
| `MEMORY_BATCH_MAX_ITEMS` | `64` | hard item cap of one claimed batch |
| `MEMORY_BATCH_CLAIM_LEASE_MIN` | `5` | how long a claimed batch is HELD before a recovery sweep may hand its rows back (GH #72). Must exceed one full extraction cycle — the extractor's `message_timeout` is 180 s, so a lease below ~4 minutes reclaims live batches and pays for them twice |
| `MEMORY_TIER1_LEG_LIMIT` | `20` | per-leg candidate cap of the tier-1 fan |
| `MEMORY_TIER1_AXIS_LIMIT` | `200` | page bound of the AXIS reads — the hydration's chain select (`t1-hyd-axis`) **and** the window leg's generous pre-filter share it. Too small truncates a chain, and a candidate whose chain was cut is delivered **without** `history` rather than with a guessed one |
| `MEMORY_EXTRACT_WINDOW_AXES` | `8` | axes whose OPEN statements travel in the extraction prompt as the replacement window (statement identity W4). An axis with more open statements than one page is skipped rather than truncated. The PAGE fetches twice this many by recency; which of them the prompt SHOWS is decided by subject matter against the batch's own turns (GH #67), with recency as the stable tie-break |
| `MEMORY_EXTRACT_VOCAB_ROWS` | `512` | cap of the axis-hint read (GH #68). Deduplicated by the store, so it counts AXES, not facts; ordering is subject-major, so the cap cuts whole subjects off the tail |
| `MEMORY_EXTRACT_WINDOW_SCAN` | `512` | cap of the recency page that picks the window's candidate axes (GH #68). Counts open fact rows, newest assertion first |
| `MEMORY_EXTRACT_WINDOW_ROWS` | `256` | cap of the window's own page, which reads those axes whole (GH #68). Axis-major, so a full page has exactly one unproven axis: the last, which is dropped |
| `MEMORY_CANON_EXTRACT_LOOKBACK_DAYS` | `7` | how far back the nightly round reviews the closures the EXTRACTOR wrote (W4, ruling Q2 guard rail 3). Derived from `delta_to`, so a re-run reads the same window back; a closure older than this has been in front of a round already |
| `MEMORY_CANON_MAX_AXES` | `8` | axes with more than one open statement put to the judge per run (the currency question of W3, and since W6 the rewording question on the same list). The whole budget, pages included: an axis too big for one page takes one of these slots rather than a slot of its own (GH #66) |
| `MEMORY_CANON_MAX_PAGED_AXES` | `2` | how many of those slots a night may spend on axes it can only show one PAGE of (GH #66). Such an axis is never truncated blind: it is offered only after its relation was seeded or judged FUNCTIONAL, the page is the most recent statements, and what it leaves behind is stated in the run receipt |
| `MEMORY_CANON_MAX_CARD` | `8` | relations whose CARDINALITY is put to the judge per run (statement identity W5). Only relations the seed list does not own and the store has not judged yet are offered, so the budget always reaches something the memory has not decided |
| `MEMORY_CANON_CLOSED_ROWS` | `256` | page bound of the identity questions' own read of the CLOSED rows (GH #73), most recently ended first. Bounded on ROWS and never on a clock: `expired_at` says when a statement stopped being TRUE, not when the closure was written, so a cutoff derived from `delta_to` would drop exactly the closure written last night about a change dated last spring |
| `MEMORY_CANON_MAX_CLOSED_AXES` | `12` | how many spellings out of that page reach the two identity questions (GH #73). ON TOP of `MEMORY_CANON_MAX_PREDICATES`, not out of it: a spelling a closure just proved belongs to another one is the best-founded question a night has, and the open vocabulary's budget was never sized for it |
| `MEMORY_DREAM_AXIS_LIMIT` | `5000` | page bound of the dream lane's axis select. A **full** page means the chain may be incomplete, so the materialisation SKIPS the derivation for that page instead of guessing supersession from half a chain |
| `MEMORY_TIER1_TOPK` | `20` | how many fused candidates survive the RRF cut into the tier-1 bundle |
| `MEMORY_TIER1_TOKENS` | `2000` | token budget of the tier-1 bundle; candidates are taken in fused order until the next one does not fit |
| `MEMORY_TIER1_ITEM_CHARS` | `400` | per-item truncation inside the tier-1 bundle (a claim, an episode's content, a supersession marker). Truncate, never drop |
| `MEMORY_BUNDLE_EPISODE_BUDGET` | `6` | the episode share of the fusion cut (P15 O-7): episodes take at most this many of the `TOPK` slots and keep that many against any fact wall. Whichever side cannot fill its share lets the other backfill, so the bundle is never shorter than a plain prefix would be |
| `MEMORY_TIER1_GRAPH_DEPTH` | `2` | `max_depth` of the graph leg's `traverse` (store cap: 5) |
| `MEMORY_TIER1_GRAPH_NODES` | `200` | `max_nodes` of the graph leg's `traverse` — the fan-out kill switch, so one hub entity cannot turn a recall into a walk of the whole graph |
| `MEMORY_RRF_K` | `60` | RRF constant |
| `MEMORY_RRF_W_KEYWORD` | `1.0` | fusion weight of the keyword (FTS) leg |
| `MEMORY_RRF_W_SEMANTIC` | `1.0` | fusion weight of the semantic (embedding) leg |
| `MEMORY_RRF_W_GRAPH` | `1.0` | fusion weight of the graph (traverse) leg |
| `MEMORY_QUERY_SAFE_CHARS` | `200` | a query at or below this length reaches the legs untouched. Above it the hygiene guard runs (GH #88) |
| `MEMORY_QUERY_MAX_CHARS` | `250` | hard clamp of the hygiene guard: whatever survived the question / tail-sentence steps is cut to its last this many characters, so the cost of a recall stops depending on how much context the caller pasted |
| `MEMORY_QUERY_TOKENS` | `24` | how many tokens of the sanitised query reach the FTS matcher, taken from the TAIL — after the hygiene guard the tail is the question (GH #88) |
| `MEMORY_RRF_W_TEMPORAL` | `1.0` | weight of the temporal leg **in window mode only** (P15 O-4) |
| `MEMORY_RRF_W_TEMPORAL_POINT` | `0.0` | weight of the temporal leg in **point** mode. Measured, not chosen: 50 identical LongMemEval extractions, paired — `0.0` gives R@1 **84.0 vs 74.0** and R@5 **98.0 vs 96.0**, flips **11:1** (sign test p=0.0063), and across 100 runs (2×50) the leg never carried a hit **alone**. Set it to `1.0` to restore the pre-O-4 fusion |
| `OPENROUTER_HTTP_REFERER` / `OPENROUTER_X_TITLE` | `https://meclaw.ai` / `MeClaw` | OpenRouter app attribution headers |

**Model recommendation (P15 R10): put the memory lane on a strong model.** The three `MODEL_*`
slots are where extraction quality is decided, and every extraction defect found so far hangs on
a weak local model: a small village name silently "corrected" into a spelling that does not
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
strongest available model is the right answer rather than an indulgence. Cost stays in
cents — the lane runs once per batch and once per night, not per turn.

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
| `query_timeout_ms` | `30000` | **read lane.** A-timeout of ONE query-embedding attempt. Deliberately more generous than the write lane: the query vector is what recall's semantic leg waits on, and losing it costs a whole leg of the four-leg fan |
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

## The read path (tiers 0, 1, 2)

| Tier | Cost | What it does |
|---|---|---|
| 0 | no LLM, no embedding, ~1 round trip per leg | three fixed legs (session episodes, active beliefs, open foresight) → one token-budgeted bundle |
| 1 | no LLM, one embedding call | four retrieval legs → RRF fusion → hydration → ranked candidates |
| 2 | one LLM call on top of tier 1 | `dialectic` synthesises an answer with a mandatory gap statement |

**The four legs (tier 1).** Each leg returns a *ranked id list*; content is fetched once, at the
end, in a single hydration round. That is what keeps the fusion cheap and the scratch payloads
small.

| Leg | Store op | Ranked by | Yields |
|---|---|---|---|
| keyword | `search episodes` + `search facts` (bm25) | `rank`, merged facts-before-episodes on a tie | facts + episodes |
| semantic | `similar embeddings` | hamming `distance` | facts (via `owner_id`) |
| graph | `select entities` → `traverse entity_edges` | depth, then accumulated weight | episodes (via the edge's `episode_id`) |
| temporal | point mode: as-of `select facts` · window mode: a generous interval pre-filter, exact cut in code | point mode: `recorded_at` desc · window mode: `valid_from` desc, `recorded_at` desc, `id` asc | facts |

**RRF.** `score(d) = Σ_legs w_leg / (K + rank_leg(d))`, `K = 60`, weights 1.0 — except the
temporal leg, whose weight depends on the mode (O-4, below). A candidate found by two legs adds
both terms — that is the entire point. Ties break by best rank, then leg priority (keyword,
semantic, graph, temporal), then — on an **exact** score tie — the temporal leg's own rank, and
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
| point (no window) | the **current** statement of the hit's axis, i.e. the one its closures lead to, carrying `history: [{id, claim, from, until}]`. A CLOSED hit is therefore never a candidate of its own — it is a field on the statement that closed it. Two hits landing on one statement collapse into one candidate, and that candidate carries the **union** of both hits' `legs` (P15 O-4b) |
| window | **every** version stays its own candidate and carries `span: {from, until}` — in a time-range question the versions themselves are the answer, so nothing collapses |
| multivalued axis | untouched: no `history`, every value stands (see below) |

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

**The fallback is a candidate without history, never a lost candidate.** If the axis select was
truncated (`MEMORY_TIER1_AXIS_LIMIT`) or carried no matching `(subject, predicate)` pair, the hit
is delivered as it came, with `history: []`. Losing a candidate would be the worse failure.

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
* **Nothing is dropped and nothing is re-ranked.** Superseded values stay in the bundle (history
  questions need them, and hiding them would hide the store's own uncertainty), the key is absent
  on an open statement, and a line without it renders byte for byte as before. Demoting closed
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
approximate with a remote one. Registered in `docs/roadmap.md`, not worked around here.

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

**The gap statement is enforced, not hoped for.** `recall` parses the dialectic answer; if the
`gap` field is missing or the answer is not JSON, the answer is still delivered but carries
`gap_missing: true` in the body and `hop.gap_missing='1'`. A provider error downgrades to the
tier-1 candidates with `hop.tier_downgraded='1'` — never silence.

**Three gates, three guard rows.** The semantic join, the main fan-in and the hydration fan-in
all use the P2 collector protocol (`insert → select → guarded update → select → fire`), each on
its OWN guard row (`qvec`, `kw-ep`, `fused`) so they cannot steal each other's `fired` flag.

Cost: a tier-1 recall is ~31 store round trips (P15 added the axis select). Deliberate — the
join-less store answers one result set per message, and determinism beats hop-thrift here.

## Temporal questions — what is answerable, and what is not

Three shapes are answerable, and they differ only in what the caller puts in the hop:

| Question | Hop | What comes back |
|---|---|---|
| "what does Alex use?" (now) | `recall_as_of: ""`, both window keys `""` | the current fact of each axis, predecessors attached as `history` |
| "what did he use in May?" (an instant) | `recall_as_of: "2026-05-01T00:00:00Z"`, both window keys `""` | the chain trimmed to that instant first — the answer is what was true THEN, not what is true now with a date attached |
| "what did he use between March and now?" (a range) | `recall_window_from` + `recall_window_to` both set | every version whose derived span intersects the window, newest validity first, each with its own `span` |

**"And what was it before?" is a field, not a second query.** The predecessors travel with the
current candidate (`history`), so the change is answerable from one recall — which is the whole
reason supersession moved to read time.

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
| `claim_hash = sha256(episode_id\|subject\|predicate\|claim)[:16]`, filtered before insert | inline + batched extraction |
| guarded `update … where {status:'pending'}` + `rows_affected > 0` | batch claim (exactly one winner) |
| guarded `update … where {status:'claimed', claimed_at:{lt: now - lease}}` | the recovery sweep (#72). Idempotency is not the same as free: the `(episode_id, claim_hash)` dedup makes a re-extraction write nothing new, so the OLD sweep took back every claimed row and called that safe. It was safe for the data and expensive for everything else — a batch reclaimed while its extractor call is still in flight is extracted, and paid for, twice, which is where 5 859 batched items for 3 839 turns came from. The claim now carries the instant it was taken, so the sweep can tell a dead chain from a slow one |
| guarded `update … set {status:'inline'} where {episode_id in …, status:'pending'}` | inline coverage (#52) -- a turn already extracted inline is never offered to a batch; a row a batch already claimed is left to the batch that owns it |
| `select episodes where {session_id, sender:'user'} order by recorded_at desc limit 1` | the inline BIND -- the turn a block that names none is speaking for. `sender` is what makes it deterministic: the answer's own episode is written by the same per-turn lane, concurrently, so "newest episode" would be a race and "newest user turn" is not |
| guarded `update recall_scratch set fired=1 where {request_id, fired:0}` | recall fires exactly once per request |
| window guard on `(delta_from, delta_to)` and on `run_id` | dream lane, stage 2 |
| `set_alias` upserts on the alias, `reject_pair` upserts on the ordered pair, `canonicalize` reports only the rows that MOVED | canonicalisation round — re-judging a pair writes no second row, and a second run over unchanged data reports 0 |
| every dream write derives its timestamp from `delta_to`, belief ids from `sha256(holder\|statement)` | dream lane, stage 3 — replay is byte-identical |
| `max_concurrency: 1` on `extract-glue`, `dream-glue`, `recall` | serialises the read-modify-write handlers (a `code` cell is a stateless dispatcher and would otherwise run them in parallel) |

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
printf 'OPENROUTER_API_KEY=dummy\nMODEL_EXTRACTOR=dummy/extract\nMODEL_DREAMER=dummy/dream\nMODEL_DIALECTIC=dummy/dialectic\n' > $RUN/.env
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
   {"from":"./anchor","to":"./memory/writer","condition":"has(hop.route) && hop.route == \"write\""},
   {"from":"./anchor","to":"./memory/recall","condition":"has(hop.route) && hop.route == \"recall\"",
    "modifier":{"set_context":{"recall_query":"hop.recall_query","memory_tier":"hop.memory_tier",
     "recall_as_of":"hop.recall_as_of","recall_window_from":"hop.recall_window_from",
     "recall_window_to":"hop.recall_window_to"}}},
   {"from":"./anchor","to":"./memory/extract-glue","condition":"has(hop.route) && hop.route == \"inline\"",
    "modifier":{"set_context":{"store_origin":"'"'"'inline'"'"'","mem_phase":"'"'"'inline'"'"'"}}},
   {"from":"./memory/recall","to":"./capture","condition":"has(hop.route) && hop.route == \"bundle\""},
   {"from":"./memory/recall","to":"./capture","condition":"has(hop.route) && hop.route == \"reject\""},
   {"from":"./memory/extract-glue","to":"./capture","condition":"has(hop.route) && hop.route == \"reject\""}]}}'
curl -s http://127.0.0.1:7792/colony/registry     # 10 hive cells active; cron Awake
```

`store`, `extractor`, `dreamer` and `dialectic` show `active=true` + `NotYetSpawned` — that is
the correct hot/cold PASS form for stateful cells, they wake on first delivery. Only the
long-running `cron` must be `Awake`.

**G4** — two negative fixtures in the same workshop tree pin the rejection paths:
`memory_hive_env_missing` (`env_var_missing`; the four no-default variables) and
`memory_hive_unknown_port` (`edge_schema`, guards against wiring a port that does not
exist — `./memory/decay`). Both are one-line variations of the positive root above.

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
MEM=("episodes","facts","entities","entity_edges","beliefs","embeddings","emb_models",
     "consolidation_log","skills")
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

# dream lane: same window twice -> identical memory state
POST '{"target":"/anchor","body":{"messages":[{"origin":"user","type":"text","text":"{\"probe\":\"dream\",\"run_id\":\"<uuid>\",\"to\":\"2026-08-07T20:00:00.000000Z\"}"}]}}'
POST '{"target":"/anchor","body":{"messages":[{"origin":"user","type":"text","text":"{\"probe\":\"verdict\",\"run_id\":\"<uuid>\",\"to\":\"2026-08-07T20:00:00.000000Z\",\"verdicts\":{\"supersede\":[{\"old_fact_id\":\"<fid>\"}],\"beliefs\":[{\"holder\":\"self\",\"statement\":\"...\",\"confidence\":75}]}}"}]}}'
DUMP > /tmp/dump1.json   # replay both, then DUMP > /tmp/dump2.json; diff must be empty
```

`DUMP` covers **memory state**. The lane bookkeeping tables (`scratch`, `recall_scratch`,
`pending_extraction`) are append-mostly by design — a replayed run stages its payload again —
so they are explicitly *not* part of the replay claim. The claim is: **a replay changes no
memory.**

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
| statement identity: W1 to W6 have shipped (session guard + currency marker, `canonical_claim` as the identity, judged closures, extractor `replaces`, judged cardinality, judged claim aliases). What is left of the track is the ONE paid run of ruling Q5 and the ranking demotion of closed statements (ruling Q6 option C) | GitHub #13, rulings `plans/statement-identity/rulings.md`, brief `plans/0.2.0-memory-quality/statement-identity-brief.md` |
| irregular morphology in the keyword leg (`ging`/`gehen`); a window in tier 0 — regular inflection is done (0.2.0 P3) | `docs/roadmap.md` § P15 |
| store constraints (UNIQUE), native BLOB write path, ANN index for `similar` | roadmap defers |
| **embedding generation rotation is manual.** Changing `MEMORY_EMBED_MODEL` does not move `emb_models.active`, because that table is seeded once at `cell.db` creation — the semantic leg keeps filtering for the retired generation and silently sees no new fact. The operator recipe is three store ops (see "Rotating the embedding model" above); automating it would mean a template that rewrites its own seed table | — |
| `embeddings.binarization_version` is written and read nowhere. A filter on it would be additive; until then a generation is identified by `model_id` alone | — |
