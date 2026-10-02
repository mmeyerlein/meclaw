# `objects@1.0.0`

The objects of one member ([#951](https://github.com/mmeyerlein/meclaw/issues/951)): a thing its conversations keep naming becomes a row -- a type, its aliases, six slots and references -- learned in a round and visible only to the rounds that round covers. The hive is sealed (`params.ports: []`). Cells: `gate` (code, the one writer), `brief` (code, renders), `push` (code, events out), `source` (code, node source for the graph space), `tools` and `schemas` (code, a model's tools), `store` (store, `write_surface: internal`). No model, no network, no clock.

## Lanes

| Lane | Direction | Body | Meaning |
|---|---|---|---|
| `thing_seen` | in | `{items: [{name, kind?, note?}], turn_id, episode_turn_id?, session_id}`, round in `context.audience_set` | a sighting; without a round it is dropped and nothing is written |
| `in_tool` / `tool_result` | in / out | one tool call / one turn under its id | `object_find`, `object_brief`, `object_set`, `object_confirm` |
| `in_schemas` / `tool_schemas` | in / out | `{tools}` / `{schemas, unknown, sidecar}` | the menu |
| `in_read` / `answer` | in / out | a graph space's `pull` (`outline`, `links`) / `{ok, file, version, nodes \| links, next}` | the node source |
| `source_changed` | out | `{source, version, path: '', fmt: 'object', parser: 'objects', mark: '', nodes, links, tomb, audience_set}` | once per new version of an active row; `tomb` for a retired one |
| `facts` / `in_facts` | out / in | `{subject, limit: 3, messages: []}` (hop `audience_now`, `subject`, `recall_caller: 'objects'`; the member stamps the round, the caller and the id, and restates the id as `hop.subject` on the way back, since a refusal names none) / memory's `bundle`, the facts as `candidates[]` of the JSON in `system.memory.bundle.text`, or its refusal (`hop.reject_reason`) | the facts behind a brief |
| `candidate` | out | `{source: 'objects', candidates: [{id, text, triggers, once: false}]}` or `{..., withdraw: [id]}`, hop `audience_set` | the brief for the curator, in the row's round |
| `alias` / `alias_ack` | out / in | `{aliases: [{alias, canonical}], force: false, messages: []}`, at most 32 aliases per message (a longer set goes out in pieces), hop `alias_tag` = the id / `{done, refused: [{alias, error_code}]}` | the aliases for memory; an ack causes a new `alias` only for a spelling refused `store_refused`, to the same id, at most twice (`alias_tag` `<id>#r<n>`) |

## A row

`objects` is append-only: a change is a new row `rev + 1` whose `supersedes` names the rev it replaces, and the unique index (id, rev) is the compare-and-set of the one writer, `gate`. Columns: `id` (`ob-<12 hex>`), `rev`, `type`, `aliases`, `state` (`candidate`, `active`, `retired`), `origin`, `valid_since`, `audience_set`, `slots` (`{what, where, who[], when, why, how}`), `refs` (`{doc[], related[]}`), `seen`, `turns` (the last 16 turn ids), `supersedes`, `recorded_at`. `keys` maps the normal form of every alias to its id; `pushes` keeps per object the hashes of the last pushed brief and alias set and the last facts. The version a graph space sees is `sha256("<id>:<rev>")[:12]`.

The six slot names are mechanism and stay English; how a product words the question behind a slot, in German or any other language, is a seed of that product and never renames a slot. `who` holds person references (`member:<id>`), never names. The normal form of a name is the memory hive store's `normalize` (lower case, Latin-1 composition, whitespace collapsed), so an alias and a fact subject equate the same spellings.

## Rounds

A row belongs to the round of the turn that first named it, and the round of a row is never widened. A sighting counts for a row only when the row's round covers the sighting's round (nobody of the new round is missing from the row's); otherwise it starts a candidate row of its own. A brief, a candidate, a tool answer and a graph node reach only rounds the row covers; for any other round a row answers `not_found`, exactly like a row that never existed. Aliases in the memory are roundless -- they reveal an id, never content.

## The owner

Only the owner sets `why`, changes the state and confirms a candidate. The owner of a turn is `context.speaker` when it is a `member:` reference standing in the round of the turn (and equal to the param `owner` when that is set); without a speaker -- an unbound channel, a peer, a timer -- there is no owner and the answer is `owner_only`. `context.user_id` and plain membership in the round are no substitute.

## Pushes

Events only. A new version of an active row -- a promotion, a tool write, a sighting -- is announced on `source_changed` and memory is asked on `facts`; memory's answer renders the brief (`<type>: <main name>`, the filled slots in a fixed order, `who` as `<n> people`, at most three facts as `<predicate>: <text>` in memory's order, at most 600 characters by whole parts; a refusal keeps the facts last stored) and pushes it as a `candidate` when its hash changed, and the aliases as `alias` when theirs did. Two rows with the same alias (two rounds): memory keeps the alias on the oldest active one, and `alias_taken` for the younger is expected.

## Knobs

Product wording, types and thresholds are params, never code.

| Cell | Param | Default | Meaning |
|---|---|---|---|
| `gate`, `tools` | `types` | `["thing"]` | the allowed types; the first is the type of a sighting whose `kind` is none of them |
| `gate` | `promote_after` | 3 | distinct turns that make a candidate active |
| `tools` | `slot_max` | 400 | the most characters of one `object_set` value (`too_long`) |
| `tools` | `owner` | `""` | the one `member:` speaker who owns the objects; empty = every `member:` speaker in the round |
| `tools` | `scan_max` | 5000 | the most rows one read of `object_find` takes |
