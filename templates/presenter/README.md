# `presenter@1.2.2`

When the decider is sure that something on the screen helps with what was just said,
something is shown. Every turn with text becomes **one** call to a fast decider over the
topics the installed apps declare; there are no trigger words. A sure verdict opens the
topic's window with a working hint and, in the same output, asks the owning app for its
data. Blocks then appear as their data arrives. Unsure, `none`, an error or the deadline:
the screen stays as it is and the answer stays text.

```text
presenter/          hive, an app (tag `app`): screen out [view, withdraw], back [event, receipt],
                    listens [turn, mutation_committed]
  stage             code, resident, max_concurrency 1 -- the one decision maker
  decide            llm, provider "decisions" -- one call per turn, one `decision` back
  store             store -- tables shows, pending, journal, work
  clock             timer -- one one-shot `deadline` at a time
```

## The contract

An app takes part by declaring `shows` in its app block (the builder draws the edges for
`in_show`, `show_topics` and `show_data`). On every `mutation_committed` the presenter sends
`in_show {op: "topics"}`; the app answers `show_topics {topics: [...]}`, one manifest entry per
topic. The owner of an answer is `hop.show_app` (with `hop.show_at`), stamped by the edge
that carried it, never the body; an answer with an empty `show_app` is the "no app" default
edge and writes nothing:

```json
{"topic": "sample", "title": "Sample", "describe": "what the decider reads about it",
 "glyph": "S", "threshold": 0.7, "data_wait_ms": 4000, "standard": "brief",
 "candidates": [
   {"key": "brief", "block": "display-card", "describe": "a short summary",
    "set": "brief", "bind": {"title": "brief.title", "body": "brief.body"}},
   {"key": "rows", "block": "display-list", "describe": "the items as a list",
    "set": "rows", "bind": {"title": "=Items"},
    "children": [{"each": "rows", "block": "display-item",
                  "bind": {"k": "$.name", "v": "$.note"}}]},
   {"key": "slow", "block": "display-value", "describe": "a number that is costly to fetch",
    "set": "slow", "on_choice": true, "bind": {"value": "slow.n"}}]}
```

