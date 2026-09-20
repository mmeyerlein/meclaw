# `display@2.6.0`

> **Normative source:** this README is the public rendering of the display-hive description (`meclaw-next/23-display/display-hive.md`, internal), with its reference model and its scenarios, which travel with this template in `compose/scenarios/`. Where the two differ, that document rules and this README is redrawn from it (`docs/development-rules.md` § 10).

One screen per member, reached at `/<mount>/` on the colony's one listener, that several
agents and applications write onto at the same time. A **view** is a named, owned,
optionally expiring piece of that screen: whoever sends one owns it, replaces it under the
same name, and takes it down again. Three lanes go in. `in_view` puts a view up,
`in_withdraw` takes it down, `in_notice` turns a classified message into a window of its
sender. Two come out: `event` carries what a person did that the screen could not answer
itself, `receipt` a write that was refused. The hive is the address, and the cells inside
it are its own business.

```
in_view / in_withdraw / in_notice  ->  compose  <->  views    the one state
                                         | \
                                         |  +->  judge, clock    relevance, time
                                         v
                                        web  ->  /<mount>/<output>  ->  event / receipt
```

## Principles

1. Intelligence decides meaning and relevance. Deterministic code decides the immediate
   interaction and the rendering, so the model is no event handler.
2. There is one screen state per member, and every output shows that same state, rendered
   by its profile.
3. Focus is the state of the whole screen rather than one highlighted element, and visible
   is only what is of use now.
4. At every real event the situation is judged again: is this relevant, what priority does
   it have, does it support what the member is doing, can it go entirely.
5. Nothing is a placeholder. What is no longer relevant decays together with its tile, and
   an empty seat in the dock is empty space.

The guiding sentence: show as little as possible, but everything that is relevant now.

## The state

There is one screen state per member. It lies in the store `views` as a single row, owned
by `display` under the `view_id` `screen-state` with the kind `state`, and the curator
reads and writes it in a pass. Every value in it is systemwide; no value depends on an
output. The finger writes nothing directly. A tap and a hold reach the curator as events,
and the curator writes what they mean.

The write names the version it read: `update ... where updated_at = <the stamp the pass
got>`, and the first creation is an `insert`. A read pass and its write are a full message
round trip apart, so two events that arrive inside it are handed the same row; without the
condition the second write would erase what the first pass concluded, and a tap would be
lost. When the store reports that no row moved, the same pass runs again on the row that
now stands, at most sixteen times -- twice the fan-out measured on a screen whose seven
applications wrote in one breath.

The curator takes one message at a time (`params.max_concurrency` is 1). Events that
arrive together are therefore read, computed and answered in the order they arrived, and
never four at once. It does not close the round trip above: a pass is two messages with a
store between them, so two events inside one trip still read the same row and the
condition on the write is still what keeps the first of them -- measured on a test colony,
the refused writes under a burst of ten taps are the same with one worker as with four.

**The browsers hear the store, not the pass.** The calls a pass computed for the display
travel on its own state write and are sent from the reply to it: a write that landed draws
them, a write the condition refused draws nothing and runs its pass again. A patch says
what the screen IS, and before this a refused pass had already said it -- the drawing was
then taken back by the next pass that landed, which is what a flicker is. The price is one
message round trip of latency per event.

What this does not reach is a pass that LANDED: an event that arrives while another pass
is in the air is answered first by that pass, whose state is the truth of that instant and
is older than the finger. The client keeps its own drawing until a patch carries a state
computed after the touch (§ 5.7), and that is still what covers this case.

Time points are epoch milliseconds, durations are milliseconds. A hint that is a number
travels as text, because the template language reads `0` as empty.

**What an application writes.**

