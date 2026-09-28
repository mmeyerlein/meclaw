# `curator@1.0.0`

The window of one model, owned in one place, with a ledger of every call. Contract tables only; the prose follows with the program it belongs to.

## Lanes (into the hive path, on `hop.route`)

| Lane | From | Carries |
|---|---|---|
| `in_curate` | the context collector | `messages[]` the whole running round (opening turn, evidence pairs, every tool iteration so far), never history; `system` the per-round slots; `tool_scope` if set; `hop.session_id`, `hop.turn_id`, `hop.iter` and whatever else the caller puts on the call (no session = `default`, no turn = a round of its own). An `assistant` text in a round without a tool call of its own (a duplex turn pair; the evidence pairs `memory_recall` and `affinity_brief` do not count) is a final answer and an episode. Answered by one `brain`. |
| `in_slots` | the context collector | `system` only (tool menu, sidecar contract). Kept as the collector's slots, sent with the next call whose system changed; no call of its own. |
| `in_llm` | the tap: every output of the model | the raw output with `hop.finish_reason`, `hop.model`, `hop.tokens_*`, `hop.cost`, `hop.cache_expires_at`, `hop.context_window`, and `context.curator_call`, `context.session_id`, `context.turn_id`, `context.iter`. A final answer (`stop`, `length`) enters the wall from here; the sentence beside a tool call (`tool_calls`) enters as an interim answer, not final, never an episode. Without a session it is session `default`, without a turn the round of its call (the rule of `brain`); without `context.curator_call` it is dropped and said on stderr. |
| `in_pack` | an identity pusher | `system.{identity,persona,handover,instructions}` (or `slot` + `content`); the context rides on to `pack_ack` untouched. |
| `in_close` | the session keeper | `context.session_id` of the closed session. |
| `in_model` | the llm-registry | a params-only body for `./summarizer`, only when `hop.subscriber` names it (the one model door of the hive). |

## Routes (out of the hive path, on `hop.route`)

| Route | To | Carries |
|---|---|---|
| `brain` | the model | `messages[]` the window; `system` only when a family changed, each changed family one `$replace` root with all its leaves; `tool_scope` unchanged; the `in_curate` hop unchanged plus `hop.curator_call`. The parent's edge promotes `curator_call`, `turn_id`, `session_id`, `iter` into context. |
| `turn_write` | whoever keeps episodes | one participant turn per message (`user`, the final `assistant` answer without its sidecar block, `peer` with `speaker`/`speaker_ref`); `hop.turn_id` `<session_id>#<index>` (index from 0 per session), `hop.turn_index`, `hop.happened_at`, `hop.session_id`, `hop.iter`, `hop.phase`. |
| `write` | the close pass | `messages[]` the participant turns of the session, `rounds` every other wall row raw (`seq`, `turn_id`, `iter`, `kind`, `hash`, `nth`, `at`, `turn`); `hop.turn_id` `close-<session_id>`, `hop.turn_count`, `hop.round_count`. Empty for a session with no rows. |
| `pack_ack` | the pusher | `hop.pack_owner` (off the envelope), `hop.pack_slots`, `hop.error_code` (empty, `slot_unknown`, `pack_empty`), `hop.pack_unknown`. |
| `model_refused` | the registry | a push `./summarizer` refused, with `hop.refused_subscriber`. |

The model behind the hive orders its system part with `history` after `identity`, `persona` and `instructions` (`system_order`): the summary changes only on a rebuild and stands last.

## Knobs

| Cell | Knob | Default | Meaning |
|---|---|---|---|
| `policy` | `keep_recent` | 12 | rounds (one `session_id` + `turn_id` each) a rebuild leaves untouched |
| `policy` | `compress_at` | 0.5 | share of the context window at which a call's `tokens_prompt` orders a rebuild |
| `policy` | `context_window` | 0 | the window in tokens when the tap reports none; 0 = no compress trigger |
| `policy` | `summary_chars` | 4000 | longest summary accepted; a longer one is refused and the window stays |
| `policy` | `sidecar_max_chars` | 6000 | an earlier final answer shows its sidecar block up to this length, else it is shown without |
| `writer` | `turn_write` | "1" | "0" or empty: no `turn_write` |
| `intake` | `nothing_block` | "" | one JSON object: the sidecar nothing-form rendered after a sentence the model says beside a tool call, when it carries no block; empty = off |

Numeric knobs take numeric strings; `null` or empty means the default.

## Ledger (`./ledger`, all times UTC)

| Table | Columns | Rule |
|---|---|---|
| `blocks` | `hash`, `kind`, `chars`, `body`, `first_seen` | one row per `hash` = sha256 of the element's canonical JSON (keys sorted, UTF-8, no whitespace), short id = first 12 hex digits; `kind` one of `system user assistant peer advice delegation tool_call tool_result recall brief summary`, reserved `ref` `pin` |
| `wall` | `seq`, `session_id`, `turn_id`, `iter`, `kind`, `hash`, `nth`, `final`, `episode_idx`, `at` | append-only; one row per (`session_id`, `turn_id`, `hash`, `nth`), `nth` = earlier copies of the hash in the round; `seq` minted by `./intake`, microseconds since the epoch, strictly increasing |
| `calls` | `call_id`, `session_id`, `turn_id`, `iter`, `trigger`, `started_at`, `model`, `tokens_prompt`, `tokens_completion`, `tokens_cached`, `tokens_cache_write`, `cost`, `cache_expires_at`, `system_hash`, `actions` | one row per call, written before it leaves; the usage follows with the tap (`cost` as JSON number text); `actions` names `system:<families>`, `rebuild:<reason>`, `summary:<id>`, `rebuild_failed` |
| `call_blocks` | `call_id`, `pos`, `hash` | the ordered blocks of exactly what the model had: every system leaf first (by path), then the messages |
| `summaries` | `id`, `covers_to_seq`, `hash`, `sources`, `model`, `at` | the newest counts; `sources` the covered hashes in wall order, the previous summary first |
| `slots` | `path`, `hash`, `owner`, `at` | the system part: `owner` `pack`, `collector` (wins at its own leaf paths) or `curator` (`history.summary`) |
| `state` | `key`, `value` | `last_call`, `armed_call` (emptied when its order strikes), `system_hash_sent`, `rebuild_running`, `actions_pending`, parked payloads `pending:<id>` |

## Pure functions

`./policy` defines `history_window(rows, params)` and the constants `KEEP_RECENT`, `COMPRESS_AT`, `SUMMARY_CHARS`, `SIDECAR_MAX_CHARS` at the top level, without I/O, loadable with `ast` like the collector's.
