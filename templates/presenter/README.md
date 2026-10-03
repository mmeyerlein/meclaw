# `presenter@1.0.0`

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
  store             store -- tables shows, pending, journal
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
`key` are `[a-z0-9_-]` without a dot. One `standard` and at most five candidates; `block` is a
catalogue component with `block: true`, and every bound prop is a prop of it. **bind** is a
closed vocabulary: `"<prop>": "<set>.<field>"` reads `value.<field>` of the candidate's own
`set`, `"=<literal>"` is a literal, and the children under `children` (a list) repeat their
`block` once per row of the set named in `each` (again the candidate's own `set`), with
`"$.<field>"` reading the row. A raw (`html`) prop is bound only from a set's `value`, never
from a row and never as a literal. A topic that breaks one of these rules, a topic another app
already holds, and topics beyond 31 (alphabetically last) are refused with `in_show {op:
"refused", topic, reason}` to the app and get no row.

After a sure verdict the owning app receives `in_show {op: "data", topic, request, lead, also,
turn_id, sets}` (`request` is the turn's text, `sets` every set without `on_choice` plus those
of the chosen candidates) and answers -- in one part or several -- `show_data {topic, turn_id,
sets: {<name>: {value?: {...}, rows?: [{..., audience_set}], audience_set}}}`. A set counts only
when its `audience_set` covers the screen's round (`screen_audience`: `*` in the set, or every
member of the round; an empty round sees `*` data only); a set without an `audience_set`, or
with one that does not cover the round, shows nothing at all. A row with its OWN `audience_set`
is gated by it alone; a row without one inherits the set's. A `value` derived from rows carries
the intersection of their rounds as the set's `audience_set`. Data counts only from the app that
owns the topic (`hop.show_app`); any other answer is refused with an `error` (`foreign_data`). A
turn is done once every chosen block stands or `data_wait_ms` passes; then its words and data
leave the presenter's store. A newer turn of the same topic replaces the older one: the older
turn's deadline and late data no longer touch the window.

The decider's call is `{messages: [], decide: {state, questions}}` with the turn's text as `state` and
nothing else; the questions are `topic` (choice over every topic plus `none`) and per topic
`<topic>.lead` and `<topic>.also` (choices over its candidates; `also` with `none`). The window
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

The decider is `decide` (`PRESENTER_DECIDE_MODEL`, `PRESENTER_DECIDE_BASE_URL`, e.g.
`https://openrouter.ai/api`, `PRESENTER_DECIDE_API_KEY` or a `credential_grant_id`). Empty
model: every turn ends as `no_selector` and nothing is shown. A model registry can push a package through the
`in_model` door straight onto `decide`; as shipped no such edge is drawn, and a refused push
leaves as `model_refused`.

## The journal

One row per turn that asked: `turn_id, topic, p, lead, also, fallback, late, t_verdict_ms,
t_window_ms, t_content_ms, model, at, audience_set`. `fallback` is one of `none` (the lead
stands), `unsure`, `no_topic`, `timeout`, `error`, `no_selector`, `invalid` (the standard
stands instead of the lead) and `no_data` (withdrawn). The three times count from the turn's
arrival at `stage`: to the verdict, to the emission of the window, to the first block. The
row never carries the turn's text.
