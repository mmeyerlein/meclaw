# `graph-space@1.1.1`

The graph of one knowledge space ([#945](https://github.com/mmeyerlein/meclaw/issues/945), [#951](https://github.com/mmeyerlein/meclaw/issues/951)): the nodes every source of the space holds -- a file space's files, an object hive's objects -- and the edges between them, resolved once and kept current, so that a question across files is answered from one store and wakes no file. The hive is sealed (`params.ports: []`). Cells: `index` (code: announcement, pull, write, resolve), `query` (code: the questions), `store` (store, `write_surface: internal`). No model, no embedding request, no network.

## Lanes

| Lane | Direction | Body | Meaning |
|---|---|---|---|
| `source_changed` | in | `{source, version, path, fmt, parser, mark, nodes, links, tomb, audience_set?}` | a node source (`fh-<12 hex>` a file space, `ob-<12 hex>` an object hive) moved a head; `nodes`/`links` are the counts it holds for that version, `audience_set` the round an object's rows belong to |
| `pull` | out | `{op, file: '<source>@<version>', source, version, args}`, hop `op`, `op_id` `gs:<part>:<source>:<version>`, `source`, no `caller` | `outline` (limit = `nodes`), `links` (limit = `links`) and, for a file space only, `near` (`k`); `hop.source` is what the member routes the pull by |
| `in_pulled` | in | the source's `answer` to a pull, `op_id` mirrored | parked in `pulls` until the set is complete (three parts for `fh-`, two for `ob-`) |
| `in_graph` | in | `{op, args}`, hop `op`, `op_id`, `caller?`; the round in `context.audience_now`, else `context.audience_set` | a question; exactly one `answer` |
| `answer` | out | `{ok, op, op_id, items, next?, broken, ...}` | `broken` counts the broken edges the question touched |

## The contract with a node source

A source announces every head move and answers `outline`, `links` and -- a file space -- `near` for a version on its read lane. An object hive answers no `near`; its outline carries the object itself as the node with the anchor `''` (address: the source), and the round its rows were learned in travels as `audience_set` and stays on the source (`sources.audience_set`; '' for a file space, whose nodes are public within the space). The index pulls a constant number of times per announcement whatever the size of the source; a large answer travels as one blob. The set is written only if its version is still the announced one, so a stale pull never overwrites a newer version; a newer announcement that arrives after a set was found complete lands after it. A `tomb` drops the source's nodes, names and outgoing edges in one bundle, turns every resolved extracted edge into it `broken` and deletes the inferred ones; a removal carries no version (`{source, path, tomb: true}`) and none is asked of it. Two more bundles decide again the edges the removal may have made unambiguous.

Write first, resolve second: phase 1 replaces the source's rows and reads back what phase 2 needs -- including the resolved edges on a name the source provides; phase 2 updates every edge it decides differently with the state it read in the `where` (compare-and-set). Two sources that name each other and are indexed at once end up resolved, in any order of their phases, and the store does not depend on the order in which sources arrive: a later source with the same suffix makes a resolved edge `unresolved` (ambiguous, no address -- not `broken`), and removing it again resolves the edge.

## Resolution (one pure function per kind)

| Kind | Rule |
|---|---|
| `import` (python) | `a.b` → the source whose module is `a.b` (path without `.py`, `/` → `.`, `__init__` = package), else a unique dotted suffix (two candidates: `unresolved`); `a.b:c` → node `def:c`/`class:c` of `a.b`, else module `a.b.c`, else `unresolved`; `.x:y` relative to the importing package |
| `call` (python) | `f` → top-level node `f` of the same source, else through the source's `from … import f` (an import edge's `alias` -- `import … as x` -- binds `x`); `m:f` or `m.f` → `def:f` of module `m` |
| `use` (rust) | `crate::a::b::C` → source `src/a/b.rs` or `src/a/b/mod.rs` of the same crate, node `C`; `::*` → the source; another crate → `external` |
| `link` (markdown) | a path (relative to the linking file) → the source; `#frag` → `sec:<slug>` (the file space's heading slug, URL-decoded first), `broken` when the file is there and the anchor is not; no such file → `unresolved` |
| direct address | `fh-<12 hex>[#anchor]` or `ob-<12 hex>[#anchor]` (an object's `doc`, `related`, or any target of that form) → that node, or the source when no anchor is named and it lives; else `unresolved`, `broken` once lost |
| an object's other edges | `owner` and every target that is no address → `external` |
| `url` | `external` |
| `near` neighbour | an edge of class `inferred` with its `score`, never `extracted` |

A markdown `link` names a path: when the file it was resolved to moves (same source, new path), the link is `unresolved`, not `broken` and not resolved through the old address. An `inferred` edge into an anchor that is gone is deleted. `state` is `resolved`, `unresolved`, `broken` (with `since`) or `external`; `broken` means lost: the edge pointed at a target once and that target is gone -- a member that was never found stays `unresolved`. A broken edge keeps the address it pointed at and the time it broke, so `callers` of a renamed function still counts it. A source counts for resolution once it is indexed.

## Questions (`in_graph`)

| Op | Args | Items |
|---|---|---|
| `resolve` | `name` (module, `<module>:<name>` or a bare name) | `{addr, match}` |
| `callers` | `addr` | `{addr}` of every resolved incoming `call` |
| `dependents` | `addr`, `depth` (1–3) | `{addr, depth}`, incoming of any kind, transitive |
| `deps` | `addr` | `{addr, kind}`, outgoing and resolved |
| `path` | `from`, `to`, `max` (1–6) | the addresses of the shortest directed path (`found`, `hops`) |
| `similar` | `addr` | `{addr, score}` of the source's `inferred` edges |
| `broken` | — | `{from, kind, target_name, to, since}` |
| `stats` | — | counts of sources, nodes, edges by `state` and `class` |

Structural questions follow `extracted` edges only. A source address (`fh-…`, `ob-…`) walks source to source, a node address (`fh-…#anchor`) node to node.

Every answer sees the round of the question (`context.audience_now`, else `context.audience_set`). An object hive's source is visible when its `audience_set` covers that round -- nobody in the question is missing from it -- and no `ob-` source is visible to a question without a round; a file space's sources always are. What is hidden falls out of every answer: its nodes, the edges from and to it, the `resolve` hits, the traversal paths through it, and the counters (`broken`, `stats`). A question about a hidden address is answered exactly as one about an unknown address. Every list takes `limit` (default 20, at most 50) and `cursor`; a non-empty `next` is the cursor of the following page. Refusals: `bad_request`, `bad_address`, `unknown_op`, `store_error`.

## Knobs

| Cell | Param | Default | Meaning |
|---|---|---|---|
| `index` | `near_k` | 5 | neighbours asked of `near` |
| `index` | `nodes_cap`, `links_cap` | 5000 | the most nodes / edges one pull asks for |
| `index`, `query` | `scan_max` | 20000 | the most rows one read takes |
| `query` | `traverse_nodes` | 2000 | `max_nodes` of one traversal |