| Hint | Type | Default | Meaning |
|---|---|---|---|
| `context` | word | unset | what the window belongs to (`conversation`, `ambient`, `system`); the key of the weights |
| `relevance` | 0 to 1 | the class default, else 0.5 | the application's own relevance, a factor of score and rank; clamped at the door |
| `class` | word | unset | the class of a note (`system_error`, `error`, `warning`, `important_note`, `note`); supplies the relevance when `relevance` is missing |
| `pinned` | bool | false | the tile stays in the state as long as the application sends the view. Never the window |
| `relevant_until` | epoch ms | unset | the window is present until then; after it the relevance is capped at 0.2 |
| `touched` | epoch ms | unset | a value greater than the last seen is a touch |
| `topic` | text | unset | what the window is about (`weather:berlin`, `timer:<id>`, `chat`) |
| `layer` | `canvas`, `modal` | `canvas` | the ladder the window competes on |
| `seat` | `bottom` | unset | the tile has a fixed place in the dock |
| `seat_ord` | number | 0 | the order among seats, ascending from the bottom |
| `linger` | ms | the screen's `linger_ms` | how long the window holds full weight after a touch, capped at `linger_ms + fade_ms` |
| `state` | `urgent`, `hidden` | unset | `urgent` demands the rung `urgent`, `hidden` zeroes the score |
| `turn_id` | text | unset | the turn this window came out of |
| `title`, `kicker` | text | unset | props of the window component; both feed the fallback tile |
| `tile` | child | a fallback | `glyph`, `line` (at most 24 characters), `value`, `unit`, `unread`, `end_at` |
| `ttl_ms` | ms, beside the window | 0 | the view is withdrawn after that time; 0 or missing is no expiry |

A word the door does not know is refused: a `state` other than the two, a `seat` other than
`bottom`, a `ttl_ms` that is not a non-negative whole number.

**What the screen is set to.**

| Setting | Default | Meaning |
|---|---|---|
| `linger_ms` | 20000 | the linger of a window whose application asks for none |
| `fade_ms` | 120000 | how long the fall from full weight to nothing takes |
| `focus_default` | 0.3 | the bar while no verdict stands |
| `judge_min_interval_ms` | 3000 | the shortest distance between two judge calls |
| `judge` | `off` | `on` lets the curator ask the judge cell |
| `screens.<name>` | none | one output per entry, with its profile |
| `default_screen` | none | mandatory, and it names an entry of `screens` |
| `browser_mount` | `browser` | the browser cell a page is streamed from; the screen writes it on every page it draws |
| `params.mount` of the cell `web` | `display` | the path prefix of every output |

The judge writes four things and nothing else: `judged_relevance` and `judged_hidden` per
window, `bar` and `weights` on the state. The curator writes the rest, `rung`, `since`,
`score`, `decay`, `age`, `dismissed_at`, `led_until`, `topic_dupe`, the drawing level, each
tile's rank and `unseen`. What each of them means is `compose/scenarios/pass.py`.

## The pass

One run of the curator over the whole state: read, compute, write. It runs on six events
and on nothing else. An application writes a view, an application withdraws one, a verdict
arrives, a tap, a hold, a stroke of the screen's clock. Twelve steps:

1. **Triggers.** The closed list of events, when the judge is called, and what a verdict
   does to the state.
2. **The door.** What the screen normalises and what it rejects, for views, for profiles
   and for the settings.
3. **Touches.** The closed list of touch sources, what a finger touch writes that an
   application touch does not, which of them clear the window's standing verdict -- the
   judge's two values and, with them, the weight of that window's context -- and the
   gestures as they reach the state.
4. **Presence.** Which applications are present, how presence ends, and what happens when
   two of them are about the same topic.
5. **The chat closes.** The canvas window carrying the last turn's `turn_id` closes the
   chat, and what opens it again.
6. **Score.** Score, decay, linger and the bar. A window whose verdict a touch cleared
   counts with the default weight 0.5 until the judge speaks again.
7. **Rungs.** The two ladders, the urgents, the finger's lead, and `focus`, `relevant`,
   `ambient` and `hidden`.
8. **Level.** The drawing level out of ladder and rung, and what open means.
9. **Dock.** The rank, the order from the bottom, the seats, and the cut per output.
10. **`unseen`.** What waits in closed tiles.
11. **Strokes.** The moments the curator orders from the cell `clock`.
12. **`age`.** `fresh`, `settled`, `leaving`.

The code is `compose/scenarios/pass.py`, one pure function per step. `compose.py` carries
those bytes verbatim between two marker lines, written by `compose/scenarios/sync_pass.py`
and never by hand. The pins are `compose/scenarios/scenarios.json`, and
`python3 compose/scenarios/run_display_scenarios.py` runs them against the model and
against the cell.