`describe` of the topic is what the decider reads -- there is no list of words. `topic` and
`key` are `[a-z0-9_-]` without a dot. One `standard` and at most five candidates; a candidate
carries only `key`, `block`, `describe`, `set`, `bind`, `children`, `on_choice` and (in the
presenter's own topics) `source` -- any other key refuses the topic; `block` is a
catalogue component with `block: true`, and every bound prop is a prop of it. **bind** is a
closed vocabulary: `"<prop>": "<set>.<field>"` reads `value.<field>` of the candidate's own
`set`, `"=<literal>"` is a literal, and the children under `children` (a list) repeat their
`block` once per row of the set named in `each` (again the candidate's own `set`), with
`"$.<field>"` reading the row. A raw (`html`) prop is bound only from a set's `value`, never
from a row and never as a literal. A topic that breaks one of these rules, a topic another app
already holds, and topics beyond 31 (alphabetically last) are refused with `in_show {op:
"refused", topic, reason}` to the app and get no row.

After a sure verdict the owning app receives `in_show {op: "data", topic, request, lead, also,
turn_id, sets, screen_audience}` (`screen_audience` is the stage's param as a sorted list, empty
when the screen has no round; apps that choose between rows of different rounds must only choose
among rows whose round covers screen_audience) (`request` is the turn's text, `sets` every set without `on_choice` plus those
of the chosen candidates) and answers -- in one part or several -- `show_data {topic, turn_id,
sets: {<name>: {value?: {...}, rows?: [{..., audience_set}], audience_set}}}`. A set counts only
when its `audience_set` covers the screen's round (`screen_audience`: `*` in the set, or every
member of the round; an empty round sees `*` data only); a set without an `audience_set`, or
with one that does not cover the round, shows nothing at all and counts as not delivered. A row with its OWN `audience_set`
is gated by it alone; a row without one inherits the set's. A set left with no row the screen
may see and no `value` counts as not delivered, exactly like a set that never came: it is not
kept, its lead waits for `data_wait_ms` and the window is withdrawn with `no_data` -- an empty
block, or a standard standing at once, would tell the screen that rows of other rounds exist. A `value` derived from rows carries
the intersection of their rounds as the set's `audience_set`. Data counts only from the app that
owns the topic (`hop.show_app`); any other answer is refused with an `error` (`foreign_data`). A
turn is done once every chosen block stands or `data_wait_ms` passes; then its words and data
leave the presenter's store. A newer turn of the same topic replaces the older one: the older
turn's deadline and late data no longer touch the window.

The decider's call is `{messages: [], decide: {state, questions}}` with the turn's text as `state` and
nothing else; the questions are `topic` (choice over every topic plus `none`) and per topic
`<topic>.lead` and `<topic>.also` (choices over its candidates; `also` with `none`; a topic with one candidate gets no `lead` question -- a choice needs two options -- and its standard leads). The window
is the view `show-<topic>`: a `display-pane` on the canvas (`topic show:<topic>`, context
`conversation`, relevance `0.8`, the turn's `turn_id`, `touched`), first with
`display-status kind working`, then with the lead -- or the topic's standard where the lead
cannot be placed -- and the also. A placed block is never taken back; with no placeable block
after `data_wait_ms` the view is withdrawn.

## Params of `stage`

| Param | Default | Meaning |
|---|---|---|
| `budget_ms` | 1000 | the verdict's deadline from the turn's arrival; later verdicts change nothing (`late`) |
| `threshold` | 0.7 | confidence a topic needs; a manifest `threshold` wins |
| `data_wait_ms` | 4000 | how long an open window waits for a placeable block; a manifest value wins |
| `also_threshold` | 0.5 | confidence the second block needs |
| `screen_audience` | `[]` | the screen's round (canonical list) |
| `work_hint` | `Working on it…` | the hint's text |
| `catalog` | the display's block copy | written by `scripts/display_sync.py` only |
| `observed_topics` | `["search", "work"]` | the presenter's own observed topics offered to the decider |
| `test_patterns` | a list of common test commands | substrings that make a bash command a test run |
| `builtin_topics` | the residents' topics | the presenter's own topics with declared sources |

The built-in topics `digest` and `research` (GH #976) show the last result of the daily
digest (standard `last`, a `display-card`; variant `recent`, a list) and of the research
assistant (standard `answers`, a `display-list`; variant `latest`, a card), read with op
`last` on the residents' road. Every row of those answers carries the round it was made
for, and a `value` that carries its own `audience_set` (the card's newest row) is gated by
it like a row: a screen sees a result only when that result's round covers it, and a set
of the residents' road never reaches a screen wider than the member's round.

The decider is `decide` (`PRESENTER_DECIDE_MODEL`, `PRESENTER_DECIDE_BASE_URL`, e.g.
`https://openrouter.ai/api`, `PRESENTER_DECIDE_API_KEY` or a `credential_grant_id`). Empty
model: every turn ends as `no_selector` and nothing is shown. A model registry can push a package through the
`in_model` door straight onto `decide`; as shipped no such edge is drawn, and a refused push
leaves as `model_refused`. The door drops the hive's inner context, and a refusal that does not
name this decider (a push for another cell, or one without `subscriber`) leaves as `error`
`decide_refused`, never as a dead letter inside the presenter.

**The decider's key can be a grant** ([#976](https://github.com/mmeyerlein/meclaw/issues/976)),
exactly as on the talky and cogny brains. `decide` carries `credential_grant_id` (empty as
shipped, which is no grant) and `credential_wait_ms` (30000). Installed with a grant and an
**empty** `api_key` -- the empty key is the switch, a key in the config counts as a credential --
the first turn is parked, the decider sends `credential_request` with the grant handle and a
fresh recipient key, and the member's own `access` answers `in_sealed` with the sealed box,
which is opened in RAM and written nowhere. The rim names `./decide` as the connect point of
both lanes (`at`), so the member draws them as v-lanes straight between `./access` and
`./apps/presenter/decide`; the decider's own exits never carry a `credential_request` into
`stage`. The request is the one emission of `decide` without a `finish_reason` (it
comes before any model call): the contract keeps `finish_reason` required and names that one
route in `optional_on_route`, so a decision that lost it still breaks the contract. A round that does not come back in `credential_wait_ms` ends every parked turn on
`credential_pending`, which reaches `stage` as an error verdict: nothing is shown. The builder's
`install_app` renders the whole road from one `credential` object in the wish
(`{"cred_ref", "subject", "expires_at"}`): both params on `decide`, the two v-lanes with the
requester `app:presenter/decide`, and the grant with its birth event through `seed_rows`.

## Its own topics: tool results are data

Since 1.1 the presenter observes the tools of the generation it is installed for: its app
block declares `observes_tool_calls` (`web_search`, `web_fetch`, `bash`, `file`, `edit` --
the occupants of a generation's own `./tools`) and the string form of
`observes_tool_results`, both onto `./stage`. Installing it therefore needs a `generation`.
A call or a result is **data and nothing else**: it opens no window and asks the decider
nothing. Every observed call becomes one row of the table `work` (tool, a short summary, the
path of a file tool, its state and exit code, the call's round; at most 50 rows per session,
the oldest leaves -- a view, the colony log stays the record); a result fills in its call's
state, and a result whose call was not observed is dropped. A call belongs to the turn its
`turn_id` names, else to the newest open turn of the same round.

Why the string form of `observes_tool_results` and not the object form: the string form
hears the two producers of the generation -- its `./tools` and the member's
`./memory-hive` -- and never another app, whose result could carry a round that app wrote
itself; the round of a hit is taken only from a result of the generation. The
`./memory-hive` edge of the string form is not bounded to the generation (it answers every
generation of the member); the presenter binds it anyway, because a result counts only when
its call id is one the presenter saw leave this generation's surfaces
(`observes_tool_calls` is drawn from them alone). The object form works too since the tool
hive and the memory stamp `hop.tool_name` on every result; it is not used here.

Two topics of its own are offered to the decider beside the apps' (`observed_topics`; an app
may not hold their names):

- `search` -- "results of a web search the assistant ran for this request". The hits of the
  turn's `web_search` results (title, site, snippet; links only `http(s)`), stamped with the
  round the result carries; a result without one shows nowhere. Candidates `results` (the
  standard, a list of at most six), `top` (a card of the first hit) and `sources` (a table of
  sites). `data_wait_ms` 15000: the result comes seconds after the verdict, the hint bridges.
- `work` -- "what the assistant is working on right now". The rows of the turn's own
  session (the session its calls ran in, else the newest in the turn's round) the screen may
  see: `steps` (the standard, at most twelve), `files` (the paths touched) and
  `tests` (the verdict of the last bash command that matched `test_patterns`). A shell
  command is named only "test run" or "shell command"; no file content and no command
  output ever reaches the screen.

A topic of `builtin_topics` may give a candidate a **source** instead of an app (an app's
topic may not): `"source": {"read": "<resident>", "hop": {...}, "body": {...}, "rows":
"<path>", "value": "<path>"}`. The app block declares `reads_residents` (all nine residents
a source may name -- the member's seven plus the daily digest and the research assistant, GH
#976), so the builder draws the road and installing needs the member's person
too. After a sure verdict `stage` sends one `resident_read` per wanted set: `hop.resident`,
`hop.op_id` (`<turn_id>/<set>`) and the source's hop keys (`op`, `recall_query`,
`memory_tier`), with the source's body; a string that is exactly `$request` becomes the turn's
text. The answer comes back as `resident_answer` with `hop.resident_status` and
`hop.resident_round`, the round the builder's edge asked in. `rows`/`value` are dotted paths
into it (digits index a list, a string met mid-path is read as JSON, `""` is the whole body).
The set's round is `resident_round` and nothing in the body: without it the set shows
nothing. A refusal or an error is a set without data.

**The model registry** announces the decider as `{cell_path: "<presenter>/decide",
start_model: "", requirement: <decide's requirement>, protocol: "decisions"}`: born without a
model, filled from the registry's decisions rows and pushed through `in_model`. The shipped
decisions rows name no `base_url`, so a push leaves `decide` on the endpoint its set gives it
(`PRESENTER_DECIDE_BASE_URL`). A row that does name one is followed only to an origin listed
in `decide`'s `base_url_allow`, which ships empty (the template names no provider): the set
lists that endpoint there (`override_params`) or sets `PRESENTER_DECIDE_BASE_URL` to it; any
other endpoint is refused (`model_refused`) and the registry records the refusal.

## The journal

One row per turn that asked: `turn_id, topic, p, lead, also, fallback, late, t_verdict_ms,
t_window_ms, t_content_ms, model, at, audience_set, missing`. `fallback` is one of `none` (the lead
stands, or the one block of a topic with one candidate), `unsure`, `no_topic`, `timeout`, `error`, `no_selector`, `invalid` (the standard
stands instead of the lead) and `no_data` (withdrawn). The three times count from the turn's
arrival at `stage`: to the verdict, to the emission of the window, to the first block. The
row never carries the turn's text. `missing` is the JSON list of question keys the decider
left unanswered (`[]` for a whole verdict): a missing `topic` is an `error` (nothing opens), a
missing `<topic>.lead` lets the standard lead (`invalid`), a missing `<topic>.also` places no
also, and another topic's missing questions change nothing.
