# `lead@3.0.0`

The switch of a composite that chooses between targets, as a hive of its own (GH #1474, R-58-9 Nr. 5). For each call of
the curator beside it the lead names the way the call leaves on, its target, the target it falls back to, whether it
leaves the composite as an errand; it keeps the pending errand
and the turn the quick voice gave up in its own store, and decides and orders the early interrupt of a delta stamp. Until GH #1474
the two cells sat inside the curator (`curator/lead`, `curator/lead_state`, GH #1447), so every composite with a curator
carried a model choice; now the curator holds none, and a composite without a choice (cogny, simple-agent) runs its
curator without a lead and names its target at the curate (`hop.target`).

The lead names no target by itself. Every target and errand class is a knob of the composite (`path_routes`,
`departures`, `fallback`, `bridge_target`), shipped empty: without a path, with an unknown one or with
`path_routes` empty the verdict's `target` stays empty and the curator takes the curate's `hop.target`.

## Lanes (into the hive path, on `hop.route`)

| Lane | From | Carries |
|---|---|---|
| `lead_ask` | the curator's `./policy`, through the composite | the question of one call, sent in the same emission as the curator's ledger reads: `ask` the route stamp (`path`, `needs`, `families`) and the turn's head (call, turn, iteration, the consult it answers, the scope, the folded tools, `live` -- the handle of the running session the turn belongs to, "" without one; `event` -- the kind of an event round, today only `delegation`, read off the round's structure (its event key and the `delegation_id` at the seam, never its words), absent on a turn's own round, GH #1504; `text_norm` -- the person's words of the curate, normalised as the route stamps a delta, GH #1529); with `read: "errand"` the rebuild's bundle, given back unread with the session's errand state. Answered by one `lead_verdict` |
| `lead_note` | the curator's `./policy`, through the composite | a note of what only the window saw (`note`: an escalation the wall showed, an errand the model started, a round a side call's condition held for, the end of a turn and of its round, the delta stamp of a voice turn -- kind `delta_stamp`, on which a crisis value orders the early interrupt on `in_feed`); it lands in `./state`, no answer |

## Routes (out of the hive path, on `hop.route`)

| Route | To | Carries |
|---|---|---|
| `lead_verdict` | the curator, as `in_lead` (the composite's edge sets the lane) | the verdict of one `lead_ask`, `hop.call` the call it answers, `hop.handle_ms` the lead's own time: `verdict`, `fwd`/`errand` (the rebuild's bundle and the errand state), `again` |
| `in_feed` | the curator's own lane `in_feed`, passed on unchanged (D58 moveout, GH #1476) | an order of the lead -- the early interrupt: `hop.section` `interrupt`, `hop.once` `interrupt`, `hop.sender` `lead`, `hop.live`, `hop.session_id`, `hop.turn_id`, `hop.audience_set`, the question as the assistant's text, `mark {delta_n}`. The curator sends it once per turn and books it (its README, "A feed on order") |

The rim deletes the station keys of the lead (`cur_origin`, `cur_phase`, `cur_call`, `cur_reason`) on the way out. The
curator's station uses the same names, so without the deletion the verdict would carry the lead's phase into the
curator.

## Wiring

Inside the hive (`config.json` `params.graph`): `. -> ./lead` on the two lanes, `./lead -> ./state` on `lstore`
(the station keys set from the hop), `./state -> ./lead` on `cur_origin` `lead`, `./lead -> .` on `lead_verdict` and
on `in_feed` (the lead's orders, D58 moveout).

A composite that wires a lead draws, beside its curator, `./curator -> ./lead` on `lead_ask` and
`lead_note`, `./lead -> ./curator` on `lead_verdict` setting the route `in_lead` and `./lead -> ./curator` on `in_feed`
(passed on as it is), and sets the curator's `./policy` `lead_wired` "1" -- and `feed_in` "1" when the lead orders
(`interrupt_on_delta`): the curator takes no order on a shut lane; it sets its targets at the lead
(`override_params.lead`). A composite that sets
`lead_wired` "1" without these edges is a broken composite: the curator asks and nobody answers. Each question crosses
two rims each way, so it costs four routing hops more than inside one hive (two per direction) and no run of a cell.

## The lead (`./lead`, `./state`)

`./lead` is a warm code cell with one slot (`max_concurrency: 1`); `./state` is its own store, and nobody but the lead
speaks to it. Per call the curator's `./policy` sends `lead_ask` in the same emission as its ledger reads; the lead
reads its rows, decides `lead_verdict(stamp, head, state)` -- a pure function of its input -- and answers with the
verdict. The curator builds the window once its ledger legs and the verdict are both in, whichever comes first; the
call's `lead` mark carries the lead's `handle_ms`. Events only the window sees -- the quick voice gave the turn up, the
speaking model started a consult, a delta stamp -- come as
`lead_note`. The lead never reads the curator's ledger or wall.

The verdict: `target`, `fallback`, `departs` (`{class, wait_ms}` of an errand that leaves the composite, empty
otherwise), `side`, `reason`, `spec`, `escalate` and the target it fell `from`,
`speak_check`, `pending`, `pending_end` (the lead's own: which consult mark its store drops), `deny` (`spec`, `spec_row`,
`spec_held` and what the window was built on beside them went with the speculation, GH #1584;
`interrupt_on_delta` no more since D58 moveout -- the curator reads nothing of the interrupt). Since 1.0.0 the verdict carries no `spec_allow` and no `wait_ms` of its own: neither had a
reader, and the wait of an errand travels in `departs.wait_ms`. Nor does it carry `live` (D58 live, GH #1482): the
question brings the handle of the running session, and the curator keeps the turn's handle itself. `listen` (GH #1504)
stands in the verdict only when it is true: the turn is only heard (`live_turn`, below); a verdict of before gains no
field.

**The side calls (D58 moveout, GH #1476).** `side` lists the calls that go beside the main call, each
`{when, target, window, text, fill?, first, fallback?}`. The lead decides whether one goes, to which target and with which text;
the curator only evaluates the condition and builds the window. `when` is `external` (the main call leaves the
composite: an errand of `departures`) or `must_call` (the curator's cut gives the call its required tool round -- only
the cut knows the tools a call has open, so the lead names the entry on a fast target's first iteration of a turn whose
stamp needs a `tool`, and the curator sends it only when the round is required). An errand names no `must_call` entry,
so one side call goes beside a call, as before the move. `target` is `bridge_target`'s -- a verdict that names none
sends no bridge (OR-d58-2.target.71): without it the lead names no `side` entry; `window` is `turn` (the person's turn
alone) for the forms `tool` and `soon`, `wall` (the target's window) for `later`; `text` is the form's
`bridge_texts` with the class's wait put in; `fill` (the tool round only) carries `bridge_places`; `first` is true on
`must_call` -- the side call and the required call go to the same voice, one `handle()` at a time, so the sentence
leaves first; `slot` (from `side_slot`) is the slot the text stands under when the target has reported no profile
yet; `fallback` (from `bridge_fallback`, only when set) is the target the same side call tries once more when it ends
in an error or its timeout. Before the move this was the curator's bridge (`side_target`, `bridge` in the verdict, the wording in
`curator/policy`). The knob `side_when` (GH #1504) lists the conditions the lead orders a side call for; a condition it
does not list gets no entry in `side`, and everything else of the verdict stays. A composition drops one where its
surface shows the work itself: a turn that needed a tool got two messages in a text chat, the side call's sentence
after 1.6 to 3.3 s and then the answer (20 of 20 turns in the lab). The knob is set per instance of the lead; the lead
tells no channel from another.

**The bridge: what talky's lead makes of a side call (T4-12, GH #1248; moved here from the curator's README by D58 moveout, GH #1476).** A `consult` or `research` errand never leaves the person in silence (R-WP-4, R-TR-27): beside it `./policy` sends the quick voice -- the `target` of the verdict's `side` entry under `external`, from the lead's `bridge_target` -- ONE call on `model_call` with `hop.call_kind` `side` (the composite lifts it into `context.call_kind`, by which `./policy`, `./intake` and the collector know it) and the leaf `instructions.bridge` -- an instruction, never a fixed sentence; the model says it in its own words (R-WP-4 a). On both ways the instruction tells the voice that it is handling the request for the person right now, in its own voice, and that it never refuses the request or says it cannot do something (GH #1258): told only that a backend was working, it once denied in its own name the very request the backend was answering. For `complex` (way `consult`) it says one short bridge sentence and the answer follows in the same turn within about the class's wait (`departures`, R-WP-9); for `research` it names the waiting time (its wait in `departures`) and keeps the conversation going, and the result comes in a later turn as a round with `consult_id` that leaves on `brain`. The bridge calls no tool and carries no consult id; the errand carries its mark as `hop.consult_wait_ms` -- read by no cell, a mark for whoever reads the message log -- and the state row `ask_pending:<session>` keeps it as `wait_ms`. The bridge is a model call of its own with its own `calls` row (`trigger` `side` since D58 moveout, `bridge` before; GH #1260), so its tap books its usage. **The tool round has a bridge too (GH #1260, OR-WG-6).** A `simple` turn whose stamp needs a `tool` sends its quick call with `tool_scope.require` -- in the wave's lab the fast model called its tool in 0 of 5 turns and 7 of 10 probe calls, 10 of 10 only when the call was required, and then said nothing beside it -- so the bridge (the form `tool`, under `must_call`) says the sentence the person hears while the lookup runs, told not to answer yet. It carries `hop.round_open` "1" (`hop.interim` "1" before D58 moveout), which talky's edges lift onto `context.interim` "1" of the quick call and the dispatcher puts on its sentence: an interim answer leaves the round open for the tool result, a final one would end it at the fan-in. The bridge leaves BEFORE the required call (the entry's `first`), on the same edge: both go to the one `./quick`, a stateful `llm` cell that takes its mailbox one call at a time (`max_concurrency` is a knob of stateless cells), so the two cannot run side by side, and in this order the person hears the bridge after one model call, not two. A bridge keeps no copy of its parked turn: its errand answers the turn. Every bridge states no content: its instruction forbids facts, numbers, results and guesses (a lab bridge stated a rain forecast before any lookup); the tool round's says only that it is looking it up, the consult's that the answer is still coming, and research names its wait. The tool round's bridge also names the lookup that runs beside it: the tools its `tool_scope` allows at the ops stage (not the hint stage), a family once, in words: where each looks ("on the web and at the weather service"; a family without words for its place is never named by its tool name -- beside named ones it adds "in other places", alone the sentence names no place), and it is told to write no tool call, JSON, code or command -- told the `code` name of its tool, the fast model wrote the call out as text and made weather up beside it in a probe. And every bridge is told never to speak of missing access, missing abilities or limits and to ask nothing about the request itself (GH #1260, OR-WG-13): a lab bridge, not knowing a tool worked for it, said it had no access to weather data and asked which day, while the `weather` call ran. The tool round's one sentence asks the person nothing at all, the consult's asks nothing about the request itself, and research keeps the conversation going while it waits (R-TR-27) -- on another subject, never on the request: told only that anything else was fine, it asked in a probe what mattered to the person in the request. Its window carries no `tool_call` or `tool_result` of earlier turns; on a consult and on a tool round it is the person's turn alone -- every text of the person since the last message that is not theirs, so a request sent in two messages stays whole, and never a pin standing as a message (`broadcast_mode` tail), which the window can put behind the turn (research keeps the quick window): an earlier answer in the window is a pattern the fast model follows -- in a probe it answered a consult in the bridge itself and said tonight's degrees on a tool round, and with the person's turn alone it did neither. The curator does not see the menu of the quick model: when the channel's scope names tools that menu does not carry, the `llm` cell drops the requirement (its `WARN` line), and the answer of that round comes without its sidecar block, after the bridge's interim sentence.

**The waiting time (GH #1400; moved from `curator/policy` by D58 moveout, GH #1476).** With `horizon_feed` on, a
running session's first call of a turn (its question carries `live`) that leaves the composite or gets
its required tool round is told how long the answer takes: the verdict's `feed` carries ONE entry `{when, section
"horizon", body, mark}` under the side call's condition (`external` with the errand class as the way, `must_call` with
the fast target). The body is numbers only -- `section`, `seconds` (rounded up), `way`, `family`, never words; the
sentence is the voice's `duplex.gpt_live.horizon_text` -- and the curator sends it unchanged with the round's sends;
`mark` `{planned, family}` lands in the curator's mark `feed`. The seconds are the `horizon_quantile` (nearest rank,
default 75, never the median: a wait said too short is worse than one said too long) of the booked rounds of the turn's
family, else `horizon_defaults_ms[family]`, else no entry; several families take the longest, a turn without a family
counts under its way. The bookkeeping is this store's: the verdict keeps its plan as the entry of its turn in `horizon_plan:<session>`
(the call, family, way, seconds); the curator's note `round` (a condition held for the call, its sends went) starts
the turn's clock at the seam once the plan is that call's (the turn's entry in `horizon_turn:<session>`); the note
`turn_end` with `round_end` books the duration under the family in `horizon_rounds` (the newest 20 per family),
takes the turn's entry out and keeps `horizon_last:<session>` `{turn, family, planned, actual}` (seconds). Both tables are `{turn: row}`, so two waiting
rounds of one session that overlap (a tool round while an errand of a turn before runs) are both booked; this store
sweeps nothing, so each keeps the newest 8 turns -- a turn whose round never ends leaves its entry until then. A held
round sends its note only with its sends, so a round that never goes is never booked. Off (the default) nothing of this is read, written or ordered.

**The early interrupt (GH #1404; an order on `in_feed` since D58 moveout, GH #1476).** With `interrupt_on_delta` on,
the first delta stamp of a voice turn whose `choices.risk` value (the route's own judgement; `hop.fallback` empty or
`unsure`, GH #1407) has a question in `interrupt_questions` is ORDERED at once, like any domain orders a feed: one
message on `in_feed` with `section` and `once` from `interrupt_section` (talky: `interrupt`), `sender` `lead`, the handle `live` (the stamp's
hop, else its context), `session_id`, `turn_id` and the round in the head, the question as the assistant's text and
`mark {delta_n}` (the words of the partial). The curator's claim `pending:feed:<live>#<turn>#interrupt` lets ONE
`live_feed` per turn leave (no `hop.at_turn_end`), its mark `feed` names the lead; a later partial of the turn orders
again and the claim keeps it silent. An order without a handle is booked by the curator as not sent
(`feed {reason: no_live}`). A stamp of another fallback (`no_text`, `error`, `late`) is never ordered. Either way the
turn's row `interrupt:<session>#<turn>` says it: the reason (`skip` `fallback:<value>`) or `at`, `delta_n` and
`live` of the order. A row without an order (a fallback, or an order without a handle) gives way to the turn's first
real order, which reads the row first and writes by compare-and-set; a real order's row stays. The turn's first normal
ask (iteration 0) adds `turn_end_at` and `ms_before_turn_end`: how early the order went before the
turn's end -- the order, not the sending; whether the curator sent it (its lane open, its claim taken) is its mark
`feed`. As many runs as before the move: policy ->
lead -> policy -> store -> policy. Before the move the lead handed the finished feed to the curator,
which claimed, sent and booked it (marks `interrupt_skipped`, `interrupt_early`).

**A turn that is only heard (`live_turn`, GH #1504).** In a running session whose own model speaks, that model has
answered the person's turn already; a target the lead named answered it a second time. With `live_turn` `listen` the
verdict for the question of such a turn -- a handle `live`, no event round (neither an `event` nor a key of the form
`<label>~<deadline_ms>~<hex8>`) and no `ask_id` -- names no target: `target` "", `listen: true`, `side` and
`feed` empty, no waiting-time plan, no escalation, `reason` `listen`. The mark of an open
errand (`pending`) runs on. The curator then sends no call (its README, "A turn that is only heard"). An event round
and the round that answers an errand are always called: a delegation or an advice would otherwise stay without an
answer. The stamp and the early interrupt run on (the interrupt is an order on `in_feed`, not a part of the verdict).

**The note of a heard turn (`listen_sections`, GH #1538).** A heard turn calls no target, so no model wrote its section
`memory`: a turn like "my knee has hurt since yesterday" left no wellbeing fact and no followup. `listen_sections`
names a target and a gate over the enum values of the route stamp's choices, `{"target": "<target>", "when":
{"<choice>": ["<value>", ...]}}` as JSON text (type string), for example `{"target": "quick_bridge", "when": {"depth": ["important"], "risk":
["strained", "acute"]}}`. The question carries the stamp's choices (`choices`, the collector's `hop.choices`). When the
turn is heard and the stamp meets EVERY key of the gate with a value the route model judged (`source` `model`, never
its fallback), the verdict keeps `target` "" and names one side entry `{"when": "sections", "target": <target>}`. The
values are the ones the route model judged; the lead reads no word of the turn. The curator sends that entry as the
turn's silent call (its README, "The silent call of a heard turn"). Empty, `0`, null or a value without a target and a
gate: off, as shipped (a malformed value is said on stderr).

**The target of an event round (`event_routes`, GH #1504).** An event round (a delegation) owns no stamp; before, it
went by `*` of `path_routes` or inherited the stamp of its turn. With an entry in `event_routes` for the kind its
question names (`event`), the round goes to that target, whatever stamp it carries or inherited: `reason` `event`,
`departs` empty, no side call and no waiting-time feed -- the session's own model leads the talk while the round
works. One exception (GH #1540): with `horizon_feed` on, the first call of a delegation round gives that model the
waiting-time advise of a turn's own round (condition `delegation`, the routed target as its way), once per delegation,
so the voice that started it does not wait in silence; no other routed event gets one. `fallback` and escalation hold as for every target; the round that answers an errand keeps its target. The
name in `event_routes` is a target the composition wires, never an errand class of `departures`: a routed round never
leaves the composite as an errand. The lead checks no names, it knows no topology; a set that names an unwired target
is a broken set. A kind without an entry goes as before.

| Row in `./state` | What it holds |
|---|---|
| `escalated:<session>` | the turn the quick voice gave up |
| `ask_pending:<session>` | the pending errand (`ask_id`, `until`) |
| `verdict:<session>` | the last verdict, so an ask again of the same call gets the verdict given, never a second one |
| `horizon_plan:<session>`, `horizon_turn:<session>` | tables `{turn: row}`, the newest 8 turns: the waiting-time plan of each turn's call and the clock of its round, until booked (`horizon_feed`) |
| `horizon_rounds` | the booked durations of the rounds per family (ms, the newest 20) |
| `horizon_last:<session>` | what the last booked round of the session was told and took (`planned`, `actual`, seconds) |
| `interrupt:<session>#<turn>` | the turn's early interrupt: the reason none was ordered (`skip`), or when, on how many words and under which handle it was ordered (`at`, `delta_n`, `live`) and how early the order went before the turn's end (`turn_end_at`, `ms_before_turn_end`); a row without an order gives way to the turn's first real order (`interrupt_on_delta`) |

## Knobs

| Cell | Knob | Default | Meaning |
|---|---|---|---|
| `lead` | `path_routes` | "" | D58 target (contract point 9): the target of a turn by the path it was stamped with (`hop.path`, else `context.path`, GH #1247): `path:name` pairs, comma separated. A name is a target the composite wires (the call's `hop.target`, route `model_call`) or an errand class `departures` lists (the call leaves the composite, route `errand`). `*:name` names the target of every other turn: no path, an unknown one (a class the route stamps without its own entry: the lead names no class of its own, GH #1478), the round that answers an errand. Empty (shipped): the lead names no target, the curate's `hop.target` decides. Every call with a path leaves one `route` mark (`{call, path, way}`, `way` the target or the errand's class) |
| `lead` | `departures` | "" | D58 target (contract point 9): the errand classes that leave the composite, `class:wait_ms` or `class:wait_ms:pending_ms` pairs, comma separated. The verdict says `departs` `{class, wait_ms}`; the errand carries `hop.consult_class` and `hop.consult_wait_ms` (the mark the side call says, GH #1248); its pending mark stands `pending_ms` longer (else `ask_pending_ms` alone). Empty: no path leaves the composite |
| `lead` | `fallback` | "" | D58 target (contract point 9): `target:target` pairs, comma separated -- the target a round goes to when the first one gives it up (an unspeakable tool result, a failed answer, `fast_max_iter`, `recall_to_fallback`); the `escalate` mark names both (`from`, `to`). A target with a fallback is a fast one; a verdict without one hands nothing on |
| `lead` | `bridge_target` | "" | D58 target (OR-d58-2.target.14, .38): the target of the side call beside an errand or a tool round (`target` of each `side` entry): one name, or `*:name` pairs (only `*` is read); a verdict that names none sends no bridge (OR-d58-2.target.71) |
| `lead` | `bridge_fallback` | "" | d54s bruecke (GH #1586): the target a side call tries once more when it ends in an error or its own timeout (`fallback` of each `side` entry, left out when it names the entry's own target). The curator sends the same call, same window and text, once to this target under its own `calls` row; if that one fails too, the error goes on to the composite's errors like the first and there is no third try. Never a fixed sentence (R-TR-27). Empty (shipped) = no second try, as before |
| `lead` | `bridge_wording` | "" | D58 moveout (GH #1476; the curator's knob of D58 target, contract point 14, moved here): the form of the side call beside an errand, per class of `departures`: `<class>:<form>` pairs, comma separated. `soon`: the answer follows in this turn within the class's wait, the side call sees the person's turn alone (`window` `turn`); `later`: the result comes in a later turn, the side call keeps its target's window (`window` `wall`). A class the knob does not name speaks `later` |
| `lead` | `bridge_texts` | neutral sentences | D58 moveout (GH #1476, contract point 5): the text of a side call per form, a JSON object `{tool, soon, later}`. The curator puts it as the leaf of the target's side slot (the profile's `instructions.side_slot`, talky `instructions.bridge`). `{seconds}`/`{minutes}` take the class's wait; `{fill}` is the gap the curator fills with the places of the tools the cut left open. A form without a text sends no side call. The shipped sentences carry the measurements of GH #1258 and GH #1260 (one voice, no content, no talk of missing access, the tool round asks nothing); d58-3 namen (GH #1525): the shipped sentences name no speaking surface, a composition sets its own wording |
| `lead` | `bridge_places` | "" | D58 moveout (GH #1476, contract point 1, `fill`): the words of the places a running lookup looks, `{places: {family or tool: words}, rest, before, join}`, carried as `fill` on the tool round's side call. The curator names each open tool once (a family with a row by its family name, the hint stage not counted), adds `rest` beside named ones when some have no row, joins with `join` and puts `before` + the phrase into `{fill}`; nothing named: the gap goes empty; d58-3 namen (GH #1525, N9a): "" = no `fill`, the composition sets the words of its places |
| `lead` | `side_slot` | "" | D58 moveout (GH #1476, orchestrator ruling moveout.15): the `slot` of each `side` entry -- the slot its text stands under when the target has reported no profile yet (the first errand of a colony goes out beside the target's `catalog_ask`); the target's own `instructions.side_slot` wins. Empty = no `slot`: a target without a profile gets no side call; d58-3 namen (GH #1525, N9a): "" = the entry names no slot, the composition sets its surface's slot |
| `lead` | `horizon_feed` | `0` | GH #1400 (moved from `curator/policy`): `1` or true = the waiting-time advise (above): ONE `feed` entry section `horizon` on a running session's first call that leaves or gets its required tool round, and the bookkeeping of its rounds in `./state`; `0` = nothing read, written or ordered (`horizon_advise` until d58-3 namen, GH #1525) |
| `lead` | `horizon_defaults_ms` | `{}` | GH #1400 (moved from `curator/policy`): JSON object family (or way) -> milliseconds, the waiting time of a family before a round of it is booked; `{}` = none until one is |
| `lead` | `horizon_quantile` | `75` | GH #1400 (moved from `curator/policy`): the percentile (nearest rank, 51..100) of a family's booked rounds the advise says; anything else reads as 75 (stderr) |
| `lead` | `errand_tool` | "" | D58 target (contract point 9): the tool a speaking model starts an errand with (GH #1280); denied in the `tool_scope` of every speaking call of the session while an errand stands |
| `lead` | `errand_family` | "" | D58 target: the family of `errand_tool` on a folded menu (`context.tools_folded`), denied with it |
| `lead` | `ask_pending_ms` | 120000 | GH #1247 (renamed by d58-3 namen, GH #1525): how long a pending ask (an errand of `departures`, and since GH #1280 a call of `errand_tool` by the speaking model) stays pending without its answer -- the backstop: the mark ends with the answer (the round under its `ask_id`). The mark `ask_pending:<session>` carries `until` = the ask's stamp + this value, or the class's own `pending_ms` in `departures` (GH #1260), and while it stands every speaking call of the session carries the policy's pending note and denies `errand_tool` in its `tool_scope` (GH #1280); the first call past it deletes the mark (by key and the value read) and the speaking model may ask again. The default is three times the measured answer time (38-42 s, GH #1280; 30000 before, which let the mark lapse while the first ask still ran); null, empty or below 1 = 120000 |
| `lead` | `recall_to_fallback` | `0` | GH #1435 (renamed by d58-3 namen, GH #1525): `1` or true = a turn whose path leaves on a fast target (one `fallback` names) and whose route stamp names a need of `recall_needs` leaves on that target's fallback instead; `reason` and `way_reason` are the need it named; other paths and the answer of an errand change nothing |
| `lead` | `recall_needs` | `[]` | d58-3 namen (GH #1525, O6, R-583-1 form B): the needs of the route stamp that call for the recall, a list (or its JSON text, or a comma string); with `recall_to_fallback` on, a turn of a fast target whose stamp names one leaves on the fallback, `reason` is the name it named. `[]` = no need calls for the recall; the composition sets its names |
| `lead` | `interrupt_on_delta` | `0` | GH #1404: `1` or true = the first delta stamp of a voice turn whose `choices.risk` value (the route's own judgement; `hop.fallback` empty or `unsure`, GH #1407) has a question in `interrupt_questions` orders the interrupt at once on `in_feed` (the curator sends ONE `live_feed` section `interrupt`, no `hop.at_turn_end`, at most one per turn; without a handle `live` it books `feed {reason: no_live}`); another fallback is never ordered, its reason kept in `interrupt:<session>#<turn>`, where the turn's end adds `ms_before_turn_end`. Needs the composite's edge `./lead -> ./curator` on `in_feed` and the curator's `feed_in` "1". `0` = nothing read or written |
| `lead` | `interrupt_questions` | `{}` | GH #1404: JSON object crisis value -> question: its keys ARE the `choices.risk` values that interrupt, the question is what the voice says; an entry without words is said on stderr and left out. `{}` = no value interrupts |
| `lead` | `interrupt_section` | "" | d58-3 namen (GH #1525): the feed section the interrupt is ordered under on `in_feed`, the name the speaking surface reads it by; also the order's `once`. "" = no interrupt is ordered; the composition sets its surface's section |
| `lead` | `fast_max_iter` | `2` | Named `quick_max_iter` before GH #1529. GH #1248 (T4-10): how many iterations past its first call a quick round keeps the quick voice after a tool result |
| `lead` | `side_when` | `must_call,external` | GH #1504: the conditions the lead orders a side call for, comma separated (or a JSON list), out of `must_call` and `external` (above, The side calls); an unlisted condition gets no `side` entry, the rest of the verdict stays. Null, empty or an empty list = both. A name that is neither condition makes the whole value the default, both, and is said on stderr: a typo in a set never changes the behaviour without a word |
| `lead` | `live_turn` | `call` | GH #1504: `listen` = the turn of a running session is only heard (above): no target, `listen: true`, and the curator sends no call; `call` = the turn is called like every turn, as before. Null or empty = `call`; any other word is said on stderr and read as `call` |
| `lead` | `event_routes` | "" | GH #1504: the target of an event round by the kind its question names (`event`, today `delegation`), `kind:target` pairs, comma separated (or a JSON object); a target, never an errand class (above). Empty = every event round goes by its stamp, as before |
| `lead` | `listen_sections` | "" | GH #1538: the silent call of a heard turn (above), `{"target": ..., "when": {choice: [values]}}` as JSON text (type string); the gate is met when every key holds a value the route model judged. Empty, `0` or null = off |
| `lead` | `max_concurrency` | 1 | GH #1447: the one slot of the lead, warm; its state lives in its own store `./state`, never in the child |

Numeric knobs take numeric strings; `null` or empty means the default. talky sets the targets at its lead:
`path_routes` `talk:brain,simple:quick,complex:consult,research:research,*:brain`, `departures`
`consult:10000,research:120000:240000`, `fallback` `quick:brain`, `bridge_target`
`*:quick`, `bridge_wording` `consult:soon,research:later`, `errand_tool` `consult_cogny`,
`errand_family` `consult`. What the ways
and the side calls do with these values is told in the curator's README
([`curator`](../curator/README.md), The ways of a call), where the window is built on them.
