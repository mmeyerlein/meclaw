# `builder-librarian@2.2.0`

Lexical retrieval over the builder's own knowledge base, as a hive of existing cell types
-- no new cell type, no Rust. Three cells: `retrieve` (a `code` cell, the query/brief state
machine), `store` (a `store` cell holding the corpus and its FTS5 index) and, since
`2.1.0`, `catalogue` (a `code` cell that reconciles the catalogue against the colony's own
template registry).

It sits in front of an authoring cell so a model **retrieves** the pattern it needs instead
of carrying the whole corpus in its prompt. One per builder hive.

**Lexical by design** (plan D-L1). The local GPU serves a chat model and no embedding model,
and paying a metered provider to embed a corpus the builder can already find by name would
be the wrong trade. BM25 over section-cut chunks answers the question a builder actually
asks -- "show me the pattern for X" -- and answers it for nothing.

## The cells

| cell | type | what it holds |
|---|---|---|
| `retrieve` | `code` | phase A builds a BM25 query from the request; phase B renders the rows into a briefing. No state, no writes. |
| `store` | `store` | the `docs` table (`id`, `source`, `section`, `kind`, `text`) with FTS5 over `text` + `section`. Seeded at birth; the only runtime write is the reconciliation below. |
| `catalogue` | `code` | the one look outward: asks `/colony/templates` what the library holds and adds a row for every name the corpus does not carry. Reached on `in_ingest`, answers on `catalogue`. |

