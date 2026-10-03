# `librarian@1.0.1`

The catalogue of one knowledge space ([#950](https://github.com/mmeyerlein/meclaw/issues/950)): one row per source with its path, kind, format, summary line, tags and the names of its top-level items, kept current from the space's own announcements, so that "where is the thing called X" is answered from one store and wakes no file. The hive is sealed (`params.ports: []`). Cells: `index` (code: announcement, pull, write), `query` (code: the questions), `tools` and `schemas` (code: the tool adapter and the menu), `store` (store, `write_surface: internal`). No model, no embedding request, no network.

## Lanes

| Lane | Direction | Body | Meaning |
|---|---|---|---|
| `source_changed` | in | `{source, version, path, fmt, parser, mark, nodes, links, tomb}` | a source (`fh-<12 hex>`; `ob-` is reserved) moved a head, or was removed (`{source, path, tomb: true}`, no version) |
| `source_described` | in | `{source, version, path, oneline, tags}` | the source wrote the summary of a head: summary line and tags, taken only while `version` is the announced one |
| `pull` | out | `{op, file?, args}`, hop `op`, `op_id`, no `caller` | `info` and `outline` per announcement (`lib:f:i:<part>:<source>:<version>`), `list` for `tree` (`lib:f:t:<token>`), a graph question for `symbol`/`related` (`lib:g:<token>`) |
| `in_pulled` | in | the answering space's `answer`, `op_id` mirrored | written (catalogue) or answered (questions); the token is the question itself, nothing is parked |
| `in_lib` | in | `{op, args}`, hop `op`, `op_id`, `caller?` | a question; exactly one `answer` |
| `answer` | out | `{ok, op, op_id, items, next, ...}` or `{ok: false, op, op_id, error}` | codes `invalid_input`, `unknown_op`, `store_error`, or the other space's own error |
| `in_tool` / `tool_result` | in / out | one tool call / one `tool_result` turn | `lib_find`, `lib_symbol`, `lib_related`, `lib_tree`; refused here: `tool_unknown`, `bad_request` |
| `in_schemas` / `tool_schemas` | in / out | `tools` / `{schemas, unknown, sidecar: []}` | the menu, `*` = all four |

## The contract with a source

An announcement writes path, folder and format at once and pulls `info` (the kind) and `outline` (the top-level names) of that version. The summary line and the tags arrive later on `source_described`, because a source announces a head before its model has summarised it; they are taken only while that version is still the announced one. Each answer is written with `announced == version` in its `where`: an answer that arrives after a newer announcement changes nothing. The path comes only from the event, so a move is just another announcement. A removal turns the row into a grave (`tomb` '1'); nothing is ever deleted.

## Questions (`in_lib`)

| Op | Args | Items |
|---|---|---|
| `find` | `q`, `kind?`, `fmt?`, `under?` | `{file, path, kind, fmt, oneline, score, why}`, best first |
| `symbol` | `name` | `{addr, match, path, oneline}` (graph space `resolve`) |
| `related` | `addr`, `how` (`callers`, `dependents`, `deps`, `similar`), `depth?` (1–3) | the graph's items plus `path`, `oneline` |
| `tree` | `under?` (`/`), `depth?` (1–3) | `entries` of the file space's `list`, by path, `more?` |

The catalogue keeps every word run and its parts at case boundaries (`HTTPServer` → `httpserver`, `http`, `server`); `find` cuts the question into the parts, requires every part as a prefix, and says `why` a row matched (`name`, `path`, `tag`, `text`); `score` adds the best field each word hit (4, 3, 2, 1) to the full-text rank folded into 0..1. Every list takes `limit` (default 20, at most 20) and `cursor`; a non-empty `next` is the cursor of the following page.

## Wiring in a member

`./file-space -> ./librarian` on `source_changed` and on `source_described`, each with `restore_ttl` (a door); `./librarian -> ./file-space` on `pull` with `hop.op_id.startsWith('lib:f:')` restamped to `in_read`; `./librarian -> ./graph-space` on `pull` with `lib:g:` restamped to `in_graph`; both spaces' `answer` with `hop.op_id.startsWith('lib:')` back restamped to `in_pulled`. The tools ride the assistant level like the file tools: `tool` with `tool_name.startsWith('lib_')` restamped to `in_tool`, `tool_result` back to `in_tool`, `schemas` to `in_schemas`, and `tool_schemas` back to `in_menu` with `context.tool_answerer` 'library'.

## Knobs

| Cell | Param | Default | Meaning |
|---|---|---|---|
| `index` | `names_max` | 64 | the most top-level names one source keeps |
| `query` | `scan_max` | 2000 | the most rows one `find` reads (`truncated` when reached) |
