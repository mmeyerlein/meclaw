# Roadmap

The [issue tracker](https://github.com/mmeyerlein/meclaw/issues) is the single
source of truth for everything actionable. This file only orders it: what comes
next, what comes after, and why.

The four horizons are relative to the work and not to a calendar. Now is the
running wave. Next is the wave after the next instance rebuild. Later has no
date. Alongside is cross-cutting, so it rides with whatever wave touches it.

Four rules keep it from silting up:

- A stream names open issues only. Work that shipped leaves the stream and
  appears once, as one line, under [§ Shipped](#shipped).
- No content lives here twice. The issue carries the detail; this file carries
  the ordering and the reason.
- Closed issues are not eulogised here. Their record lives where they closed,
  in the issue itself.
- Every entry carries an anchor: an open issue, or `(register: <id>)`. The
  second marks something deliberately not built, with a named trigger that
  would make it due. The register holding those reasons is internal, so the
  marker is all this file gives you about such a line.

A gate resolves both kinds on every push, and a line pointing at a closed issue
is a red build.

Release detail is in [CHANGELOG.md](CHANGELOG.md) and the
[GitHub releases](https://github.com/mmeyerlein/meclaw/releases).

## Now

v0.32.1 is a documentation patch on top of v0.32.0: the README and the public
docs were rewritten so that a person can read them, and nothing in the contract
moved. The wave itself is v0.32.0.

v0.32.0 turns the block a front model appends to its answer into a typed offer.
The fence opens with ```` ```sidecar ````, holds one JSON object, and each
top-level key is a section. `memory` is what the old extraction lane carried;
everything else is offered to whatever listens. The member sorts them, memory
up into its hive and every other section into `./apps`, and the edge into the
app that offered a section comes from the mutation that installs the app, never
from the template. That is the app rim, and together with the screen's second
column it is what turns a spoken turn into something on a screen. Beside it, a
telephone became a channel of a person: a `freeswitch` template in front of the
`voice` cell, one call one session, the signalling kept as a book. What the
release contains is the [`[0.32.0]`](CHANGELOG.md) section of the changelog.

That release is also where v0.31.0 first reaches the public. It gave the
substrate a `voice` cell type: raw audio over a WebSocket in, text turns into
the tree, an assistant's turn back out as speech. It is built like the `web`
cell, and the audio terminates in the cell's I/O half, so no sample ever
becomes a message. Two speech-to-text and two text-to-speech providers sit
behind traits, an `echo` provider calibrates the wire before anybody blames a
model, and the cell serves its own browser test page. It was cut as the
[`[0.31.0]`](CHANGELOG.md) section and never tagged, so `v0.32.0` carries both.

Behind them, two releases that were already out. The clean-up wave shipped as
v0.30.0, and with it every issue the tracker held is either built or ruled. The
poll timers are gone: the mutation door leaves a receipt, and the menu and the
screen follow it. File transfer is a substrate slot, so every store writes and
reads the directories it owns, and the one cell that did it for the holders is
gone. `memory_recall` is an ordinary tool call answered by the member's own
memory hive. Every member grows a screen and an app, and the OS hands out the
port. The last ~140 environment knobs in the template library are params, with
a gate that keeps the surface closed. What the wave did is the
[`[0.30.0]`](CHANGELOG.md) section of the changelog. The wave that prepared it
left the gate process behind as one entry point whose scope comes from the
diff.

The first colony built on that release turned up four repairs, and v0.30.1
carries them: a member wish is one submission again, and `--validate` reads the
`override_params` a `ref` marker carries.

Nothing is open under this horizon as an issue. The streams below carry what
comes next; the tracker carries the rest.

## Next

Findings from running the thing.

- Re-measuring the builder's acceptance quota, once the acceptance cases stop
  moving under it. The last run measured four cases, one of them ordering a
  build no template could deliver; a quota read off that is a reading about the
  cases. *(register: builder-acceptance-quota)*
- Metering what a subscription plan actually carries until it resets. A trigger
  starts it: it fires when a recurring lane wants the subscription path.
  *(register: subscription-budget)*
- The message-header size watch. Headers carry no cap by design, so the watch
  is the instrument: it fires on drift past ~100 KB on a single hop, and the
  last reading was 5.4 KB max. *(register: header-size)*
- Telling submissions apart by the door they came in at. Every question the
  broker is asked carries the same requester and the same subject whichever
  front raised it, so a rule that would open the shell to the operator and hold
  it shut against an agent cannot be written today. The shell-scoped rule ships
  switched off instead. *(register: policy-by-requester-origin)*

## Later

The memory hive is public since 0.9.0. One finding from a 50-question
LongMemEval run orders this stream: the bottleneck sits in the synthesis. In
nineteen of twenty-one wrong answers the retrieval had already delivered the
gold session.

- Re-running the answer half of that measurement, directed and stratified. It
  waits until the memory chain (collector, recall, curator, memory hive) stops
  moving between builds. A measurement of a surface still in motion buys a
  number that is stale by the next build. *(register: memory-answer-half)*
- Renaming, despite append-only. Nothing in the tree is ever deleted, and a
  path is a cell's identity; that rigidity is what makes the record auditable.
  Giving a thing a name and changing it later is a human act, and today the
  only answer is `move_nodes`, an identity-level operation for a presentation
  problem. The likely shape is a description layer: mutable display names held
  in a database, never an address, with the tree untouched underneath. It gets
  a design round before it gets an issue. *(register: display-names)*
- A browser as a cell. A `link` card frames the page it names, and that is as
  far as a frame goes: a page decides whether it may be embedded, a growing
  share of the web says no, and nothing behind a login was ever reachable that
  way. The shape that lifts the limit is a browser running as a cell, with the
  page rendered where the colony runs, its picture streamed into the card, and
  pointer and keys routed back. A screen then shows any page, including the
  ones that refuse to be framed. It waits on the sidecar `display` section
  being in daily use, because only then is it clear which pages a person
  actually asks for. [#610](https://github.com/mmeyerlein/meclaw/issues/610)

## Alongside

Cross-cutting work: surfaces, docs and the way a colony is operated.

- Voice-to-graph. The channel exists: a `voice` cell takes speech in and puts
  turns into the tree. A spoken intent still arrives as a sentence somebody
  else has to act on, and never as a node and an edge. What is missing is the
  operating form: which utterances count as build intents, what a speaker hears
  while the graph grows, and what taking something back looks like when nobody
  pressed Enter. It waits on a `voice` instance being used in earnest for more
  than a day, because that is what tells a real intent from an invented one.
  Dictation, a voice note through the ordinary text path, stays fully designed
  and explicitly secondary. *(register: voice-to-graph)*

The template surface is open alongside all of this, and it needs no entry to
stay that way: a template is a directory, a README and a `template.json`.
Forty are listed in [`templates/README.md`](templates/README.md) as worked
examples, and what a hive template has to satisfy is § *The hive boundary*
there.

## Shipped

One line per release; details in [CHANGELOG.md](CHANGELOG.md) and the
[GitHub releases](https://github.com/mmeyerlein/meclaw/releases).

- v0.32.1: the README and the public docs rewritten in plain language, nine why
  pages became six, the catalogue descriptions shortened; no code change.
- v0.32.0: the answer carries typed offers. One ```` ```sidecar ```` block with
  sections, a member that sorts them into its apps, a screen with a second
  column, and a telephone as a channel of a person.
- v0.31.0: speech is a channel. The `voice` cell type and `voice@1.0.0`, two
  STT and two TTS providers behind traits, an echo provider, a test page.
- v0.30.1: the repairs a first colony on 0.30.0 turned up. A member wish is one
  submission, and `--validate` reads a `ref` marker's `override_params`.
- v0.30.0: every open issue is built or ruled. The poll timers are gone, a
  store writes its own files, memory answers an ordinary tool call, every
  member grows a screen, and one gate process reads the diff.
- v0.29.0: the tracker is clean. The front door is one place, the deep edge is
  the declared form, and a generation grows with its keys in one act.
- v0.28.0: the organism grows its own surfaces. Four composition levels from
  one catalogue, a builder that submits through one front, a core with one
  brain that declares its own errand, and one memory hive for several askers.
- v0.27.0: the builder stops guessing and starts looking. The intake is a
  bounded, typed tool loop with four eyes and no hand.
- v0.26.0: a template arrives in a running colony (`add_templates`), and the
  shutdown finally drains.
- v0.25.0: a wish in the chat becomes a manifest somebody else submits.
- v0.24.0: the vault delivers without giving anything away.
- v0.23.0: a display moves without being rebuilt.
- v0.22.5: the toolchain moves when somebody moves it.
- v0.22.4: the registry has no unpinned bucket left.
- v0.22.3: a mixed completion answers the turn it spoke for.
- v0.22.2: a seed nobody can load is refused by name.
- v0.22.1: what the first day of a `web` cell in production found.
- v0.22.0: a display is a cell, and it owns its port.
- v0.21.0: the four composition levels, and a connector that is one cell.
- v0.20.1: an emission from the boot window is held instead of lost.
- v0.20.0: identity comes off the edge, and the colony answers counts about its
  own books.
- v0.19.0: the turn annotates itself, and the closed session is read once.
- v0.18.0: a default, a slot, and one message instead of nine.
- v0.17.4: a refusal stops arriving as a result.
- v0.17.3: a template can put another template inside itself.
- v0.17.2: the error paths keep the contract's word.
- v0.17.1: the night the audit was answered.
- v0.17.0: content can leave a cell and enter a running one.
- v0.16.0: a fact remembers who was there.
- v0.15.x: every shipped hive is behind its boundary.
- v0.14.0: a name means one thing.
- v0.13.0 / v0.12.x: the canvas keeps its stylesheet's word; a surface installs
  into a running colony.
- v0.11.x: a colony serves surfaces over HTTP.
- v0.10.x: the wave before the launch. Vault, audience sets, the steward.
- v0.9.x: sealed hives, open memory.
- v0.8.0: the hard shell.
- v0.5.0-0.7.0: the agent waves. Collector, front door, advisor.
- v0.4.x: the bug-and-substrate wave, the pre-MVP finish line.
- v0.1.x-0.3.x: hardening, memory quality, statement identity.