## Gestures

Immediate, deterministic, and without the judge.

- **A tap on the tile of a window that is not open** touches that window. `since` moves to
  now, the window leads its ladder, and `led_until` holds it there for its linger.
- **A tap on the tile of an open window** puts it away. The window closes, the tile keeps
  its place and its rank, the decay runs on, and a later application touch lifts it.
- **A tap on the tile of a canvas application while a modal is open** puts the modal away
  as well, whether the tap opened something or put it away.
- **A hold on the OS mark** acts on the window with `topic: chat` like a tap, even when it
  is open. A hold never puts away. Without such a window the hold is absorbed.
- **A press on the OS mark**, shorter than the hold threshold of 250 ms, opens or closes
  the dock in one short movement, the same one in both directions on every output. It stays
  in the browser: no event, no pass. The initial state is the profile's `dock_default`, the
  toggle is not remembered, and a closed dock never opens by itself.

Two events carry all of it: `tap`, with the object id of the window it is for, and `hold`.
An application that names an event of its own `tap` or `hold` loses it to the screen, and
every other name is delivered. The browser draws the effect of a tap at once and the next
pass confirms it. Because the finger stands above the score, the pass never disagrees. It
may be late, though: a pass that was already running when the finger landed renders the
state from before the tap. The browser keeps its own drawing until a patch carries a state
computed after the tap, which it reads off `data-acted` - the moment this window was last
touched or put away.

## Outputs and profiles

An entry of `screens` is one output. Its profile:

| Field | Values | Default |
|---|---|---|
| `display_type` | `tv`, `monitor`, `phone` | mandatory |
| `viewing_distance_m` | a number | the reference of its type |
| `physical_size_in` | a number | unset; carried, read by no rule |
| `inputs` | any of `audio`, `touch`, `pointer`, `keyboard` | empty, and empty for a `tv` whatever the profile says |
| `dock_default` | `shown`, `hidden` | `shown` on a tv and a monitor, `hidden` on a phone |
| `dock_max` | a whole number | 7 on a tv, 8 on a monitor, 5 on a phone |

Every output shows the same open windows, the same rungs, the same levels. What differs is
the rendering. A monitor puts canvas windows side by side, up to three in a row and further
ones below. A tv puts up to two side by side, with larger type and fewer details. A phone
stacks them, the leading one on top. When the open windows do not fit, the canvas scrolls
and the page does not, and no window is missing.

`inputs` gates what is bound. Without `pointer` and without `touch` there are no hover rules
and no press. Without `audio` there is no hold: a long press is a press, the browser records
nothing, and the mark has no listening state. A television is an output device, so its
inputs are empty and hold and press come from a phone or a laptop.

`/<mount>/` is a switch. The page asks once, in the browser, for a coarse pointer and a
narrow viewport, and goes to `/<mount>/phone`, otherwise to the output `default_screen`
names. An explicit URL overrides the switch, and a name with no entry in `screens` is a 404.

The scale is one token. A tv starts at 1.6, a monitor and a phone at 1.0, modulated linearly
against the reference distance of the type (3.0 m for a tv, 0.7 m for a monitor, 0.35 m for
a phone) and clamped to 0.8 and 2.2. A missing distance counts as the reference, so the
factor is one. The sheet scales through tokens only, and no width query overrides a profile.

The dock is a column at the right edge with the OS mark at its foot, and all tiles are one
size. A tile carries its glyph, its line, its value and unit, whether it is unread, its
window's rung as colour rather than as size, whether that window is open, whether it is
pinned, and a countdown where the window gave an `end_at`. Tiles stay readable over every
window. The mark itself stands at the bottom right without a surface behind it, and shows a
dot as long as something waits in a tile the dock is not showing.

A window on the modal level sets the canvas behind it back a little, a window on the urgent
level sets both back further. Nothing else blurs. The dock and the mark lie above all of it
and react to no change of focus.

What the dock cut takes from an output is the tile, never the window. The application stays
present, and a window that is open stays open. A tile that was cut cannot be tapped on that
output, and can be on every other.

