# `file-space@1.4.8`

The files of one knowledge space, each a logical file hive under one address, over the space's one store ([#899](https://github.com/mmeyerlein/meclaw/issues/899), ADR-0047). Contract tables only; the prose follows with the program it belongs to. The hive is sealed (`params.ports: []`): every endpoint is the hive path. Cells by contract: `store` (store, `write_surface: internal`), `read`, `write`, `guard`, `ws`, `derive`, `embed`, `schemas`, `tools` (code), `summarizer` (llm). The child hive `./projection` ([`projection`](../projection/README.md)) lays a workspace out on a disk. A lane or route enters `config.json` with the cell that serves it (first: `in_read`, `in_ws`, `answer`).

## Address

| Form | Names |
|---|---|
| `fh-<12 hex>` | a file; minted at birth at random, never reused |
| `/a/b.md` | a file by its path (`files.path`); the answer names the id |
| `…@<hex prefix>` | a version: 4 to 64 hex digits, resolved within the file; several matches → `version_ambiguous` with `candidates` |
| `…@ws:<name>` | the file as an open workspace sees it (hop `ws` does the same) |
| `…@snap:<name>` | a named snapshot of the file |
| `…#<anchor>` | a node of the file (see [Nodes and links](#nodes-and-links)); `read` reads its span, `write_node` and `insert` write at it (see [Directories](#directories)), every other op answers `anchor_unsupported` |

A version is the sha256 of the raw bytes; every answer names it by its first 12 hex digits. **No answer field carries the store, the space or the hive path**: moving a file's rows into another store changes no address. The curator's block kind `ref` stays reserved (see [`curator`](../curator/README.md), `blocks.kind`), its body `{"type":"ref","ref":"fh-…@<v12>","text":"<one line>"}`; a pin into a curator goes through `in_pin` with `pins[]`, each `{text, source, until?}`. Nothing in 1.0.0 writes a `ref` or a pin.

## Lanes (into the hive path, on `hop.route`)

| Lane | To | Carries |
|---|---|---|
| `in_read` | `./read`; `hop.op` `ask`, or `search` with `hop.mode` `semantic` → `./derive` | a request |
| `in_write` | `./write` | a request; without `ws` on the main line, with `ws` on the workspace's working version |
| `in_ws` | `./ws`; the projection ops (`ws_materialize`, `ws_exec`, `ws_adopt`, `ws_export_git`, `ws_import_git`, `ws_push`, `ws_pull`) → `./projection` (lane `in_proj`), exclusively | a request |
| `in_model` | `./summarizer`, unless `hop.subscriber` names another cell (the curator's door) | a params message of the llm-registry (ADR-0046) |
| `in_schemas` | `./schemas` | a menu question: body `tools`, the declared names (`*` = all) |
| `in_tool` | `./tools` | one tool call: hop `tool_name` (`file_*`), `tool_call_id`, the arguments as JSON text in the turn; `context.tool_caller` 'cogny' for a write, a workspace op or a projection tool |
| `in_ingest` | `./ingest` | a turn carrying `attachments: [{blob_id, mime_type, filename, size_bytes, sha256}]`, a reference into the colony's blob store, never the bytes (GH #907) |

## Routes (out of the hive path, on `hop.route`)

| Route | Carries |
|---|---|
| `answer` | exactly one per request; hop `op`, `op_id`, `caller` unchanged |
| `model_refused` | a refused `in_model` push (the curator's pattern) |
| `derived` | `{file, version, ok, oneline}` after an `in_derive` with `notify`, only for an empty `caller` |
| `source_changed` | the node source contract ([Nodes and links](#nodes-and-links)): once per head move and once per removed file, whatever the `caller`; the head carries no `caller` |
| `source_described` | `{source, version, path, oneline, tags}` once per stored summary of a file while its version is still the head ([#947](https://github.com/mmeyerlein/meclaw/issues/947)); none without a summary; the head carries no `caller` |
| `tool_schemas` | `{schemas[], unknown[], sidecar[]}` for `in_schemas`, the shape of `memory-hive` |
| `tool_result` | one turn under the call's id for `in_tool`: the answer as JSON text without `op_id` and `caller`; `hop.error_code` on a refusal |
| `turn` | the turn of an `in_ingest`, once, without `attachments`: one text turn after the caption, `[file fh-<id>@<v12> "<name>", <n> pages: <oneline>]` (the page count only for a PDF, counted from its `derived` parts), or `[file "<name>" could not be stored: <code>]`; caption and hop keys unchanged; the turn leaves on what its last try left of the TTL (`./ingest -> ./extract` and `./ingest -> ./write` restore it once per try), and the door of the hive that owns the turn restores it again (GH #929) |

## Request and answer

| Part | Form |
|---|---|
| request body | `{op, file?, args{}}`; `file` an address (not for `list`, `find`, `ws_*`) |
| request hop | `op`, `op_id` (required), `ws?`, `mode?` (`search` only), `caller?` (internal callers only; empty from outside) |
| answer body | `{ok: true, op, op_id, file, version, …}` plus `messages: []` (transport) |
| refusal body | `{ok: false, op, op_id, error: {code, message, candidates?, current?}}` |
| write | every op that moves content (all but `create` and `snapshot`) carries `args.base` (the version read), `force` optional (it skips the hook, nothing else); answer `version` (new), `base`, `diff` (unified, 3 lines context, ≤ 400 lines; `truncated: true` and `cut`, the mark `...[cut: <shown> of <total> chars shown; read the two versions for the rest]`, when cut), `hook` (`ok`/`none`/`forced`; `hook_note` carries the guard's `note` when it passed without checking: `too_large_to_check`, `too_deep_to_check`, `no_text`); `stage` on `replace`; `rebased: true` when moved; `hint: use_replace` on an `overwrite` that changes at most 5 % of the lines |

A line reads as `<n>:<h4>|<text>`, `h4` the first 4 hex digits of the sha256 of the line without `\n` or `\r\n`; `replace_lines` checks exactly these. A version with `derived` pages reads as them, each headed `--- page <n> ---` -- also when its bytes happen to be UTF-8, as a pure-ASCII PDF's are; a version without reads as its own text.

## Internal lanes (`hop.route` of the emission)

| Lane | Between | Carries |
|---|---|---|
| `read_store` (and one route per code cell) | cell → `./store` | store operations; the edge lifts `hop.reply_cell`/`hop.phase` into context `cur_origin`/`cur_phase`, only the store's way back reads `cur_origin` |
| `in_check` / `checked` | `write` → `guard` → `write` | the hook check and its verdict |
| `in_put` / `put` | `ws` → `write` → `ws` | ops `merge3`, `patch`, `create` |
| `in_recover` | `write` → `ws` | `{commit}`; no answer |
| `in_derive` | `write`, `ws` → `derive` | `{file, version}`, hop `notify`/`caller` of the causing write; no answer; a door (`restore_ttl`): the derive job of one file starts with the full colony TTL, however long the tool chain that wrote it ([#995](https://github.com/mmeyerlein/meclaw/issues/995)) |
| `in_extract` / `in_write` | `ingest` → `extract` → `write` | a PDF: `{attachments: [ref], path, mime}`, hop `op_id`; `extract` reads the blob (`consumes.body.attachments`), runs `pdftotext -layout - -` (no shell, no disk) and sends the one `create` on, the pages as `args.derived`, caller `ingest` -- without `derived` when there is no text (`no_extractor`, `extract_failed`, `timeout`, a reader error; stderr names it) |
| `in_write` / `answer`, `derived` | `ingest` (or `extract`) → `write` → `ingest`, `derive` → `ingest` | `create` of a document under `/inbox/<YYYY-MM-DD>/<name>` (a taken name gets ` (2)`, ` (3)` …; two of one name in one batch are two files, a claim row per document in `pending`; `path_taken` searches again, a PDF through `extract` once more), `args.attachment` 0 with `attachments: [ref]`, caller `ingest`, `notify` '1'; `pending` holds the reference, never the bytes or the pages; the turn waits for `derived` |

`op`, `op_id`, `caller`, `ws`, `notify` survive no store phase: a cell parks them in `pending` and knows a reply by route and `op_id`, never by context.

## Store (`./store`, all times UTC)

| Table | Columns | Rule |
|---|---|---|
| `files` | `file`, `path`, `kind`, `mime`, `head`, `head_seq`, `lock`, `tomb`, `oneline`, `bytes`, `lines`, `born_at` | one row per file; `head` the full main-line version ('' = born in a workspace only); `lock` the commit id during prepare, else ''; `tomb` '' or a time; `path` unique among the living |
| `blocks` | `file`, `hash`, `enc`, `size`, `body`, `first_seen` | one row per (`file`, `hash`); the same hash in two files is two rows; `enc` `utf8`/`b64` |
| `versions` | `file`, `version`, `blocks`, `bytes`, `lines`, `parent`, `force`, `made_by`, `at` | one row per (`file`, `version`); `blocks` a JSON array of the block hashes in order |
| `line` | `file`, `seq`, `version`, `prev`, `op`, `note`, `ws`, `commit`, `at` | the main line, append-only; one row per head move; `seq` µs since the epoch, monotonic per file |
| `snaps` | `file`, `name`, `version`, `at` | a named pointer |
| `derived` | `file`, `version`, `kind`, `part`, `body` | the text of a non-text file (`kind` `text`, `part` = page from 1) |
| `summaries` | `file`, `version`, `level`, `text`, `model`, `at` | `level` `oneline`/`short`; the newest per level counts |
| `embeddings` | `file`, `version`, `section`, `from_line`, `to_line`, `blob`, `dim`, `model` | the `memory-hive` form |
| `nodes` | `file`, `version`, `anchor`, `kind`, `parent`, `unit`, `from_pos`, `to_pos`, `oneline`, `parser` | the items of the head only (`version` full); replaced in one bundle per head move, dropped with the tomb |
| `links` | `file`, `version`, `from_anchor`, `kind`, `target_name`, `pos` | the outgoing edges of the head only, as written in it |
| `node_runs` | `file`, `version`, `fmt`, `parser`, `mark`, `nodes`, `links`, `fvec` | one row per head: how it was extracted, and `fvec`, the bitwise majority of its section bits (base64) that `near` compares |
| `ws` | `ws`, `name`, `root`, `base_seq`, `state`, `opened_at`, `closed_at`, `commit` | `state` `open`/`committed`/`discarded`; `root` a path prefix |
| `ws_files` | `ws`, `file`, `base`, `working`, `state`, `at` | written on first touch (copy-on-write); `state` `touched`/`conflict`/`created`/`removed` |
| `commits` | `commit`, `ws`, `state`, `plan`, `note`, `at`, `deadline` | `plan` JSON `[{file, from, to, kind?}]` (`from` '' = born in the workspace, `to` '' = removed); `state` `prepared`/`committed`/`aborted` |
| `pending` | `op_id`, `cell`, `phase`, `body`, `at` | a cell's working values between two store phases, key (`op_id`, `cell`); `./derive` also keeps one row per file (`op_id` `done:<file>`, `phase` = the last derived version) so an older version arriving late writes nothing |

Every query on a table with `file` names `file` in its `where`, bar three reads: `files` across files (path lookup, `list`, `find`), `ws_files` by `ws` (a workspace's touched files) and `node_runs` across files (`near`); every select carries a `limit`.

## Nodes and links

Every head of the main line is cut into **nodes** (its items) and **links** (what it names outside itself) by a pure extractor chain in `./derive`; `./read` runs the same extractors for an older version or a workspace's view. Nodes never depend on a model: a failed summary or embedding still leaves them written.

**Anchors** (`fh-<12 hex>[@<v12>]#<anchor>`): segments `<kind>:<name>` joined by `/`, never a line number; `%`, `#`, `@` and whitespace in a name are percent-encoded.

| Format | Anchors | Links |
|---|---|---|
| python (`.py`; `ast`, else `scan`) | `class:<N>`, `def:<N>` (also `async`), nested `class:A/def:b` | `ast` only: `import` (`a.b`, `a.b:c`, `.x:y`; `as` kept as `alias`), `call` of a name or an imported module alias (`<module>:f`); no `obj.meth()` |
| rust (`.rs`; `scan`) | `fn:`, `struct:`, `enum:`, `trait:`, `mod:` (nested `mod:a/fn:x`), `macro:`, `impl:<Type>`, `impl:<Type>+<Trait>`, methods `impl:<Type>/fn:<m>` (generics and paths cut) | `use` → `import` (`a::{b, c}` two, `a::*` kept) |
| markdown (`.md`, `.markdown`) | `sec:<slug>` flat (GitHub slug, repeats `-1`, `-2`) | `link` (relative, resolved to an absolute space path, `#frag` kept), `url`; nothing from code |
| json, toml | `key:<RFC 6901 pointer>` down to `key_depth`, an array only as its key's node | – |
| pdf (`derived` pages) | `page:<n>` | – |

A repeated anchor in one file takes `~2`, `~3` in source order. `unit` is `line` or `page`, `from`/`to` 1-based inclusive.

**Marks** (`node_runs.mark`, `mark` of `outline` and of the event; `""` when clean): `no_extractor` (no format known), `unparsable` (every extractor of the chain refused: no nodes, no links), `too_large` (text over `extract_max_bytes`: no nodes), `truncated` (more than `nodes_max` nodes or `links_max` links: the first ones kept). A mark never touches the write.

**Node source contract** (route `source_changed`, for every later node source too): after each head move of a file (birth, write, workspace commit, one event per file) `{source: "fh-<12 hex>", version: "<v12>", path, fmt, parser, mark, nodes, links, tomb: false}`, emitted after its rows are written and only while that version is still the head; after a removal (`remove`, or a commit that removes) `{source, path, tomb: true}` -- no `version`: a grave has no head -- once the tomb won, its rows dropped in the bundle after it (a store bundle is no transaction: a lost tomb must not take a living head's rows). A link is `{kind, from_anchor, target_name, pos, alias?}`: `alias` only where the source binds a name of its own (python `import m as a`, `from m import f as a`), the key missing otherwise; two names for one target are two links. A source answers `outline` and `links` page by page over `{source, version?, cursor?, limit?}` -- here the `in_read` body `{op, file: <source>, args}` with an empty `caller`. Source ids carry a prefix of their kind: `fh-` a file; `ob-` is reserved.

## Directories

A directory is a path without a trailing `/` and one row of `dirs`, unique on `path`, kept by `./derive` ([#947](https://github.com/mmeyerlein/meclaw/issues/947)); the root `/` always exists and reads as zeros without a row. Every write that takes a path -- `create` (in a workspace too), the target of `move` and `copy`, `create_dir` -- first inserts a `claims` row, unique on `path` ([#918](https://github.com/mmeyerlein/meclaw/issues/918)): another writer's living claim, a living file at the path or above it, or a living directory at it answers `path_taken`; the claim ends with the write, and one left behind expires after `write.claim_ttl_s` (120 s). The counts come in three stages. Stages 1 (`files`, `bytes`, `nodes`) and 2 (the bit counters of the file vectors, and the counters of the topic tags a summary ends in, `TAGS: a, b`) are propagated: `contrib` holds the share of one file last counted in, and after every head move, `remove`, `move` and committed removal `./derive` moves the difference into the row of every ancestor and marks it `dirty`. A row another writer won is tried again for at most `derive.agg_retries` rounds (default 6, and 6 is also the maximum the sync's routing budget holds: a larger value acts as 6, [#973](https://github.com/mmeyerlein/meclaw/issues/973)); the counters are stored with their sign and read as zero below it, so two syncs give the same sum in either order. Stage 3, a directory's one-line `summary`, is lazy: `dir_summary {path}` (served by `./derive`) writes it from the children's one lines only when the directory changed since, never cascading and never on a write. Two `dir_summary` of one path at once ask the model once: the first holds the claim `y:<path>` in `claims` until its summary is written, the other answers the stored summary with `fresh: false`. The children of a directory are found by their path, so a file row written before `files.dir` existed belongs to its folder too; a summary reads its direct files past any number of deeper ones, skipping a sub-directory whose files fill a read window (at most 17 windows of 2001 rows, the routing budget of the job; past that it says so on the cell's stderr). `subdirs` is counted on reading. The structure ops move the main line only: `create_dir {path}` (`created: false` when it lives), `remove_dir {path}` (`not_empty` while a file or a directory lives in it; a directory is never moved or copied), `move {to}` of a file (id, head and nodes stay; one `source_changed` with the new path), `copy {to}` (a new file with the same version), and `write_node {text, base}` / `insert {where?, text, base}` at `fh-<12 hex>#<anchor>` (the head's span of the node; `base` must be the head). `list` gives each directory entry `files`, `bytes`, `nodes`, `subdirs`, `dirty`, `tags` (the top 8 as `[{tag, n}]`) and `summary` (once written) and shows empty directories too; `info` carries the version's `tags`; `dir_info {path, cursor?, limit?}` answers `{path, parent, files, bytes, nodes, subdirs, dirty, tags, summary, born, children, next}`, the children in path order, 50 per page (at most 200); a page reads at most 2001 rows from its cursor (never from before `path/`), so under many deeper files a page can be empty while `next` is set -- read on until `next` is empty; `{name, path, type: "dir", files, bytes, nodes, dirty, summary}` or `{name, path, type: "file", file, kind, mime, bytes, lines, version, changed, oneline}`. Tools: `file_dir_info` and `file_dir_summary` for every surface, `file_create_dir`, `file_remove_dir`, `file_move`, `file_copy` and `file_write_node` for 'cogny' only; `file_insert` takes an anchor in `file` instead of `line`.

## Ops

| Lane | Op | Arguments | Answer |
|---|---|---|---|
| `in_read` | `info` | – | `version`, `path`, `kind`, `mime`, `bytes`, `lines`, `pages`, `oneline`, `workspaces[]`, `snapshots[{name, version}]`, `tomb?` (the only op on a removed file's head) |
| `in_read` | `read` | `from`/`to` (1-based, inclusive, negative from the end) or `page` (a paged document only: on a version without pages `page` 0 or 1 is ignored and the range or the whole file is read, [#994](https://github.com/mmeyerlein/meclaw/issues/994)); `at?` | `version`, `lines` (total), `from`, `to`, `text`; `page`, `pages` for a page; `ignored: ["page"]` when it was ignored; `more{from, to}` when cut |
| `in_read` | `search` | `pattern` (≤ 500 chars), `mode` `exact`\|`regex`, `context` ≤ 2, `limit` | `version`, `hits[{line, h4, text, before?, after?}]`, `total`; `more` and `cut` (the mark: hits shown of `total`) when the budget stopped the hits |
| `in_read` | `summary` | `level` `oneline`\|`short` | `version`, `level`, `text`, `model`, `at`; `pending: true` while none is written |
| `in_read` | `history` | `limit` (20, ≤ 200) | `version`, `entries[{at, version, op, prev?, note?, ws?, commit?}]`, newest first |
| `in_read` | `show` | `version?` | `version`, `bytes`, `lines`, `at`, `made_by`, `head` (first 40 lines), `parent?`, `force?` |
| `in_read` | `diff` | `a`, `b?` (else the addressed state) | `version`, `a`, `b`, `diff` (unified); `truncated` and `cut` (the mark: characters shown of the whole diff) when cut |
| `in_read` | `list` | `prefix` (`/`), `depth` (1) | `prefix`, `depth`, `entries[{path, file, bytes, lines}` or `{path, dir, files}]`, `more?` |
| `in_read` | `find` | `glob`, `limit` (100, ≤ 500) | `glob`, `files[{file, path}]`, `more?` |
| `in_read` | `raw` | `version?` | `version`, `mime`, `bytes`, `b64` |
| `in_read` | `outline` | `version?`, `cursor?`, `limit` (200, ≤ `nodes_max`) | `version`, `parser`, `mark?`, `nodes[{anchor, kind, parent, unit, from, to, oneline, parser}]`, `next` (`""` at the end) |
| `in_read` | `links` | `version?`, `cursor?`, `limit` (500, ≤ `links_max`) | `version`, `links[{kind, from_anchor, target_name, pos}]`, `next` |
| `in_read` | `near` | `k` (5, ≤ 10) | `near[{file, path, score}]`: the living files closest by `node_runs.fvec` (Hamming, ties by path), without the file itself; `[]` for a file without `fvec`; no model, no embedding call |
| `in_read` | `search` `semantic` | `pattern`, `limit` | served by `./derive`: `version`, `mode`, `hits[{section, from_line, to_line, score, preview}]` over this file and version only (`preview` ≤ 3 lines in read form) |
| `in_read` | `ask` | `question` | served by `./derive`: `version`, `answer`, `sources[{from_line, to_line}]`; the model sees the summary and the best `ask_sections` sections, never the whole file |
| `in_write` | `create`, `overwrite`, `patch` | `path`, `text`\|`b64`\|`attachment`, `mime?`, `derived?`, `notify?` \| `text`\|`b64`\|`attachment`, `derived?`, `notify?` \| `diff` | write answer; `attachment` = index into `body.attachments` (`true` = 0), exactly one content source (`bad_request` else), an entry's `error.code` is the answer's; an attachment is not checked by `./guard`, a moved base is `base_moved` (no merge), the answer has no `diff`, and its bytes and pages are staged as blocks, never parked; `./tools` never passes `attachments` on (a blob id reads any blob of the colony, so only `ingest` and `extract` may set one); a `create` that brings `derived` pages makes the version binary, whatever its source, because the pages are its text (a pure-ASCII PDF is UTF-8) |
| `in_write` | `replace`, `replace_regex`, `replace_lines`, `insert`, `delete` | `old`, `new`, `expected` `1`\|`all`\|n \| `pattern`, `repl`, `expected` \| `from`, `to`, `hashes`, `new` \| `line`, `before`\|`after`, `text` \| `from`, `to` | write answer |
| `in_write` | `snapshot`, `revert`, `remove` | `name` \| `version` \| – | write answer |
| `in_ws` | `ws_open` | `name` (unique among the open; `git` and `git.<…>` are the views of the git projection, opened only by `caller` 'projection' or the owner's door without a caller, else `reserved_name`), `root` (`/`) | `ws`, `name`, `base_seq` |
| `in_ws` | `ws_status`, `ws_diff`, `ws_tree` | – | `files[{file, path, state, base, working, behind}]` / `diff` (base → working, ≤ 400 lines) / `files[{file, path, kind, version}]` under `root` as the workspace sees them, plus `name` and `root` |
| `in_ws` | `ws_patch`, `ws_merge` | `diff` (several files; `--- /dev/null` creates, `+++ /dev/null` removes) \| – | the moved files; a late failure swings every pointer back |
| `in_ws` | `ws_commit`, `ws_discard` | `note` \| – | `ws` (the workspace NAME, also when the request named its id, [#980](https://github.com/mmeyerlein/meclaw/issues/980)), `commit`, `files[{file, version}]` (an empty workspace: `files: []`, no `commit`) \| – |
| `in_ws` | `ws_materialize`, `ws_exec`, `ws_adopt` | see [`projection`](../projection/README.md) | answered by `./projection`; `caller` 'projection' on its own requests to `read`, `write`, `ws`, whose answers go back to it on `in_answer`; each of those requests is one file step of the job and arrives with the full colony TTL (`restore_ttl` on `./projection -> ./ws`, `./read`, `./write`, [#975](https://github.com/mmeyerlein/meclaw/issues/975)), the job's file list bounds the steps |

## Tools (`./schemas`, `./tools`)

| Tools | Op, lane | Who |
|---|---|---|
| `file_info`, `file_read`, `file_search`, `file_summary`, `file_ask`, `file_history`, `file_show`, `file_diff`, `file_list`, `file_find`, `file_outline`, `file_links` | `<op>` on `in_read` (`ask`, `search` `semantic` → `./derive`) | every surface |
| `file_create`, `file_replace`, `file_replace_regex`, `file_replace_lines`, `file_insert`, `file_delete`, `file_overwrite`, `file_patch`, `file_snapshot`, `file_revert`, `file_remove` | `<op>` on `in_write` | `context.tool_caller` 'cogny' only |
| `file_ws_open`, `file_ws_status`, `file_ws_diff`, `file_ws_patch`, `file_ws_merge`, `file_ws_commit`, `file_ws_discard` | `ws_<x>` on `in_ws` | `context.tool_caller` 'cogny' only |
| `file_ws_exec` `{ws, argv, timeout_ms?}`, `file_ws_export` `{root, note}`, `file_ws_push` `{root, remote?}` | `ws_exec`, `ws_export_git`, `ws_push` on `in_proj` of `./projection` ([#980](https://github.com/mmeyerlein/meclaw/issues/980)) | `context.tool_caller` 'cogny' only; on a menu only where `./schemas` has `projection_tools` "1" |

Argument `file` goes into the body, `ws` onto the hop (a workspace name), `mode` of `file_search` onto the hop as well, everything else into `args`; the call id is the request's `op_id` and `caller` is 'tools'. Every argument is checked against the tool's schema first: a mismatch answers `bad_request` and no other cell runs. No tool for `raw`, `ws_tree`, `ws_materialize`, `ws_adopt`, `ws_import_git` or `ws_pull`.

The three projection tools ([#980](https://github.com/mmeyerlein/meclaw/issues/980)) let a writer test, export and push with one call each. `file_ws_exec` runs `argv` in the directory of a workspace it opened (argv[0] in the projection's `exec_allow`, else `not_allowed`; the cap is `min(timeout_ms, exec_timeout_ms)`) and answers like `ws_exec` -- `ws` (the workspace NAME), `argv` as run, `exit`, `out_tail`, `err_tail`. `file_ws_export` exports the main line under `root` into the git projection of that root (the git cell opens and discards the view, see [`projection`](../projection/README.md) § git) and answers `{root, commit, files, subject}` (`subject` as git logs it), or `nothing_to_commit` with the `commit` HEAD holds -- also when the tree is what the last pull brought in: HEAD then moves onto that pulled commit and `previous` names the one it left (L-8). `file_ws_push` pushes that projection to `remote`, a name of the git cell's `remotes` (`origin` when the call names none; a URL or path is `remote_unknown`), and answers `{root, remote, branch, commit}`. Both echo `root` as called, so a record of an export and its push names the folder it belongs to. Without the owner's `base_path`, `git_dir` and `exec_allow` they answer `no_base_path` / `no_git_dir` / `not_allowed`, never nothing. Their answer leaves `./projection` for `./tools` (`caller` 'tools'), and the edge into the projection is a door (`restore_ttl`): an export of many files runs within the colony TTL. Every context key the call carried comes back on its `tool_result`.

## Error codes

| Code | When |
|---|---|
| `bad_request` / `unknown_op` / `bad_path` | no `op_id`, a malformed argument, a bad workspace name / an op the lane does not serve / a path a file cannot take |
| `bad_address` / `anchor_unsupported` / `unknown_anchor` | not `fh-<12 hex>` or `/path`, or a malformed suffix / an address with `#…` on any op but `read`, `write_node` and `insert` / `read` of an anchor the version has no node for (`candidates`: ≤ 5 anchors with the same last name) |
| `not_found` / `tombstoned` | no such file, or none at that state / the head of a removed file (its versions stay readable) |
| `version_unknown` / `version_ambiguous` | no / several versions of the file start with the prefix (`candidates`) |
| `snap_unknown` / `ws_unknown` / `ws_closed` | no snapshot / no open workspace of that name (`read`) / a write into a committed or discarded workspace |
| `corrupt` / `no_text` | a version or block the file names is missing / the version has neither text nor derived pages |
| `not_indexed` | `ask` or `search` `semantic` on a version without embeddings (not derived yet, `derive.embed` "0", or a working version); `error.current` names the version |
| `embed_failed` / `model_failed` | the embedding endpoint / the summarizer failed on `ask` or `search` `semantic` |
| `bad_pattern` / `bad_range` / `page_unknown` | a pattern of the wrong size or no regex / a range with no line / no such page (on a version without pages: a `page` other than 0 or 1) |
| `too_long` | a `regex` search over a file with a line longer than the budget of the window (a tenth of the reading model's `input_soft`): the expression is not run over it, its backtracking has no bound; the message names the line, its length and the budget, and mode `exact` searches it whole (GH #1085) |
| `store_error` | the store refused a read (or, in `./derive`, a write) |
| `base_moved` | a write overlaps what moved since `base`; with the current lines (`lines`, read form) and the new token (`current`). `use_replace` is no refusal: it is the `hint` of an `overwrite` that landed |
| `base_required` / `not_text` / `out_of_range` | a write without `args.base` / a line op on a binary file / a line number outside the base (`lines` = its count; `insert` at the count + 1, `before` or `after`, appends instead, [#996](https://github.com/mmeyerlein/meclaw/issues/996)) |
| `path_taken` / `too_large` | `create` on a path a living file holds (`file` names it) / content over `write.max_bytes` |
| `ambiguous` | `replace`: `old` matches other than `expected` times on the first stage with a hit (`stage`, `count`, `lines`); no hit on any stage is `not_found` with `count: 0` |
| `stale_lines` | `replace_lines`: an `h4` of `hashes` no longer names its line (`lines` = the current ones, read form) |
| `count_mismatch` | `replace_regex` matched other than `expected` times (`count`) |
| `patch_failed` | a hunk's context is not found, or found more than once, within ± 50 lines of its place (`hunk`, 1-based); nothing is applied |
| `snap_exists` | `snapshot` under a name the file already has |
| `syntax` | the guard refused the new content: `lang`, `line`, `col`, and `preview` / `original` (± 5 lines around the place in the new and the old content, read form); the head stays |
| `busy` | the file is locked by a commit, or the compare-and-swap lost three times |
| `conflict` | a merge in `ws_commit` overlaps; the commit aborts |
| `ws_exists` / `outside_root` | `ws_open` with the name of an open workspace / a `ws_patch` path outside the workspace's `root` |
| `reserved_name` | `ws_open` of `git` or `git.<…>` by a caller other than the projection or the owner's door ([#980](https://github.com/mmeyerlein/meclaw/issues/980)) |
| `patch_invalid` / `patch_failed` | a diff that does not parse, or renames / a hunk that does not apply (a `ws_patch` creating a taken path answers `path_taken`) |
| `merge_failed` | `./write` refused a merge of `ws_merge` or `ws_commit` |
| `read_only` | `./tools`: a write or workspace tool called by a surface other than the reasoning core (`context.tool_caller` ≠ 'cogny') |

## Knobs

| Cell | Knob | Default | Meaning |
|---|---|---|---|
| `read` | `max_lines`, `search_limit`, `diff_lines` | 2000, 20, 400 | the most lines one `read` carries (`more` names the next range), hits one `search` lists, lines one `diff` carries |
| `read` | `max_chars` | unset | the characters one `read`, `search` or `diff` carries are a tool result's share (10 %) of the reading model's window, `input_soft` on the request (`./tools` hands on the one the tool edge stamped) or in its context, at three characters a token; a cut read says `cut`, the mark with what it shows, the total and the read that gets the rest; a single line over the budget, in a read or a search hit, is cut with the mark in its text; no window, no character bound -- lines are searched and shown whole ([#1085](https://github.com/mmeyerlein/meclaw/issues/1085)). A number is an owner's override |
| `write` | `max_bytes` | 25 MiB | set by its cell |
| `guard` | `max_check_bytes` | 1 MiB | the largest text the syntax hook parses; a larger one answers `none` with `note` `too_large_to_check` |
| `derive` | `summary_on_commit`, `embed`, `ask_sections`, `section_lines` | "1", "1", 4, 60 | a summary on every main-line head move (birth always gets one); embeddings per section; sections one `ask` hands the model; lines per window where a file has no headings |
| `derive` | `input_soft_fallback`, `embed_input_soft_fallback` (rows in `input_soft_fallback_row`, `embed_input_soft_fallback_row`) | catalogue rows | the summarizer's and the embedder's window in tokens, the `input_soft` of the catalog rows of the models they are born on ([#1085](https://github.com/mmeyerlein/meclaw/issues/1085), OR-IG-9): the summarizer's model is a birth token (`MODEL_FILE_SPACE`), so its row is the llm-registry's `light` tier, `openai/gpt-6-luna`; the embedder's is the embedding row of its default model, `google/gemini-embedding-2`. The `_row` params are labels the drift lock `gh1085_fallback_windows_match_the_catalog` reads, not the script. A summary reads half the window (a longer file is cut with the mark naming the sections left out), a section over the embedding window is embedded as several pieces at line borders. 0: whole; a summary the summarizer refuses as too long is asked once more with half of the window the refusal names |
| `derive`, `read` | `nodes_max`, `links_max`, `extract_max_bytes`, `key_depth` | 2000, 5000, 2 MiB, 3 | the most nodes / links one version keeps (`truncated` above), the largest text extracted (`too_large` above), the depth of `key:` nodes |
| `read` | `near_scan_max` | 5000 | the most files one `near` compares |
| `extract` | `extract_cmd`, `extract_max_pages`, `extract_timeout_ms` | `pdftotext`, 500, 30000 | the program that turns a PDF into pages (a name found in `/usr/bin:/bin`, the only `PATH` it runs with, or an absolute path), the most pages one `create` carries as `derived`, the time one extraction may take |
| `embed` | `endpoint`, `model`, `dim` | the `memory-hive` embedding settings (`MEMORY_EMBED_*`) | one embedding model, one dimension and one binary form per member |
| `schemas` | `projection_tools` | "0" | "1" puts `file_ws_exec`, `file_ws_export` and `file_ws_push` on the menu ([#980](https://github.com/mmeyerlein/meclaw/issues/980)); the owner sets it beside the projection's `base_path`, so no model sees a tool that always refuses |
| `summarizer` | `model` | `${MODEL_FILE_SPACE}`, no default -- growth is refused without it (`env_var_missing`), as for the `MODEL_*` of `memory-hive` | the model that writes summaries and answers `ask`; the llm-registry may move it through `in_model` |

## Versioning

`1.1.0` takes the **second** digit ([#905](https://github.com/mmeyerlein/meclaw/issues/905), [#906](https://github.com/mmeyerlein/meclaw/issues/906), [#907](https://github.com/mmeyerlein/meclaw/issues/907), [#908](https://github.com/mmeyerlein/meclaw/issues/908)): the child hive
`./projection` lays a workspace out on a disk, runs a permitted program in it and carries
it into git and back; `in_ingest` stores a document a channel sent as a file; `in_schemas` and
`in_tool` put the file tools on a model's menu. Lanes joined the boundary and none left.
