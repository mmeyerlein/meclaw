# You talk, it shows

Typing is the slow end of an assistant and a wall of prose is the weak end. The pairing
several pieces of this tree are aimed at is the other one: you speak, and what comes of it
appears on a screen you and your agents share. The voice carries the conversation, the
display carries the substance.

## What ships today

Speech is a channel. Since 0.31.0 `voice` is a cell type: audio arrives on its own WebSocket
port, recognition and synthesis sit behind traits, and the colony sees ordinary text turns
([`voice-wire-protocol.md`](../voice-wire-protocol.md)).

A screen is a channel too, and it belongs to the person rather than to an agent.
[`display`](../../templates/display/) is one screen as a hive, several agents hold named
views on it, and [`colony-view`](../../templates/colony-view/) is the first app that draws on
one. How that is wired is in
[an operating system for agents](an-os-for-agents.md) § *Screens and apps*; this page is
about what it is for.

Since 0.32.0 there is a path from an answer to that screen. A front model may append one
fenced `sidecar` block to its reply, holding sections that are offers rather than
instructions. The member sorts them and fans them out to the apps wired at its rim, and an
app that holds a view puts its section up. What a model offers and what an app accepts stay
two decisions, made by two parties ([CHANGELOG](../../CHANGELOG.md)).

## What is missing

Nothing in the tree decides what deserves the screen and what belongs in the voice. That
judgement sits with whichever front model is answering, and no measurement stands behind it.
There is one screen layout and one shipped app. Whether the division of labour holds up is
something a colony in daily use has to answer first, and the [roadmap](../../ROADMAP.md)
carries it as exactly that.
