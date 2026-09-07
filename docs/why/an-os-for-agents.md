# An operating system for agents

`meclaw-os` is a set of templates that arranges a colony into four nested levels under one
rule: a level owns what its siblings must share.

## The four levels

| level | template | what it owns |
|---|---|---|
| shell | `meclaw-os` | the capability broker, the control loop, the authoring path, the one front door |
| organisation | `org` | a name and a boundary, and no cell of its own |
| member | `member` | the memory, the curated record, the screening, the channels |
| assistant | `assistant` | one generation: a conversation surface, a reasoning core, a tool surface |

Each `template.json` repeats the rule and says what it concluded from it.
`templates/org/template.json` is the shortest case: the members of one organisation share a
name and a boundary, they do not share a memory or a firewall, so the level is thin and says
so. Memory sits at the member level, so replacing an assistant does not take the history
with it. Screening lives outside the assistant, so a new generation meets the same attacker
record and the same rate window, and channels belong to the member, so one chat account can
reach two of that person's agents.

## The parts that decide

None of the authorities below asks a model. `access` is the capability broker: an agent may
ask in natural language, what travels on the wire is a handle, and the secret stays in the
connector. `firewall` measures size, sender, forbidden literal and rate, and every verdict
names the row that fired. `affinity` holds the curated record of the people a colony knows.
`session-keeper` ends a session by arithmetic, on the pattern of a phone call.

`argus` is the control loop, and everything it could pursue ships switched off: both rows in
`templates/argus/charter/seed/goals.jsonl` carry `"enabled": 0`, so a freshly grown shell
measures nothing until an operator turns one on ([self-modification](self-modification.md)).

## Screens and apps

A screen is a channel, so it belongs to the person. [`display`](../../templates/display/) is
one screen as a hive: a `web` cell that owns its own HTTP and WebSocket port, a store of what
is currently up, and a compose step. Several agents hold named views on one screen and none
can touch another's, because view ownership is read off the envelope the substrate stamped.
An app is a composed use of templates, tagged `app` in its manifest and grown under a member;
the first shipped one is [`colony-view`](../../templates/colony-view/), which draws the
colony's own topology onto such a screen.

Speech is a channel too. Since 0.31.0 `voice` is a cell type: it terminates audio on its
own WebSocket port and hands the colony text turns, with recognition and synthesis behind
traits ([CHANGELOG](../../CHANGELOG.md)). Voice and screen are aimed at the same use: you
talk to a colony, and a display shows what came of it. That aim is as far as it goes today.

## What is rudimentary about it

The tree ships one app and one screen layout. The apps rim is the youngest part of it, and
what an app may offer a member is still moving ([ROADMAP](../../ROADMAP.md)).

The OS is grown, so a seed with a single `ref` declaration builds the whole shell on first
boot through the same validation any mutation gets, and adding an organisation, a member or
an assistant is one declaration each ([`examples/organism`](../../examples/organism/)). None
of that gives you a way back: nothing in the tree is ever deleted, so a generation you regret
stays on disk, disconnected.