The front urgent rings. In the tile that is visual. The sound is the browser's, on the
urgent's arrival and then every two seconds for at most a minute, and a browser that forbids
sound before the first gesture stays silent.

## Apps

An app is a view. It sends exactly one window, one of `display-pane`, `display-panel`,
`display-overlay` or `display-view-prose`, with the hints above and a child keyed `tile`.
One view is one app, one window, one tile, one rung. Whoever shows several things sends
several views.

When a window brings no tile, the curator builds one: the glyph from the `context`, else
from the `class`, else a dot; the line from the window's title, else its kicker, else the
owner's name; no value.

An app may put a `display-input` into its window, one field and no form. Enter sends the
app's own event with the text and the screen clears the field. Whether the field appears on
an output is that profile's `inputs`.

`linger` is a request with a cap, and the app does not learn whether it was capped.
`pinned` pins the tile and never the window; whoever wants the window in front holds it
with touches.

A **note** is a window the curator builds out of a classified message of any cell. The
owner is the sender, the ladder is `canvas`, and the class supplies the relevance.

The chat competes on the modal ladder. The clock, the weather, a timer and every
voice2vision card compete on the canvas.

An app may show a **page**: a picture a browser cell of the member's renders and streams on
a topic of its own. The window is an ordinary window, a `display-pane` with the content
component `display-browser` in it, and the page is content rather than a fifth kind of
window. The screen joins the page's topic while that window carries a level of 1 or more
and leaves it at 0; the level is one for the whole screen, so every output joins or none
does, and a dock cut never takes a window. The page adds nothing to the state: no field, no
hint, no rung of its own.

Which browser cell answers is `browser_mount`, a setting of this cell, shipped as `browser`.
The screen writes it onto every `display-browser` it draws, the way it writes the window a
typed line belongs to, because the name is the operator's arrangement of the member's colony
and an application cannot know it.

## The chat app and the channel `chat`

The chat is the whole conversation of the member with their assistant, across every
channel: the mark, the telephone, Telegram, typed. A channel is a transport form, never a
conversation of its own.

`chat` is one of those channels and has its own talky. The chat app hands the event of its
input line to `channels/chat` as a turn, and from there it goes the way of every turn:
firewall, the member's session, talky. The turn carries the text, a `turn_id` the channel
assigns on acceptance, and the channel's own name. It is not a disguised voice turn.

The answer reaches the apps and appears in the window. It is not spoken, because this
channel has no loudspeaker. An answer to a voice turn is spoken and appears in the window
as well, because the chat shows every channel. The answer carries its turn's `turn_id`, and
every app that builds a window out of it, or touches its standing window on it, writes that
`turn_id` to its own window. That write is what lets a fresh canvas window close the chat.

Every line says which channel it came from, by glyph or word, and beside it the time the
line arrived. The line carries that time raw, as `at` in epoch milliseconds; the client
writes HH:MM into it in the time zone of whoever is looking, because the screen state
has none and a device has one. A line of another day carries the date short in front of
it. The tile carries no time.

The chat window, as the app writes it: `layer: modal`, `pinned: true`,
`context: conversation`, `topic: chat`, a relevance that is higher while the newest line is
the member's and lower once it is an answer, and a `linger` of its own. Its children are the
lines with the newest at the foot, the input line below them, and the tile: a speech bubble,
the last line, and `unread` when the last line is an answer that arrived after the member's
last turn.

The chat app holds its lines itself. Every turn and every answer of the member reaches it as
an event, from any channel, with the channel and the `turn_id` as fields, and it reads no
talky store. On each of them it writes `touched`, because the lines are children and a child
change is no touch on its own. Whoever types on a member's screen is the member: the
identity comes from the member's channel, and authentication in front of it is the proxy's
business.

## The app cell `ambient`

Three views. `clock` sits at the bottom of the dock. `weather` sits above it and carries its
value and its unit apart, so the tile shows a number and the window a place, a condition and
the day's range. In both the unit stands under the number, as its label. `timer` carries an
`end_at` and asks for `state: urgent` while it rings, for `ring_ms`; at the end of the ringing
it removes `state` alone, which is no touch.

`pin_clock` and `pin_weather` are the member's own dials. A pinned tile stands in the state
permanently, and the window follows score and bar like any other.

