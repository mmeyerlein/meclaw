# You talk, it shows

**Status: this is an idea.** There is no voice template in this repository, no
speech-to-anything cell, and nothing on this page is a shipped feature unless it says so
explicitly. It is written down because it is the direction the assistant is built
towards, and because several pieces of the substrate only make full sense in its light.

## The idea

The keyboard-and-chat-bubble interface is a bottleneck on both ends. In: typing is the
slowest way to tell an assistant anything. Out: a paragraph of prose is the weakest way
to receive a plan, a list, a comparison, a picture.

The idea is the movie *Her*, taken literally on one axis: **speech in, sight out.** You
talk to your assistant the way you talk to a person — and the answer does not come back
as a wall of text in your ear, but appears on the window you and your agents share:
lists, plans, tables, images, controls. The voice carries the conversation; the display
carries the substance.

Think of the input half like dictation done right — always there, low-latency, no app to
switch to. The output half is what dictation tools do not have: a surface the assistant
*draws on* while you speak.

## What already stands under it

The idea is not shipped, but it is not floating either. Three pieces of the tree exist
today because of it:

- **The window exists.** [`display`](../../templates/display/) is one screen as a hive: a
  `web` cell on its own port, a `views` store, a deterministic compose step. It is a
  *channel*, so it belongs to the **person** — several agents hold views on one screen,
  and none can touch another's, because view ownership comes off the envelope the
  substrate stamped. [`colony-view`](../../templates/colony-view/) is the first app that
  draws on it. One screen, one app today.
- **Sessions are already phone calls.** The session-keeper models a session as a channel
  generation, on the pattern of a phone call, ended by arithmetic. The speech-shaped
  session model is in the substrate before speech is.
- **Images already travel inward.** The `llm` cell takes image attachments — vision *in*
  exists; the idea adds the mirror direction, vision *out*.

## What is missing

The entire voice half: capture, transcription, latency budget, barge-in, the decision of
what deserves the screen versus the voice. That is a design round of its own before it is
an issue — it is on the [roadmap](../../ROADMAP.md) as exactly that, and nothing more.

## The name

The idea has a working name and a place where its story will unfold:
**[voice2vision.eu](https://voice2vision.eu)**. If it grows, it grows the meclaw way — as
templates on top of the substrate, not as a fork of it.