`retrieve` recognises phase B **positively** (`hop.operation == 'search'` -- or `'bundle'`,
which is what a catalogue lookup's two legs come back as -- and no
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

### Three kinds that answer what the briefing demands exactness about

The corpus grew three `kind`s because the builder's briefing stated three rules whose data
was in no chunk of it -- the same shape the `CONTRACT --` line was the first instance of.

* **`kind: "level"`** ([#486](https://github.com/mmeyerlein/meclaw/issues/486)) -- one row
  per rendered level set, out of `examples/organism/grow-*.json`, which are byte-pinned
  against the fast lane's renderer. The rows are **condensed**, not the files: edges are
  grouped by the guard they share, so seventeen member edges are two lines. That is not
  tidiness. `grow-member.json` is 2 934 characters, `retrieve` hands the model
  `text[:1200]`, and a fixed set published as its first two edges is worse than one not
  published at all -- it looks complete.
* **`kind: "store"`** ([#483](https://github.com/mmeyerlein/meclaw/issues/483)) -- one row
  per **table**, which is the unit a `seed_rows` entry addresses and the unit small enough
  to arrive whole. `memory-hive`'s one store declares 1 437 characters of schema; as a
  single row it would arrive as two thirds of itself. Each row cites the `config.json` it
  was derived from. The catalogue rows carry a short `STORES --` line beside `CONTRACT --`
  saying which stores a template has and where, so a `catalogue_lookup` points the way even
  though it does not carry the columns. The `PARAMS --` line below it is the same move for
  the params surface.
* **`kind: "params"`** ([#505](https://github.com/mmeyerlein/meclaw/issues/505)) -- the
  cells and params of a template whose list did not fit its catalogue line, one row per
  page of it. `override_params` must name a param the addressed cell declares, the mutation
  door refuses the first key that is not one and prints the set it does accept -- and no
  shipped template's param names were in the corpus. Measured: a design lane that wanted a
  periodic feed named the `clock` template correctly and then guessed `interval_ms`,
  guessed `cron`, and wrote a `schedules` entry the timer refused -- three rounds spent on
  a set of two names. Most templates need no row of this kind at all, because the whole
  list stands on the catalogue row; the ones that do are paginated at a whole number of
  CELLS, which is the unit `override_params` addresses.

All three are reached with `librarian_search`, which is unfiltered. `catalogue_lookup` is
not: see below.

### `catalogue_lookup` is the catalogue

`templates/builder/lib` has always stamped `hop.lib_kind = "template"` for a
`catalogue_lookup`, and its contract published that as *"the same corpus filtered to the
rows that open with `CONTRACT --`"*. **Nothing read the key** -- both tools ran the identical
unfiltered BM25 query -- so a lookup for a template by its exact name could come back with a
different template first: measured, `catalogue_lookup "member"` answered `org` at position
one and `member` at position four, as a continuation chunk. Since `2.0.8` phase A puts the
kind into the search op's `where`, which the store applies to the base table beside the
`MATCH`. One key, no second query. An unmarked search stays unfiltered -- that is how the
`level` and `store` rows are reachable at all.

#### And it can say **no, by name**

BM25 always returns its best hits, so a filtered catalogue still could not say *that name
does not exist*. Since `2.0.9` it can. Asked for a template that is not there it answered with four plausible
neighbours and nothing marking them as neighbours -- and a caller that cannot tell *not
found* from *found something adjacent* has no reason to stop asking. Measured on one wish
([#482](https://github.com/mmeyerlein/meclaw/issues/482)): seven rounds, the same four
questions rephrased each time, a prompt growing from 5 557 to 50 220 tokens, and no manifest
at the end of it.

So a `catalogue_lookup` asks **two** ops in one message -- the BM25 search, and the
catalogue's own **name appeal**: a `select` of `section` over every `kind: "template"` row,
which for a catalogue row *is* the template name. One bundle, one round trip; the store runs
a bundle's ops in order over its one connection and answers with one `tool_result` turn per
leg, keyed on that leg's id. `librarian_search` asks neither the filter nor the appeal, and
its briefing is unchanged.

Phase B then compares the request against that list on **word boundaries**, longest name
first, striking each match out of the request as it goes -- so `member` is not found inside
`remember` or `membership`, and `builder-librarian` is never also reported as `builder`. The
verdict goes in **front** of the rows, because a reader that meets four plausible neighbours
first has already begun reading them as the answer. Where the request holds a name the
catalogue has:

```text
the request names templates that exist: <names>. The rows below are the catalogue's
answer for them.
```

and, where the request holds no name the catalogue has:

```text
no template by that name. The catalogue holds <n> names: <every name, comma separated>.
Anything below is a nearest neighbour by text and not a name an `add_nodes` can use.
```

The second form is the whole point, and it is why zero search hits get it too: `(no matching
patterns)` on its own is precisely the answer that does not say what there *is* instead. The
verdict is **measured** and not merely spoken -- `hop.catalogue_names` counts the names the
catalogue answered with and `hop.catalogue_named` is `1` or `0` -- and both keys are absent
from an unmarked search, which never asks.

A lost appeal costs the briefing nothing. Retrieval is an enhancement and the appeal is an
enhancement on top of it: an empty or refused name leg leaves the rows exactly as they were
and simply says nothing about names. A refused *search* leg is the other case and keeps the
[#308](https://github.com/mmeyerlein/meclaw/issues/308) discipline -- a bundle reports a
leg's failure in the body's `results[]` and never in the header, so that code is lifted onto
the hop and the terminal branch marks the briefing `degraded` instead of rendering an error
text as zero rows.

## Knobs

**Since 2.2.0 every knob of this hive is a param of `./retrieve`, not an environment
variable** ([#138](https://github.com/mmeyerlein/meclaw/issues/138), ruling R-0904-6). Each
one exists three times with one value: under `params`, as a `contract.settings` entry, and
as the literal the shipped script falls back to; `crates/meclaw-cells/tests/gh138_long_tail_params.rs`
pins the three against each other. Retune one librarian without touching another:

```json
{"add_nodes": [{"name": "builder-librarian", "template": "builder-librarian",
  "override_params": {"retrieve": {"topk": 8, "row_chars": 2000}}}]}
```

| param of `./retrieve` | default | meaning |
|---|---|---|
| `topk` | `5` | how many rows the store returns and the briefing renders |
| `row_chars` | `1200` | the window one rendered row may spend -- for every kind but `template` and `level` |
| `catalogue_chars` | `4000` | the window for a CATALOGUE row (`kind: "template"`), which is the corpus chunker's own cap, so such a row travels whole |
| `level_chars` | `1600` | the window for a LEVEL row (`kind: "level"`), which carries a composition level's complete transit edge set |

A numeric knob may arrive as a string, and one blanked or set to `null` means "not
configured" and falls back to the shipped default -- an operator who empties a line in a
config did not mean to stop the cell.

**A cut row says it was cut** ([#511](https://github.com/mmeyerlein/meclaw/issues/511)).
The retriever used to hand the model `text[:1200]` of every hit: silent, mid-word, and the
same number for every kind. Measured over the shipped corpus that cut 330 of 603 rows --
and 80 of the 87 CATALOGUE rows, which is the class where it did the damage. A catalogue row
is `CONTRACT --`, `STORES --`, `PARAMS --` and then the whole `template.json`, and
`description.examples` is the LAST key of every one of them, so the cut landed every time on
the only place a template's worked instantiation is published. One measured wish looked
`clock` up three times, got the identical cut row back each time, correctly read the corpus
as exhausted, and spent its repair budget guessing the two param names the row already
carried.

So the window is per kind, and a cut is never silent. A catalogue row travels whole, bounded
by the discipline that wrote it rather than cut a second time on the way out. Anything else
keeps the recall window and, when it does not fit, is cut on a word boundary and carries the
marker

```
… [TRUNCATED: <n> of <m> characters were not sent. This row is a FRAGMENT and not a whole
statement. A template's own catalogue row is never cut -- ask catalogue_lookup for it by name.]
```

which is the `-cont` discipline of [#344](https://github.com/mmeyerlein/meclaw/issues/344) on
the retrieval side: a fragment a reader knows is a fragment is a different object from one it
does not.

## The seed corpus is a BUILD PRODUCT -- do not hand-edit it

`store/seed/docs.jsonl` is **generated**. It is chunked out of the spec (`docs/*.en.md`, see
above), the cookbook, the corpus briefs, the template catalogue (`templates/*/template.json`), the
pinned error codes, the six rendered **level edge sets** (`examples/organism/grow-*.json`)
and every shipped store's **table schemas** by:

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
([#234](https://github.com/mmeyerlein/meclaw/issues/234) removed it). The unskippable run is the
maintainers' release-export gate (its rule R11), in the private tree that
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
(the four spec documents under their public names, plus the catalogue of published
templates), and the export aborts if a single row names a source the
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

## The corpus is in the language it is searched in

The four spec documents are chunked from their **English** edition
(`docs/X.en.md`), and that is a decision rather than a default
([#497](https://github.com/mmeyerlein/meclaw/issues/497)). The design lane briefs in
English, the composer answers in English, and every query it has produced in a measured
run is English. The corpus was German: the generator globbed `docs/*.md` and the plain
name is the German original, so **294 of 590 chunks** -- over half of it, and the half
that carries the specification -- were prose an English query could not match.

That is not a ranking nuisance, it is a miss. FTS5 tokenizes lexically, so an English
query against German prose matches on the tokens the two languages happen to share --
`params.schema`, `script_inline`, `config.json` -- and on nothing that carries the
meaning. Measured: four `librarian_search` calls in one run that each needed
`docs/cell-types.md` and none of which got it, while the chunk that would have answered
sat in the corpus with its identifiers matching and its sentences invisible.

**Instead of, not beside.** Carrying both editions would double those chunks and dilute
the ranking of every query that matches either. The English edition is held structurally
equal to the German one by the export gate, so the corpus is a translation of the spec
rather than a second spec.

A private row cites `docs/cell-types.en.md`, because in this tree that is the file whose
bytes it holds. A **public** row cites `docs/cell-types.md`, because the exported tree
carries the English edition under the plain name and ships no German original
([#441](https://github.com/mmeyerlein/meclaw/issues/441)) -- the same `DOCS_MAP` mapping
the export itself makes. Same bytes, two names, and each one names a file its own reader
can open.

## The catalogue of a RUNNING colony, and how it stops lying

`store/seed/docs.jsonl` is loaded once, at `OpenStatus::Created`, and never again. So a
running librarian answers about the library of the moment it was born. A template
registered since -- by an `add_templates`
([#440](https://github.com/mmeyerlein/meclaw/issues/440)), by a directory dropped into
`templates/` plus a rescan, by anything at all -- is resolvable at the mutation door and
**does not exist** for the composer. Measured
([#496](https://github.com/mmeyerlein/meclaw/issues/496)): a design-lane run spent seven
rounds and its whole budget looking for a `clock` that had been in the library for an
hour, and ended with no manifest.

Two different drifts wear the same face, and only one of them is what this section is
about:

* **The committed corpus vs. the tree.** That is a build-product gate and it already
  stands: `--check` regenerates and byte-compares, run from the `corpus` station of
  `scripts/gate.sh` (every strand, the integration pass and the release; the export
  reads that station's verdict out of the release receipt as R11) and from
  `crates/meclaw-cells/tests/librarian_seed_corpus.rs`. It is deliberately **not** in
  `.github/workflows/ci.yml`
  ([#234](https://github.com/mmeyerlein/meclaw/issues/234)): the generator lives under
  `workshop/`, which does not travel, so the step failed on a missing file and said
  nothing about drift.
* **A running colony's index vs. its own registry.** No gate can reach that one: the
  corpus was correct when the colony booted and the library moved afterwards. This is
  what `in_ingest` closes.

### One nudge, one look outward

`catalogue` asks `/colony/templates` what the library holds, asks the store which names
the corpus already carries, and inserts one row per name that is missing. Four phases,
each recognised positively -- the discipline `retrieve` already carries, for the same
reason ([#308](https://github.com/mmeyerlein/meclaw/issues/308)).

```
.  --(in_ingest)--> ./catalogue --(cat_read)--> /colony/templates
/colony/templates --(reply_to)--> ./catalogue --(cat_store)--> ./store   # which names do you hold?
./store --(context.librarian_origin == 'catalogue')--> ./catalogue --(cat_store)--> ./store   # insert the rest
./store --> ./catalogue --(catalogue)--> .
```

**The colony is asked first, and that ordering is forced.** A colony reply arrives on a
fresh trace with an **empty** context compartment, and `/colony/templates` echoes no tag
of its own -- unlike `/colony/graph` and `/colony/registry`, which carry `query.tag`
back. Nothing of ours survives that round trip, so the registry answer has to be the
first thing the cell holds rather than the second. It then rides
`context.cat_seen` across the store round trips, as `name@version` space separated: a
template name matches `^[a-z][a-z0-9-]{1,63}$` and a version carries no space, so the
join is unambiguous, and the registry's `filesystem_path` is deliberately **not**
carried -- one absolute path per template would grow the compartment with the deployment,
and the row does not cite it.

**Both exit edges clear the same set.** The hive is sealed, so no interior marker may
leave it ([#494](https://github.com/mmeyerlein/meclaw/issues/494)) — and the rule is about
the UNION, not about the path: `cat_seen` cannot be set on the `brief` lane and
`orig_request` cannot be set on the `catalogue` lane, but an exit edge that only clears the
keys its own branch writes is one refactor away from letting the other branch's out. So
both `-> .` edges name all five.

**It only ever adds.** `docs` has no key, so a second reconciliation that re-inserted
everything would double every row and the name appeal would answer the same name twice.
The diff against the corpus is what makes it idempotent, and it is also what keeps a
shipped template's rich generated row: a seeded row is never replaced, never deleted and
never touched.

**And the row it writes says what it cannot know.** `/colony/templates` answers the
registry's projection -- `template_id`, `name`, `version`, `filesystem_path`, `author` --
and neither the `requires` block nor the store schemas are in it. So the row opens with
`CONTRACT --`, because every catalogue row does and the retriever hands the model
`text[:1200]`, and what it says there is that the declaration was not read and must be.
A row that had written *"requires no ctx and no env key"* out of that silence would be
exactly the confidently-wrong answer this corpus exists against, and BM25 would rank it
as high as a true one. What the row DOES buy is the half that was missing: the name is in
the catalogue, so `catalogue_lookup "foo"` answers *the request names templates that
exist: foo* instead of handing back four neighbours.

### Who drives it

The lane is the mechanism; the nudge is topology. Two wirings, and neither is invented
here:

* **After a submission that registered a class.** Since `meclaw-os@1.7.0` the shell draws
  `./operator -> ./builder` on a `sub_receipt` that carries no `error_code` and whose
  `hop.registers_class` is `true`, re-stamped `set_hop: {route: "'in_ingest'"}`; from
  there `builder` forwards the nudge to `./librarian` and lets the report back out
  on `catalogue`. That closes the loop at the one moment where both facts are known --
  the manifest registered a class, and the colony committed it. `submit` is the cell that
  knows the first (its gate already reads the diff for `add_templates`, which is how it
  derives the `code.author` question) and it publishes that verdict as
  `hop.registers_class` on every receipt its renderer produces; the key travels on the
  flight row, because the colony answers on a fresh trace that says nothing about what
  the declarations were. Until `meclaw-os@1.6.1` the edge read `./submit -> ./builder` on
  the lane `receipt`, because the submitter was a hive of the shell; since GH #556 it is an
  occupant of the front door, whose own `sub_receipt` lane carries the submitter's receipt
  out for exactly the two facts the shell reads here.
* **Once after a deployment, and after any rescan.** A message to the librarian's hive
  path with `hop.route = "in_ingest"`. That is the whole of the boot reconciliation, and
  it is a message rather than a timer on purpose: a retrieval hive that polls the
  registry on a schedule buys the same answer at a cost that never stops, and a cell has
  no boot hook that a timer would not itself be.

The form this section named before `meclaw-os@1.6.0`, `./submit/gate -> ./builder/librarian`,
is **undrawable** -- by a manifest, by an operator, by a hand-written `config.json` alike.
Both endpoints are interior nodes of sealed hives, and `hive_port_boundary` refuses each
of them on its own account. The reachable edge runs between the two HIVE PATHS, which is
why each end had to grow a lane first.

The nudge is **not** wired inside this template, and cannot be: the librarian does not
know who its operator is, and a lane that fired itself would be a hive with an opinion.
Only a level that holds the submitter's hive and `builder` as SIBLINGS can draw it, for the
same reason only that level can draw the broker pair -- `./operator -> ./access` on `ask`
since GH #556, `./submit -> ./access` before it.

## Lanes

`params.ports` is empty (GH #228): the address is `./librarian` itself.

| lane | direction | what travels |
|---|---|---|
| `in_request` | in | a build request to find patterns for |
| `brief` | out | the request plus the patterns found for it, for the authoring cell behind this librarian |
| `in_ingest` | in | a nudge to reconcile the catalogue against the colony's registry. The body is not read -- the message **is** the nudge |
| `catalogue` | out | the reconciliation's report: `hop.catalogue_known`, `hop.catalogue_ingested`, and the names that were added |

Retrieval is an enhancement, so a failed lookup comes back on `brief` too -- degraded and
marked, never as silence and never as a hang. The same holds one lane over: a failed
reconciliation comes back on `catalogue` naming its `error_code`
(`catalogue_unavailable` for a reply the cell cannot read, `store_refused` for a write the
store would not take), with the corpus exactly as it was.
