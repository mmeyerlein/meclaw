# `curator@1.2.0`

The window of one model, owned in one place, with a ledger of every call. Contract tables only; the prose follows with the program it belongs to.

Since 1.1.0 ([#892](https://github.com/mmeyerlein/meclaw/issues/892), [#893](https://github.com/mmeyerlein/meclaw/issues/893), [#895](https://github.com/mmeyerlein/meclaw/issues/895), [#896](https://github.com/mmeyerlein/meclaw/issues/896)) the window follows a role (`role`: `talky`,
`consult`, `coding`, `research`), foreign words and tool results stand behind a short id
`[#<12 hex>]` the model can release or pin through the `window` section of its answer, another hive
can pin a text into the window (`in_pin`), and the hive answers the collector's menu question itself
(`./schemas`). Three cells joined it: `./history` answers the `history_*` tools out of the ledger,
`./push` builds the memory leg's question and looks a `gap` up after the answer, and `./handover`
hands a new session a note on the last one and a renewed duplex call a block of the recent
conversation. Lanes joined the boundary, so it is the second digit.

Since 1.1.1 ([#904](https://github.com/mmeyerlein/meclaw/issues/904)) the cache clock is one standing order under a fixed id per hive,
re-armed on every call (`add` with `rearm`), so the timer keeps one row for it instead of one
per call. No lane moved; a promise is repaired, so it is the third digit.

## Lanes (into the hive path, on `hop.route`)

| Lane | From | Carries |
|---|---|---|
| `in_curate` | the context collector | `messages[]` the whole running round (opening turn, evidence pairs, every tool iteration so far), never history; `system` the per-round slots; `tool_scope` if set; `hop.session_id`, `hop.turn_id`, `hop.iter` and whatever else the caller puts on the call (no session = `default`, no turn = a round of its own). An `assistant` text in a round without a tool call of its own (a duplex turn pair; the evidence pairs `memory_recall` and `affinity_brief` do not count) is a final answer and an episode. Answered by one `brain`. |
| `in_slots` | the context collector | `system` only (tool menu, sidecar contract). Kept as the collector's slots, sent with the next call whose system changed; no call of its own. |
| `in_llm` | the tap: every output of the model | the raw output with `hop.finish_reason`, `hop.model`, `hop.tokens_*`, `hop.cost`, `hop.cache_expires_at`, `hop.context_window`, and `context.curator_call`, `context.session_id`, `context.turn_id`, `context.iter`. A final answer (`stop`, `length`) enters the wall from here; the sentence beside a tool call (`tool_calls`) enters as an interim answer, not final, never an episode. Without a session it is session `default`, without a turn the round of its call (the rule of `brain`); without `context.curator_call` it is dropped and said on stderr. |
| `in_pack` | an identity pusher | `system.{identity,persona,handover,instructions}` (or `slot` + `content`); the context rides on to `pack_ack` untouched. |
| `in_close` | the session keeper | `context.session_id` of the closed session. |
| `in_model` | the llm-registry | a params-only body for `./summarizer`, only when `hop.subscriber` names it (the one model door of the hive). |
| `in_section` | the parent's splitter | one section of the sidecar block of an answer of the model (`hop.section`, body `{messages: [], section, payload}`, the answer's context). `window` (`release`/`pin`: block ids) becomes `marks` rows; `gap` goes to `./push` (through `./policy`), which marks it once and looks it up after the answer; `memory` a `topic` mark (`{movement, name}`, movement start/continue/end) and, when `./intake` `pass_sections` names it, leaves again unchanged on `sidecar`. Any other section is dropped and said. |
| `in_pin` | another hive | `pins[]`, each `{text, source, until?}`: `source` one path segment, never `model`; `until` RFC 3339. Optional `replace_sources[]` (at most 16, each by the `source` rule): every live pin of those sources ends first (`until` = now, no row deleted), except a hash the new set carries; `pins: []` with it empties those sources. A block of kind `pin` and a `pins` row; the window shows it under `system.pinned.<source>.<id>`. Nothing else of the hive is written through this door. |
| `in_schemas` | the collector's menu question | `tools[]` the declared names. `./schemas` answers on `tool_schemas` with the curator's tools and sections (`CURATOR_OFFER`). |
| `in_history_call` | the parent's dispatcher, on `hop.tool_name` starting `history_` | one `history_search`, `history_read` or `history_outline` call (GH #893): one `tool_call` turn, the arguments as its text, `hop.tool_call_id`, `hop.tool_name`, the round's context. Answered by one `tool_result` out of this hive's ledger. |
| `in_recall_ask` | the context collector | its per-turn memory ask (route `recall`): `phase`, `turn_id`, `session_id`, `iter`, `recall_query`, `memory_tier`, `recall_window_from`, `recall_window_to`, the person's words as `messages[]`. Answered by exactly one `recall` (`./push`, through `./policy` for the knobs); never held back. |
| `in_gap_bundle` | the memory, through the parent's edge | the answer to a gap's own ask, recognised by `context.gap_ask`; never the collector's. A find becomes an addendum; an empty bundle, a reject or the empty-state sentence is nothing. |
| `in_renewed` | the member, from a duplex channel | a live session of a running call was renewed at its provider's limit (GH #896): `hop.call_id`, `hop.renewal_n`. Answered by one `sidecar` without a model. |

## Routes (out of the hive path, on `hop.route`)

| Route | To | Carries |
|---|---|---|
| `brain` | the model | `messages[]` the window; `system` only when a family changed, each changed family one `$replace` root with all its leaves; `tool_scope` unchanged -- an `allow` list stays the channel's whitelist, and a channel that wants the history tools names them in it (GH #845, OR-KY-72); the `in_curate` hop unchanged plus `hop.curator_call`. The parent's edge promotes `curator_call`, `turn_id`, `session_id`, `iter` into context. |
| `turn_write` | whoever keeps episodes | one participant turn per message (`user`, the final `assistant` answer without its sidecar block, `peer` with `speaker`/`speaker_ref`); `hop.turn_id` `<session_id>#<index>` (index from 0 per session), `hop.turn_index`, `hop.happened_at`, `hop.session_id`, `hop.iter`, `hop.phase`. |
| `write` | the close pass | `messages[]` the participant turns of the session, `rounds` every other wall row raw (`seq`, `turn_id`, `iter`, `kind`, `hash`, `nth`, `at`, `turn`); `hop.turn_id` `close-<session_id>`, `hop.turn_count`, `hop.round_count`. Empty for a session with no rows. |
| `pack_ack` | the pusher | `hop.pack_owner` (off the envelope), `hop.pack_slots`, `hop.error_code` (empty, `slot_unknown`, `pack_empty`), `hop.pack_unknown`. |
| `model_refused` | the registry | a push `./summarizer` refused, with `hop.refused_subscriber`. |
| `sidecar` | the section's own door | a section named in `pass_sections`, the splitter's body and hop unchanged (a talky's `memory` keeps its door to the memory hive); and, from `./push`, a gap's find in a duplex call (`context.engine` `duplex`) as the section `fact`, for the voice model. And the handover for a renewed live session (GH #896): `hop.section` `context`, `hop.call_id`, `hop.renewal_n`, the words under `payload`; `context.call_id` is set on the way out, and the call's channel takes it as `in_advise`. |
| `tool_schemas` | the collector, as `in_menu` with `context.tool_answerer` `curator` | `schemas[]` (the tools the question named, `*` = all), `unknown[]`, `sidecar[]` (every section), `hop.operation` `schemas`. |
| `tool_result` | the parent's collector, as `in_tool` | the answer to one `in_history_call`: one `tool_result` turn under the call id, its text a JSON object; `hop.tool_call_id`, `hop.tool_name`, `hop.error_code` (empty or the refusal the text names). The call's context rides on. |
| `recall` | the memory | the collector's ask with its hop keys unchanged and a question built from the wall: `[topic: …; mentioned: …; refers to: …] <the person's words>`, at most `recall_budget` characters, the person's words never cut. A gap's own ask carries the same keys, the gap as its question and `hop.gap_ask`, which the parent's edge lifts into context. |

A gap's find enters the wall of the session's next round, ahead of its first turn, as a `memory_recall` pair whose result opens with `[addendum to your last answer -- looked up after it, for: <gap>]` -- `./push` hands the pair to `./intake`, the wall's one writer, on the internal lane `in_addendum`; its marks are `gap` (the signal), `addendum` (the find, kept) and `addendum_done` (shown, or why not). A find the window already carries is not shown twice, and one for a session that has ended stays unshown.

The model behind the hive orders its system part with `history` after `identity`, `persona` and `instructions` (`system_order`): the summary changes only on a rebuild and stands last.

**The handover (`./handover`, GH #896).** A closed session (`in_close`) leaves a note: its turns go through `./summarizer`, off the hot path, and the answer is kept as a block of kind `summary` with a `marks` row `handover` (state `prepared`); a session that left no turn leaves nothing. The first call of a session this window did not serve last goes from `./policy` through `./handover` once, without a model: a NEW session (no wall row before this round) gets the leaf `history.handover` -- the note of the session served last, when it closed, and the topic still open (the `name` of the newest `topic` mark that names one, unless an `end` came after it) -- and the call returns to `./policy` (`handover-done`), which reads its window as for any call: the plan's cover, the rows it holds, every pin. The leaf carries no pins, because the window shows every valid pin already. It stands for the session and falls at the first rebuild after it. A renewed live session (`in_renewed`) sees no system part of this hive, so its block carries it all: the newest `renew_rounds` rounds as text, the summary, the pins, the open commitments (pins with `source` `commitment`) and the topic, at once and without a model. Open commitments stay empty until a hive pins them through `in_pin`; no parent draws that door yet. Every block is at most `handover_chars` long, cut by rank: pins and commitments, then the topic, then the turns, the summary last. The pack family `handover` is the identity pusher's and is never touched; this leaf is `history.handover`, owner `curator`.

## The window per role

`./policy` `role` picks the presets; every knob set on the instance wins. The window is the system part (slots, `pinned.*`, stubs) + the wall after the plan's cover, each block as the plan shows it + the running round. A rebuild (cold cache, or `tokens_prompt` over `compress_at` of the usable window = the model's, at most `quality_cap`) makes the plan: segments by `horizon`, their form by `tiers` (`raw`, `summary`, `none`), the newest `keep_recent` rounds and the newest round always raw, the model's pins raw. Over `rebuild_to` of the usable window it shrinks old tool results (outside `keep_rounds`), then stubs tools unused for `stub_tools_after` rounds, then gives the summary up. Between two rebuilds only the end of the window moves.

| Knob | none | `talky` | `consult` | `coding` | `research` |
|---|---|---|---|---|---|
| `compress_at` / `rebuild_to` | 0.5 / 0 | 0.5 / 0.35 | 0.5 / 0.35 | 0.4 / 0.25 | 0.4 / 0.25 |
| `quality_cap` | 0 | 120000 | 80000 | 70000 | 80000 |
| `keep_recent` | 12 | 40 | 10 | 20 | 10 |
| `horizon` | `all` | `day` | `task` | `task` | `task` |
| `tiers` | raw,summary | raw,summary,none | raw,summary | raw,summary | raw,none |
| `summary_budget` | 4000 | 4000 | 3000 | 3000 | 2000 |
| `keep_rounds` / `stub_tools_after` | 0 / 0 | 2 / 20 | 2 / 10 | 3 / 20 | 1 / 10 |
| `broadcast_mode` | tail | tail | tail | tail | tail |
| `recall_push` / `recall_budget` | off / 0 | "1" / 200 | off / 0 | off / 0 | off / 0 |
| `short_ids` | off | "1" | "1" | "1" | "1" |

**Short ids.** With `short_ids` on, somebody else's words and a tool result begin with `[#<12 hex>] `, the first 12 hex digits of the wall block's hash; the model's own answers and tool calls never do. The model names ids in its `window` section: `release` shows the block as `[#<id> released — history_read("#<id>")]` from the next rebuild on, `pin` keeps it raw below the cover. Identity, persona, instructions and the round the section was said in stay; an unknown id is ignored and said. A block with `valid_until` passed at the plan's time shows as `[#<id> expired]`. The ledger keeps every block byte for byte; a form the window shows is a block of kind `view`.

## Knobs

| Cell | Knob | Default | Meaning |
|---|---|---|---|
| `policy` | `role` | "" | the presets (table above); empty = the window of 1.0.0 |
| `policy` | `keep_recent`, `compress_at`, `rebuild_to`, `quality_cap`, `horizon`, `tiers`, `summary_budget`, `keep_rounds`, `stub_tools_after`, `broadcast_mode`, `recall_push`, `recall_budget`, `short_ids` | null | a set value wins over the role's preset |
| `policy` | `summary_chars` | null | the 1.0.0 name of `summary_budget`, read when that is not set |
| `policy` | `context_window` | 0 | the window in tokens when the tap reports none; 0 = unknown |
| `policy` | `sidecar_max_chars` | 6000 | an earlier final answer shows its sidecar block up to this length, else it is shown without |
| `writer` | `turn_write` | "1" | "0" or empty: no `turn_write` |
| `handover` | `handover_chars` | 3000 | the longest handover block in characters, without its one-line head |
| `handover` | `renew_rounds` | 12 | rounds a renewed live session is told as text |
| `intake` | `nothing_block` | "" | one JSON object: the sidecar nothing-form rendered after a sentence the model says beside a tool call, when it carries no block; empty = off |
| `intake` | `pass_sections` | "" | sections that leave again unchanged on `sidecar` after their mark, comma-separated (a talky: `memory`) |
| `history` | `read_budget` | 40000 | the most characters (the blocks' canonical JSON) one `history_read` answer carries; a larger read is refused with `too_large` and its size, never shortened |
| `history` | `scan_budget` | 5000 | the most wall rows one `history_search` reads, newest first (and one outline); a search cut there says `truncated_scan` with `cut_by` `scan_budget`, a turn range with more rows is refused with `too_large` |
| `history` | `time_budget_ms` | 3000 | the most time one `history_search` spends from the call on; a match past it is interrupted and the search answers with its hits so far (`cut_by` `time_budget`); below the cell's `external_timeout_ms` |
| `history` | `page_rows` | 250 | the wall rows one page of a search reads per ledger round trip; the search stops at the page that fills its `limit` |

Numeric knobs take numeric strings; `null` or empty means the default.

## Ledger (`./ledger`, all times UTC)

| Table | Columns | Rule |
|---|---|---|
| `blocks` | `hash`, `kind`, `chars`, `body`, `first_seen` | one row per `hash` = sha256 of the element's canonical JSON (keys sorted, UTF-8, no whitespace), short id = first 12 hex digits; `kind` one of `system user assistant peer advice delegation tool_call tool_result recall brief summary pin view` (`view`: a window form of a wall block -- its short id in front, a one-line form -- put by `./policy` so `call_blocks` names only what the ledger holds; never in the wall), reserved `ref` |
| `wall` | `seq`, `session_id`, `turn_id`, `iter`, `kind`, `hash`, `nth`, `final`, `episode_idx`, `at` | append-only; one row per (`session_id`, `turn_id`, `hash`, `nth`), `nth` = earlier copies of the hash in the round; `seq` minted by `./intake`, microseconds since the epoch, strictly increasing |
| `calls` | `call_id`, `session_id`, `turn_id`, `iter`, `trigger`, `started_at`, `model`, `tokens_prompt`, `tokens_completion`, `tokens_cached`, `tokens_cache_write`, `cost`, `cache_expires_at`, `system_hash`, `actions` | one row per call, written before it leaves; the usage follows with the tap (`cost` as JSON number text); `actions` names `system:<families>`, `rebuild:<reason>`, `summary:<id>`, `rebuild_failed` |
| `call_blocks` | `call_id`, `pos`, `hash` | the ordered blocks of exactly what the model had: every system leaf first (by path), then the messages |
| `summaries` | `id`, `covers_to_seq`, `hash`, `sources`, `model`, `at` | the newest counts; `sources` the covered hashes in wall order, the previous summary first |
| `slots` | `path`, `hash`, `owner`, `at` | the system part: `owner` `pack`, `collector` (wins at its own leaf paths) or `curator` (`history.summary`, `history.handover`, and `instructions.hygiene` -- the prompt hygiene `./push` writes where it pushes, its sentence about ids only with `short_ids` on, GH #895) |
| `state` | `key`, `value` | `last_call`, `armed_call` (emptied when its order strikes), `system_hash_sent`, `rebuild_running`, `actions_pending`, `window_plan` (the plan of the last rebuild: `cover`, `keep`, `released`, `shrunk`, `stubs`, `summary`, `sum_from`, `sum_to`, `marks_to`, `as_of_ms`, `at`), `context_window` (the last one the model reported), `handover_for` (the session the window served last, GH #896), parked payloads `pending:<id>` and `pending:handover:<session>`; `push:tier`, the memory tier of the ambient ask (a gap is looked up at it, and only where it exists) |
| `marks` | `seq`, `session_id`, `turn_id`, `kind`, `value`, `at` | what was said about the window, append-only; `seq` microseconds since the epoch, monotonic in the writing cell only (no key: readers order by `seq`, a writer reads before it writes); `kind` `topic` (value the canonical JSON `{"movement":"start|continue|end","name":"…"}`, `name` "" when the section names none), `release`, `pin` (value a 12-hex id), `gap`, `addendum`, `addendum_done` (`./push`: the canonical JSON of `{id, text, engine, searched}`, `{id, call, result, gap}`, `{id, delivered, why}`), `reread`, `handover` |
| `pins` | `hash`, `source`, `until`, `at` | one row per pinned block: `source` the pinning hive, or `model` for a block the model pinned (a wall block, kept raw until released); `until` RFC 3339 or empty; `at` the arrival (a `tail` pin stands there until the next rebuild) |

## The history tools (`./history`, GH #893)

The model reads its own wall, deterministically and out of this hive's ledger alone: no model, no embedding, and never another model's wall. The schemas are the hive's menu answer (`./schemas` `CURATOR_OFFER`); the answer is a JSON object in the text of one `tool_result`.

| Tool | Arguments | Answer |
|---|---|---|
| `history_search` | `query`; `mode` `phrase` (default: the words in order, any case, any spacing), `exact` (case-sensitive) or `regex` (at most 200 characters); `since`/`until` (ISO-8601, a bare `until` date is the whole day); `kinds[]`; `limit` 1-20 (8); `context` 0-2 (0) | `hits[]` newest first: `id` (`#<12 hex>`), `seq`, `session_id`, `turn_id`, `at`, `kind`, `excerpt` (120 characters either side of the match), and with `context` the neighbours of the same session as `before`/`after`; `total_hits` (of the rows scanned), `scanned`, `stopped_at_limit` (the page that filled `limit` was the last one read), `truncated_scan` and `cut_by` (`scan_budget` or `time_budget`) |
| `history_read` | `id` (`#<12 hex>`, bracketed, bare or the full hash) **or** `from_turn`/`to_turn` | `blocks[]` in wall order, each the stored element whole (`block`) with its row and `released`; `size`; for a range `skipped_history` |
| `history_outline` | `since` | `sessions[]` oldest first: `first_turn`, `last_turn`, `first_at`, `last_at`, `turns`, and `topics[]` (`name`, `from_turn`, `to_turn`, `open`) out of the `topic` marks (OR-KY-71: the open topic is the name of the newest mark with one and no later `end`, a nameless mark belongs to it; `from_turn` null = begun earlier); the newest 100 sessions of the newest `scan_budget` rows, `omitted_sessions` the rest, `truncated_scan` |

A refusal is an answer under the same call id, its code in the text (`error`) and in `hop.error_code`: `bad_arguments`, `bad_pattern`, `bad_range` (`since` after `until`, a turn range that runs backwards), `not_found` (an id or a turn this wall never carried), `ambiguous` (an id prefix that names two blocks; `candidates` lists them), `too_large` (`size`, `budget`; a range with more rows than `scan_budget` `block_count_at_least`, one whose raw text is over ten budgets `raw_size`, both refused before a body is read), `unknown_tool`, `ledger_error`. The model's own `history_*` calls and their answers are on the wall like every round, but no hits, no neighbours and no blocks of a range; by its id each is read whole. A read of a block TRIM released writes one `marks` row `reread` (the block's hash, the reading turn), once per block and turn; released is what the `release` and `pin` marks leave in `seq` order.

## Pure functions

`./policy` defines `history_window(rows, params)` (the window between two rebuilds; short ids as the params' role says), `window_plan(rows, topic, trim, pins, prev, as_of_ms, slots, kept, cw)` (the plan a rebuild makes) and the constants `ROLES`, `KEEP_RECENT`, `COMPRESS_AT`, `REBUILD_TO`, `QUALITY_CAP`, `HORIZON`, `TIERS`, `SUMMARY_BUDGET` (= `SUMMARY_CHARS`), `KEEP_ROUNDS`, `STUB_TOOLS_AFTER`, `BROADCAST_MODE`, `RECALL_PUSH`, `RECALL_BUDGET`, `SHORT_IDS`, `SIDECAR_MAX_CHARS` at the top level, without I/O, loadable with `ast` like the collector's. `./schemas` holds `CURATOR_OFFER`: one list, an entry with `name` is a tool, one with `section` a sidecar section; `HISTORY_SCHEMAS` before it is a pure literal as well (the three history tools, spliced into the list). `./history` names what it reads of a call in `ARGS`.
