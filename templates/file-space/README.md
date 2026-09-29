# `file-space@1.0.0`

The files of one knowledge space, each a logical file hive under one address, over the space's one store ([#899](https://github.com/mmeyerlein/meclaw/issues/899), ADR-0047). Contract tables only; the prose follows with the program it belongs to. The hive is sealed (`params.ports: []`): every endpoint is the hive path. Cells by contract: `store` (store, `write_surface: internal`), `read`, `write`, `guard`, `ws`, `derive`, `embed` (code), `summarizer` (llm). A lane or route enters `config.json` with the cell that serves it (first: `in_read`, `in_ws`, `answer`).

## Address

| Form | Names |
|---|---|
| `fh-<12 hex>` | a file; minted at birth at random, never reused |
| `/a/b.md` | a file by its path (`files.path`); the answer names the id |
| `…@<hex prefix>` | a version: 4 to 64 hex digits, resolved within the file; several matches → `version_ambiguous` with `candidates` |
| `…@ws:<name>` | the file as an open workspace sees it (hop `ws` does the same) |
| `…@snap:<name>` | a named snapshot of the file |
| `…#…` | reserved for anchors → `anchor_unsupported` |

A version is the sha256 of the raw bytes; every answer names it by its first 12 hex digits. **No answer field carries the store, the space or the hive path**: moving a file's rows into another store changes no address. The curator's block kind `ref` stays reserved (see [`curator`](../curator/README.md), `blocks.kind`), its body `{"type":"ref","ref":"fh-…@<v12>","text":"<one line>"}`; a pin into a curator goes through `in_pin` with `pins[]`, each `{text, source, until?}`. Nothing in 1.0.0 writes a `ref` or a pin.

## Lanes (into the hive path, on `hop.route`)

