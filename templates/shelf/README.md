# `shelf@1.0.2`

A place to put rows, as one `store` cell. It exists because of a gap the library
had rather than the substrate: eighteen shipped templates carry a `store` inside
them -- `access/store`, `argus/charter`, `collector/window`, `submit/store` and
fourteen more -- and **not one of them was instantiable on its own**. An
`add_nodes` entry requires a `name` and a `template`, there is no form for a bare
cell, and so a running colony could not be given somewhere to keep what it
produced by any manifest, by any caller, through any door
([#482](https://github.com/mmeyerlein/meclaw/issues/482)).

That is the whole template: one cell, one table, and no opinion about what goes
on it.

## What it delivers

- **A table a declaration can ask for.** `{"name": "headlines", "template": "shelf",
  "override_params": {"schema": {…}}}` and one edge, in the same mutation, and a
  colony that could only pass messages around can keep something.
- **A shape that is a parameter.** The shipped table is a generic four-column one
  and it is meant to be replaced: `params.schema` is a bootstrap declaration, so
  an override at instantiation is baked into the `cell.db` by DDL before the cell
  ever wakes. That is what makes one template two shelves.
- **One answer per message, however many operations it carried.** A single
  `tool_call` gets a single `tool_result` and nothing else changes; two or more
  get one turn each in call order, `hop.operation: "bundle"`, a
  `hop.bundle_errors` count and the per-operation metadata in a `results[]` slot.
- **SQL errors that are answers, not crashes.** `unknown_table`,
  `unknown_column`, `type_mismatch`, `constraint_violation` and `sql_error` come
  back as regular `tool_result` turns carrying the code, so a caller reads it and
  repairs its own call.
- **Argument keys that are published.** Since `shelf@1.0.1` the catalogue row
  carries an `ARGS` line: which key each operation reads (`insert` takes `row`,
  not `values`), that an unknown key is REFUSED rather than ignored, and two
  worked calls. A measured design-lane run wrote `values` and lost forty legs to
  `insert: unknown argument "values" (known: operation, table, row)` -- the
  store's own refusal printing the list the corpus did not carry
  ([#513](https://github.com/mmeyerlein/meclaw/issues/513)).
- **A return lane whose form is published.** Since `shelf@1.0.2` the catalogue
  row carries a `RETURN` line: that `hop.operation` is the only field on an
  answer that names the leg it belongs to, that a return edge is guarded
  POSITIVELY on it, and what a guard on `context` costs
  ([#521](https://github.com/mmeyerlein/meclaw/issues/521)).

## The cell

| cell | type | what it holds |
|---|---|---|
| the template root | `store` | its own `cell.db`, and in it exactly the tables `params.schema` declares. No policy, no schedule, no trim. |

Single-cell template (one cell of one cell type, the smallest `config.json` that
starts it, and a README that explains its declarations): instantiate it under a
name that says what it keeps, and the instance IS the cell.

## Ports and wiring

One edge in from whoever writes, and one back out **per answer** -- guarded:

```json
[
  { "from": "./dedupe",    "to": "./headlines",
    "condition": "has(hop.route) && hop.route == 'store'" },
  { "from": "./headlines", "to": "./dedupe",
    "condition": "has(hop.operation) && hop.operation == 'select'" },
  { "from": "./headlines", "to": "./dedupe", "default": true }
]
```

| field | meaning |
|---|---|
| `hop.operation` | the operation that ran, `'bundle'` for a multi-call message, and the literal `'error'` when nothing parsable arrived. **Every** answer carries it, the error surface included |
| `hop.rows_affected` | rows touched; on a bundle the raw sum across the operations, which mixes reads and writes -- the load-bearing number per operation is in `results[]` |
| `hop.bundle_errors` | on every bundle answer, `0` included; never on a single-operation answer |
| `hop.error_code` | a rejection of the WHOLE message: `invalid_input`, `query_timeout`, `write_denied`. A per-operation SQL error rides in the turn instead |

A store answers every message it is sent, so a write lane that nobody drains
fills the dead-letter queue with replies. Either draw the return edge or point
the reply at something that swallows it.

## The return lane

The three edges above are the whole rule, and the second one is the one that is
easy to get wrong. A **two-phase exchange** over a store -- ask the shelf what it
already holds, then insert what is new, which is the only dedupe form a store
that ships no constraints allows -- sends TWO messages and gets TWO answers back
down the same lane. Something has to tell them apart.

The two header compartments do not both offer themselves for that:

- **`hop` is single-hop.** It is REPLACED on every emission, not merged. A
  `hop.route` the caller stamped on the way out is gone by the time the answer
  comes back; what the answer carries is what the STORE stamped.
- **`context` is persistent.** It survives the round trip -- and is therefore
  *identical on both legs*, because a compartment does not clear itself.

So the field that names the leg is **`hop.operation`**, which the store stamps
unconditionally, the error surface included
([#331](https://github.com/mmeyerlein/meclaw/issues/331)). Guard the return edge
POSITIVELY on the leg you want back:

```json
{ "from": "./headlines", "to": "./dedupe",
  "condition": "has(hop.operation) && hop.operation == 'select'" }
```

and give every other answer somewhere to go -- an ordinary `"default": true`
edge, which takes what the guarded edges declined instead of letting it
dead-letter as `no_route`. A write answers `'insert'` when the message carried
one call and `'bundle'` as soon as it carried more, so naming the read leg is the
stable half of the pair.

A guard on `context` looks like it does the same job and does not. Measured on
the verification run of [#513](https://github.com/mmeyerlein/meclaw/issues/513),
a composer drew `context.phase == 'select'` on the return edge and set the phase
on the way out; `phase` was still `'select'` on the answer to the **insert**, the
script re-entered its insert branch, and the pair looped until the TTL -- 30
rounds a tick, 1 200 rows for 40 distinct links, one `ttl_expired` dead letter to
close it ([#521](https://github.com/mmeyerlein/meclaw/issues/521)). The same
class, from the other end, is the one the cookbook note
`workshop/cookbook/reply-to-fallback-loops.md` is named after; the shipped
`builder-librarian/retrieve` recognises its own second phase this way
(`hop.operation in ("search", "bundle")`), positively and in the script rather
than on the edge, which is the same rule applied one cell over.

## Knobs

There is no env knob here, and that is a decision rather than an omission: a
table name is a per-instance fact, and a colony-wide default for it would make
two shelves in one colony impossible. The shape is `override_params` on the node
-- a **flat** params object, because a single-cell template has no inner cell to
address, and the path-keyed form (`{"": …}`) is refused with `schema`:

```json
{"name": "headlines", "template": "shelf@1.0.2",
 "override_params": {"schema": {"headlines": {"id": "text", "at": "text",
                                              "title": "text", "link": "text"}}}}
```

| param | default | effect |
|---|---|---|
| `schema` | `{"rows": {"id": "text", "at": "text", "text": "text", "meta": "json"}}` | the tables this shelf owns, as `{table: {column: type}}`, types `text` / `int` / `json`. Bootstrap-only: immutable at runtime, because a live change would desynchronise the declaration from the tables |
| `query_timeout_ms` | `5000` | the operation timeout on a query, and the only runaway guard there is -- a `select` has no implicit `limit` and no cap |

An existing `cell.db` is grown INTO the declaration at the next spawn: a declared
column the table lacks is added, and an undeclared column that is already there
is never touched, retyped or removed.

## What does NOT live here

- **Constraints.** No PRIMARY KEY, no UNIQUE, no NOT NULL, no default, no index
  -- a core defer of the cell type ([`docs/cell-types.md`](../../docs/cell-types.md)
  § `store`). Two identical rows are two rows, so **dedupe belongs to whoever
  writes**, as a stable id it computes before it inserts.
- **Full text, canonical identity, a write boundary.** `params.fts`,
  `params.canonical` and `params.write_surface` are real declarations of the cell
  type and this skeleton does not carry them, which also means an
  `override_params` cannot reach them: an override names a param the template
  already declares. A store that needs one of the three is a template of its own,
  and writing it in the same manifest is what `add_templates` is for.
- **A trim.** Nothing here deletes anything. A shelf that grows forever grows
  forever; pair it with a `clock` and something that sweeps.
- **A meaning.** What belongs on the shelf is the writer's word.

Pinned by
[`crates/meclaw-cells/tests/gh513_the_scriptlet_publishes_its_script_contract.rs`](../../crates/meclaw-cells/tests/gh513_the_scriptlet_publishes_its_script_contract.rs),
which runs the two worked calls of the `ARGS` line against a real shelf and the
key it warns against through the same cell, and by
[`crates/meclaw-cells/tests/gh482_the_composer_can_name_the_cells_it_needs.rs`](../../crates/meclaw-cells/tests/gh482_the_composer_can_name_the_cells_it_needs.rs),
which reads the shipped form and then builds a feed out of it -- a clock, a
fetcher, a scriptlet and this shelf, instantiated by one manifest into a colony
that had none of them. The `RETURN` line and § *The return lane* are pinned by
[`crates/meclaw-cells/tests/gh521_a_store_answer_names_its_leg.rs`](../../crates/meclaw-cells/tests/gh521_a_store_answer_names_its_leg.rs),
which lifts the published condition out of the row and evaluates it, through the
colony's own CEL, against the answers a real shelf gives to a real `select` and a
real `insert` -- and evaluates the `context` form the same way, so the sentence
about the loop is a measurement.
