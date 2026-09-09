# You talk, it shows

Why does an assistant here get a voice and a screen at the same time? Typing is the slow end of
an assistant and a wall of prose is the weak end, so speech carries the conversation and a
screen carries the substance. The screen belongs to the person, not to an agent, which is what
lets several agents and apps write onto one.

## What ships today

Speech is a channel. Since 0.31.0 `voice` is a cell type: audio arrives on a WebSocket port of
its own, recognition and synthesis sit behind traits, and the colony sees ordinary text turns
([`voice-wire-protocol.md`](../voice-wire-protocol.md)).

A screen is a channel too. [`display`](../../templates/display/) is one screen as a hive with a
port of its own, and a view is a named, owned, optionally expiring piece of it. Whoever sends a
view owns it, replaces it under the same name and takes it down, and nobody writing to it needs
to know that anybody else does. An app owns no port and no origin of its own. It states a view
on a lane and the wiring decides which screen holds it, which is why the same drawing can go to
a display on a laptop or to two displays at once.

Since 0.32.0 there is a path from an answer to that screen. A front model may append one fenced
`sidecar` block to its reply, and every top-level key in it is a section. The member sorts the
sections, `memory` goes into the memory hive and everything else into `./apps`, and the mutation
that installed an app drew the edge for the section that app offered (`member@1.7.0`,
`assistant@2.6.0`). A section is an offer and never an instruction, so what a model proposes and
what an app accepts stay two decisions.

[`colony-view`](../../templates/colony-view/) is the app the library ships: a committed mutation
triggers a topology snapshot, a `code` cell turns it into one component tree, and the view leaves
the hive towards a display.

## What stays a person's judgement

Nothing in the tree decides what deserves the screen and what belongs in the voice. That sits
with whichever front model is answering, and no measurement stands behind it. There is one
screen layout and one shipped app, and whether the division of labour holds up is something a
colony in daily use answers first.

## Where to read on

- [meclaw-os](../meclaw-os.md) for where voice, display and apps sit in the levels
- [templates and apps](../templates-and-apps.md) for the three ways an app plugs in