| Lane | To | Carries |
|---|---|---|
| `in_read` | `./read`; `hop.op` `ask`, or `search` with `hop.mode` `semantic` → `./derive` | a request |
| `in_write` | `./write` | a request; without `ws` on the main line, with `ws` on the workspace's working version |
| `in_ws` | `./ws` | a request |
| `in_model` | `./summarizer`, unless `hop.subscriber` names another cell (the curator's door) | a params message of the llm-registry (ADR-0046) |

## Routes (out of the hive path, on `hop.route`)

| Route | Carries |
|---|---|
| `answer` | exactly one per request; hop `op`, `op_id`, `caller` unchanged |
| `model_refused` | a refused `in_model` push (the curator's pattern) |
| `derived` | `{file, version, ok, oneline}` after an `in_derive` with `notify`, only for an empty `caller` |

## Request and answer

| Part | Form |
|---|---|
| request body | `{op, file?, args{}}`; `file` an address (not for `list`, `find`, `ws_*`) |
| request hop | `op`, `op_id` (required), `ws?`, `mode?` (`search` only), `caller?` (internal callers only; empty from outside) |
| answer body | `{ok: true, op, op_id, file, version, …}` plus `messages: []` (transport) |
| refusal body | `{ok: false, op, op_id, error: {code, message, candidates?, current?}}` |
| write | every op that moves content (all but `create` and `snapshot`) carries `args.base` (the version read), `force` optional (it skips the hook, nothing else); answer `version` (new), `base`, `diff` (unified, 3 lines context, ≤ 400 lines; `truncated: true` when cut), `hook` (`ok`/`none`/`forced`; `hook_note` carries the guard's `note` when it passed without checking: `too_large_to_check`, `too_deep_to_check`, `no_text`); `stage` on `replace`; `rebased: true` when moved; `hint: use_replace` on an `overwrite` that changes at most 5 % of the lines |

A line reads as `<n>:<h4>|<text>`, `h4` the first 4 hex digits of the sha256 of the line without `\n` or `\r\n`; `replace_lines` checks exactly these. A non-text version reads as its `derived` pages, each headed `--- page <n> ---`.

## Internal lanes (`hop.route` of the emission)

| Lane | Between | Carries |
|---|---|---|
| `read_store` (and one route per code cell) | cell → `./store` | store operations; the edge lifts `hop.reply_cell`/`hop.phase` into context `cur_origin`/`cur_phase`, only the store's way back reads `cur_origin` |
| `in_check` / `checked` | `write` → `guard` → `write` | the hook check and its verdict |
| `in_put` / `put` | `ws` → `write` → `ws` | ops `merge3`, `patch`, `create` |
| `in_recover` | `write` → `ws` | `{commit}`; no answer |
| `in_derive` | `write`, `ws` → `derive` | `{file, version}`, hop `notify`/`caller` of the causing write; no answer |

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
| `ws` | `ws`, `name`, `root`, `base_seq`, `state`, `opened_at`, `closed_at`, `commit` | `state` `open`/`committed`/`discarded`; `root` a path prefix |
| `ws_files` | `ws`, `file`, `base`, `working`, `state`, `at` | written on first touch (copy-on-write); `state` `touched`/`conflict`/`created`/`removed` |
| `commits` | `commit`, `ws`, `state`, `plan`, `note`, `at`, `deadline` | `plan` JSON `[{file, from, to, kind?}]` (`from` '' = born in the workspace, `to` '' = removed); `state` `prepared`/`committed`/`aborted` |
| `pending` | `op_id`, `cell`, `phase`, `body`, `at` | a cell's working values between two store phases, key (`op_id`, `cell`); `./derive` also keeps one row per file (`op_id` `done:<file>`, `phase` = the last derived version) so an older version arriving late writes nothing |

Every query on a table with `file` names `file` in its `where`, bar two reads: `files` across files (path lookup, `list`, `find`) and `ws_files` by `ws` (a workspace's touched files); every select carries a `limit`.

## Ops

| Lane | Op | Arguments | Answer |
|---|---|---|---|
| `in_read` | `info` | – | `version`, `path`, `kind`, `mime`, `bytes`, `lines`, `pages`, `oneline`, `workspaces[]`, `snapshots[{name, version}]`, `tomb?` (the only op on a removed file's head) |
| `in_read` | `read` | `from`/`to` (1-based, inclusive, negative from the end) or `page`; `at?` | `version`, `lines` (total), `from`, `to`, `text`; `page`, `pages` for a page; `more{from, to}` when cut |
| `in_read` | `search` | `pattern` (≤ 500 chars), `mode` `exact`\|`regex`, `context` ≤ 2, `limit` | `version`, `hits[{line, h4, text, before?, after?, long_line?}]`, `total` |
| `in_read` | `summary` | `level` `oneline`\|`short` | `version`, `level`, `text`, `model`, `at`; `pending: true` while none is written |
| `in_read` | `history` | `limit` (20, ≤ 200) | `version`, `entries[{at, version, op, prev?, note?, ws?, commit?}]`, newest first |
| `in_read` | `show` | `version?` | `version`, `bytes`, `lines`, `at`, `made_by`, `head` (first 40 lines), `parent?`, `force?` |
| `in_read` | `diff` | `a`, `b?` (else the addressed state) | `version`, `a`, `b`, `diff` (unified), `truncated?` |
| `in_read` | `list` | `prefix` (`/`), `depth` (1) | `prefix`, `depth`, `entries[{path, file, bytes, lines}` or `{path, dir, files}]`, `more?` |
| `in_read` | `find` | `glob`, `limit` (100, ≤ 500) | `glob`, `files[{file, path}]`, `more?` |
| `in_read` | `raw` | `version?` | `version`, `mime`, `bytes`, `b64` |
| `in_read` | `search` `semantic` | `pattern`, `limit` | served by `./derive`: `version`, `mode`, `hits[{section, from_line, to_line, score, preview}]` over this file and version only (`preview` ≤ 3 lines in read form) |
| `in_read` | `ask` | `question` | served by `./derive`: `version`, `answer`, `sources[{from_line, to_line}]`; the model sees the summary and the best `ask_sections` sections, never the whole file |
| `in_write` | `create`, `overwrite`, `patch` | `path`, `text`\|`b64`, `mime?`, `derived?`, `notify?` \| `text`\|`b64`, `derived?`, `notify?` \| `diff` | write answer |
| `in_write` | `replace`, `replace_regex`, `replace_lines`, `insert`, `delete` | `old`, `new`, `expected` `1`\|`all`\|n \| `pattern`, `repl`, `expected` \| `from`, `to`, `hashes`, `new` \| `line`, `before`\|`after`, `text` \| `from`, `to` | write answer |
| `in_write` | `snapshot`, `revert`, `remove` | `name` \| `version` \| – | write answer |
| `in_ws` | `ws_open` | `name` (unique among the open), `root` (`/`) | `ws`, `name`, `base_seq` |
| `in_ws` | `ws_status`, `ws_diff`, `ws_tree` | – | `files[{file, path, state, base, working, behind}]` / `diff` (base → working, ≤ 400 lines) / `files[{file, path, kind, version}]` under `root` as the workspace sees them |
| `in_ws` | `ws_patch`, `ws_merge` | `diff` (several files; `--- /dev/null` creates, `+++ /dev/null` removes) \| – | the moved files; a late failure swings every pointer back |
| `in_ws` | `ws_commit`, `ws_discard` | `note` \| – | `commit`, `files[{file, version}]` (an empty workspace: `files: []`, no `commit`) \| – |

## Error codes

| Code | When |
|---|---|
| `bad_request` / `unknown_op` / `bad_path` | no `op_id`, a malformed argument, a bad workspace name / an op the lane does not serve / a path a file cannot take |
| `bad_address` / `anchor_unsupported` | not `fh-<12 hex>` or `/path`, or a malformed suffix / an address with `#…` |
| `not_found` / `tombstoned` | no such file, or none at that state / the head of a removed file (its versions stay readable) |
| `version_unknown` / `version_ambiguous` | no / several versions of the file start with the prefix (`candidates`) |
| `snap_unknown` / `ws_unknown` / `ws_closed` | no snapshot / no open workspace of that name (`read`) / a write into a committed or discarded workspace |
| `corrupt` / `no_text` | a version or block the file names is missing / the version has neither text nor derived pages |
| `not_indexed` | `ask` or `search` `semantic` on a version without embeddings (not derived yet, `derive.embed` "0", or a working version); `error.current` names the version |
| `embed_failed` / `model_failed` | the embedding endpoint / the summarizer failed on `ask` or `search` `semantic` |
| `bad_pattern` / `bad_range` / `page_unknown` | a pattern of the wrong size or no regex / a range with no line / no such page |
| `store_error` | the store refused a read (or, in `./derive`, a write) |
| `base_moved` | a write overlaps what moved since `base`; with the current lines (`lines`, read form) and the new token (`current`). `use_replace` is no refusal: it is the `hint` of an `overwrite` that landed |
| `base_required` / `not_text` / `out_of_range` | a write without `args.base` / a line op on a binary file / a line number outside the base (`lines` = its count) |
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
| `patch_invalid` / `patch_failed` | a diff that does not parse, or renames / a hunk that does not apply (a `ws_patch` creating a taken path answers `path_taken`) |
| `merge_failed` | `./write` refused a merge of `ws_merge` or `ws_commit` |

## Knobs

| Cell | Knob | Default | Meaning |
|---|---|---|---|
| `read` | `max_lines`, `max_chars`, `search_limit`, `diff_lines` | 2000, 25000, 20, 400 | the most lines / characters one `read` carries (`more` names the next range), hits one `search` lists, lines one `diff` carries |
| `write` | `max_bytes` | 25 MiB | set by its cell |
| `guard` | `max_check_bytes` | 1 MiB | the largest text the syntax hook parses; a larger one answers `none` with `note` `too_large_to_check` |
| `derive` | `summary_on_commit`, `embed`, `ask_sections`, `section_lines` | "1", "1", 4, 60 | a summary on every main-line head move (birth always gets one); embeddings per section; sections one `ask` hands the model; lines per window where a file has no headings |
| `embed` | `endpoint`, `model`, `dim` | the `memory-hive` embedding settings (`MEMORY_EMBED_*`) | one embedding model, one dimension and one binary form per member |