The clock writes its time into a child every minute and the weather writes a new measurement
the same way, both without `touched`. That is a pass and no touch, so neither of them wakes
the judge. A change of place is a change of `topic`, which is an own prop, and that is a
touch.

## What no longer holds

| What was said | Where it stood | What holds now |
|---|---|---|
| Several windows only on the canvas level, the count a dial of the profile | an earlier ruling | every output shows the same open windows; how they are arranged is rendering |
| A phone shows one window | this README | a phone stacks the open windows, the leading one on top |
| A tile disappears when the relevance falls below the threshold | an earlier ruling | presence ends by decay, by `ttl_ms`, by a topic duplicate or by withdrawal, never by a verdict |
| Every relevant app always has a tile, on every output | an earlier ruling | the tile stands in the state; the dock cut is rendering, per output |
| The model decides which content belongs in which window | an earlier ruling | the apps write the content, and the judge chooses through relevance what is open |
| The model sorts the apps into focus, relevant, ambient and hidden | an earlier ruling | the judge writes relevance and the bar, the curator writes the rung |
| Pinned means no decay, and the window stays in front | an earlier ruling | a pin holds the tile, and the window follows score and bar |
| A tap sets the relevance back to its start value | an earlier ruling | a tap is a touch: `since` moves to now, and the finger stands above the score |
| A tap on a tile closes only the leading window | this README | a tap on any open window puts that window away |
| Blur only when something must be answered | this README | the modal level sets the canvas back, the urgent level sets more back |
| The dot counts what the member has not yet seen | an earlier ruling | it counts what waits in tiles the dock is not showing; nothing remembers having been seen |
| A drawing level per output, `unseen` per output, a narrowest output | a wave's spec | one level per window and one `unseen`, both systemwide |
| `state` as the curator's word for a window's step | a wave's spec | `rung` |
| `dock_max` caps the ranked tiles and the furniture stands on top | a wave's spec | it caps the dock |
| A typed turn on the channel `voice` | a wave's spec | the channel `chat`, with its own talky |
| The events `tile` and `touch` | a wave's contracts | `tap` and `hold` |
| The hint `modal` beside `layer` | this README | `layer` alone, with the two ladders |
| A television with `audio` in its inputs | a wave's spec | a television has no inputs |
| `hidden` means gone from the page | this README | `hidden` is the lowest rung: no window, and the tile stands while the app is present |
| The canvas ordered by first appearance | this README | the canvas is ordered by what the windows mean |
| No clock time on chat lines | an earlier ruling | every line carries its source and its clock time; a TILE still carries none |

## Versions

- `1.0.0` A screen is a channel of the member. A view carries an owner, and an event finds
  its way back to the agent whose view it was.
- `1.1.0` A second column, and an order a rewrite cannot move.
- `1.2.0` The screen carries a microphone: hold to talk, over the page's own socket.
- `2.0.0` No port of its own. The `web` cell inside hangs on the colony's one listener
  under a mount.
- `2.1.0` The screen's own design language. The sheet and the component catalogue travel
  inside the shell definition.
- `2.2.0` The screen curates. Windows carry hints, the curator scores them, a bar decides
  what is open, a judge cell may re-judge the whole screen, and a clock cell strikes the
  moment the curator predicted.
- `2.3.0` One canvas, a dock of tiles of one size, and an OS mark where the microphone
  capsule was. Each output gets a profile.
- `2.4.0` Four levels and two ladders. A tap is a touch, a window may say how long it
  lingers and where its tile sits, and a window may hold a line to type into.
- `2.5.0` The curator is the description's reference model, byte for byte. The state is one
  row in the store, the two events are `tap` and `hold`, every named output is a rendering
  of its own with `/<mount>/` as the switch between them, each profile carries its own
  `dock_max`, and the scenarios travel with the template. A chat line carries its clock
  time beside its channel word, raw as `at`, and the browser formats it.
- `2.5.1` The curator runs one message at a time.
- `2.6.0` A patch leaves only after the state row has landed. What the store
  refused is never drawn. And a window may hold a page: a picture a browser cell
  renders and streams, joined while the window stands and given back when it is
  put away.
