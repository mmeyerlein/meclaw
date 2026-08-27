# `builder-librarian@2.0.7`

Lexical retrieval over the builder's own knowledge base, as a hive of existing cell types
-- no new cell type, no Rust. Two cells: `retrieve` (a `code` cell, the query/brief state
machine) and `store` (a `store` cell holding the corpus and its FTS5 index).

It sits in front of an authoring cell so a model **retrieves** the pattern it needs instead
of carrying the whole corpus in its prompt. One per builder hive.

**Lexical by design** (plan D-L1). The local GPU serves a chat model and no embedding model,
and paying a metered provider to embed a corpus the builder can already find by name would
be the wrong trade. BM25 over section-cut chunks answers the question a builder actually
asks -- "show me the pattern for X" -- and answers it for nothing.

## The cells

| cell | type | what it holds |
|---|---|---|
| `retrieve` | `code` | phase A builds a BM25 query from the request; phase B renders the rows into a briefing. No state. |
| `store` | `store` | the `docs` table (`id`, `source`, `section`, `kind`, `text`) with FTS5 over `text` + `section`. Seeded, never written at runtime. |

`retrieve` recognises phase B **positively** (`hop.operation == 'search'`, and no
`error_code` on the same hop), and anything else arriving with a hop already set is
terminal: the librarian degrades to a briefing that says so, and the build carries on
without patterns. Retrieval is an enhancement, and it must never be able to hang a build --
the naive "phase B is anything that is not a fresh request" shape reads a store error reply
as a new question and spins until the TTL kills it -- the reply-to-fallback loop.

The `error_code` half of that test is not decoration
([#308](https://github.com/mmeyerlein/meclaw/issues/308)): a `store` cell writes
`operation` on **every** reply -- the error surface included since
[#331](https://github.com/mmeyerlein/meclaw/issues/331) -- and `error_code` on top of it,
so a failed search arrives shaped exactly like a successful one and `error_code` is the
only field that tells them apart. Recognising it on `operation` alone parsed the error
text as a result set, found zero rows, and rendered `(no matching patterns)` -- a failure
wearing the face of an honest answer. It now falls to the terminal branch and comes back
marked.

The way home is a **context** marker, not a hop field. `./retrieve -> ./store` stamps
`librarian_origin`, and `./store -> ./retrieve` is conditioned on it -- the same shape
`canvy` and `coder-pipeline` use. Conditioning on `hop.operation` instead looked equivalent
and was not: back then the store's `invalid_input` and `query_timeout` replies carried no
`operation` at all, missed the edge, and dead-lettered as `no_route`, which is the one
outcome a lane promising "never silence" cannot have. Since
[#331](https://github.com/mmeyerlein/meclaw/issues/331) those replies DO carry `operation`
-- and the context marker stays anyway, because it is what makes the way home independent
of whichever header shape the store answers with.

## Knobs

| env var | default | meaning |
|---|---|---|
| `BUILDER_LIBRARIAN_TOPK` | `5` | how many rows the store returns and the briefing renders |

## The seed corpus is a BUILD PRODUCT -- do not hand-edit it

`store/seed/docs.jsonl` is **generated**. It is chunked out of the spec (`docs/`), the
cookbook, the corpus briefs, the template catalogue (`templates/*/template.json`) and the
pinned error codes by:

```
python3 workshop/tools/build_librarian_seed.py            # write
python3 workshop/tools/build_librarian_seed.py --check     # verify, exit 1 on drift
```

A hand-edit to `docs.jsonl` survives exactly until the next regeneration and then vanishes
without a word. Edit the **source** instead, and regenerate.

**A long section spans several rows.** The chunker caps a row at 4000 characters; a section
over that line is carried across continuation rows that keep its `source`, `section` and
`kind` and take the base row's id with a `-cont<N>` suffix (`d0037`, `d0037-cont1`). The cap
used to cut instead, and the tail left the corpus with nothing said about it -- a paragraph
of the spec did exactly that
([#344](https://github.com/mmeyerlein/meclaw/issues/344)). Cuts land on a line break or a
space, never mid-word, and the generator prints a counted headroom warning for every
**unsplit** row within 5 % of the cap -- the section that still fits, barely, which is the
one the next paragraph turns into a split. An already-split row is excluded: it lies near
the cap because the splitter seams as late as it can, and listing it would bury the real
near-misses.

Because the three descriptive columns no longer identify a row, the briefing heading
carries the id: `### <source> -- <section> (<kind>) [<id>]`, with `, continued` after the
id of a continuation. Without it two pieces of one section arrive under the same heading,
and a fragment that starts mid-argument reads as a whole statement.

Being generated is not the same as being current -- the product is committed, so the tree
can hold a corpus describing a tree that no longer exists. It did, for 289 lines
([#205](https://github.com/mmeyerlein/meclaw/issues/205)). That is worse than an absent
corpus: the librarian's whole job is answering "what templates exist and what do they do",
and BM25 ranks a stale answer exactly as high as a true one. So `--check` regenerates and
byte-compares.

**Where that check actually runs.** The generator lives under `workshop/`, which does not
travel with the exported tree, so the gate cannot stand in the public CI -- it did, and it
failed on a missing file, which says nothing about whether the corpus drifted
([#234](https://github.com/mmeyerlein/meclaw/issues/234) removed it). The unskippable run is
`plans/export-fixtures/make_export.py`, as R11 of the release gate, in the private tree that
has the sources, before any export is built. `crates/meclaw-cells/tests/librarian_seed_corpus.rs`
invokes the same `--check` from `cargo test` wherever the generator is present, and skips
where it is not -- which in a clone of the public tree is always, so a green run of that test
asserts nothing about the corpus by itself. The corpus is current or the export is red.

**A published tree carries a DIFFERENT corpus, and that is deliberate**
([#441](https://github.com/mmeyerlein/meclaw/issues/441)). Every other file in this template
is a statement about a topology, and a topology carries no secret. `store/seed/docs.jsonl`
is the exception: it is a *copy of its sources*, and a third of the development corpus is
chunked out of `workshop/`, which never travels -- the catalogue there also describes
templates that are not published. So the export does not copy the blob, it **replaces** it:
the same chunker runs with `--public` over a source list the export subset actually carries
(the three spec documents in their English edition, under their public names, plus the
catalogue of published templates), and the export aborts if a single row names a source the
exported tree does not hold. What that costs the published librarian is the cookbook, the
corpus briefs and the pinned error codes; what it answers is still "what does the spec say"
and "what templates exist". Regenerating this file in a clone of the public tree is
therefore not possible and not needed -- the generator is not there, and the corpus beside
it is the one the export built.

Two failures the generator distinguishes, because they call for different fixes
([#207](https://github.com/mmeyerlein/meclaw/issues/207)):

- **drift** -- the sources moved and the committed corpus did not. Regenerate and commit.
- **a source that is present and does not parse** -- a broken reference, not a missing row.
  The generator exits naming the file and the parse error, because the template whose
  `template.json` is malformed is precisely the one somebody needs told about, and it is the
  one that used to disappear from the catalogue in silence. Regenerating does not repair it;
  fix the file.

## Lanes

`params.ports` is empty (GH #228): the address is `./librarian` itself.

| lane | direction | what travels |
|---|---|---|
| `in_request` | in | a build request to find patterns for |
| `brief` | out | the request plus the patterns found for it, for the authoring cell behind this librarian |

Retrieval is an enhancement, so a failed lookup comes back on `brief` too -- degraded and
marked, never as silence and never as a hang.
