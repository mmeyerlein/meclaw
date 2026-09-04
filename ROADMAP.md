# Roadmap

The [issue tracker](https://github.com/mmeyerlein/meclaw/issues) is the single
source of truth for everything actionable. This file only orders it: what comes
next, what comes after, and why.

The four horizons are relative to the work, not to a calendar: **Now** is the
running wave, **Next** is the wave after the next instance rebuild, **Later**
has no date, and **Alongside** is cross-cutting — it rides with whatever wave
touches it.

Four rules keep it from silting up:

- **A stream names open issues only.** Work that shipped leaves the stream and
  appears once, as one line, under [§ Shipped](#shipped).
- **No content lives here twice.** The issue carries the detail; this file
  carries the ordering and the reason.
- **Closed issues are not eulogised here.** Their record lives where they
  closed — in the issue itself.
- **Every entry carries an anchor**: an open issue, or `(register: <id>)`. The
  second marks something deliberately *not* built, with a named trigger that
  would make it due. The register holding those reasons is internal, so the
  marker is the whole of what this file gives you about such a line — and that
  is the point: it is deferred on purpose, not forgotten.

A gate resolves both kinds on every push, and a line pointing at a closed issue
is a red build.

Release detail is in [CHANGELOG.md](CHANGELOG.md) and the
[GitHub releases](https://github.com/mmeyerlein/meclaw/releases).

## Now: the wave that shipped, and what it left standing

The clean-up wave is **released as v0.29.0**. Everything the organism wave left
standing in the tracker is either built or ruled: the submitter lives in the
front door, the deep edge is the declared form (v-lanes, ADR-0020) and the three
lanes that only bridged a level ride it now, a member carries its own broker and
its brains ask it over a sealed lane, a newborn's contracts are read below the
template root, and a generation grows with its credential road in one act. What
the wave did is the [`[0.29.0]`](CHANGELOG.md) section of the changelog. The
substrate flanks it opened — a lane cannot vanish inside one diff, a capped round
is a named partial answer, a key collision is a refusal, a citation marker never
reaches a person — closed with it.

One thing is open under this horizon, and it is about how the next wave is
built rather than about what it builds:

- [#577](https://github.com/mmeyerlein/meclaw/issues/577) — the gate process
  gets one entry point, and its scope comes from the diff. Three chains — a
  strand gate, a pre-push gate and the CI workflow — each carried its own
  hard-coded list of steps, so they drifted apart and each of them paid for work
  the change had not asked for: a Python-only edit still compiled the workspace.
  One runner asks one resolver which stations a diff needs, runs each of them
  once, and leaves a receipt the release gate can read instead of running the
  same four checks again.

The streams below carry what comes next; the tracker carries the rest.

## Next: substrate flanks

Findings from running the thing.

- [#543](https://github.com/mmeyerlein/meclaw/issues/543) — a member grows its
  own screen. Everything in the level already assumes one — the screen belongs
  to the person, not to a generation — and it is the one piece a second,
  hand-written act still has to supply. The port stays a parameter of the wish,
  because a port is an instance fact.
- Re-measuring the builder's acceptance quota, once the acceptance cases stop
  moving under it. The last run measured four cases, one of them ordering a
  build no template could deliver; a quota read off that is a reading about the
  cases. *(register: builder-acceptance-quota)*
- Metering what a subscription plan actually carries until it resets.
  Triggered, not scheduled: it fires when a recurring lane wants the
  subscription path. *(register: subscription-budget)*
- The message-header size watch. Headers carry no cap by design, so the watch
  is the instrument: it fires on drift past ~100 KB on a single hop, and the
  last reading was 5.4 KB max. *(register: header-size)*
- Telling submissions apart by the door they came in at. Every question the
  broker is asked carries the same requester and the same subject whichever
  front raised it, so a rule that would open the shell to the operator and not
  to an agent cannot be written today — the shell-scoped rule ships switched
  off instead. *(register: policy-by-requester-origin)*
- [#555](https://github.com/mmeyerlein/meclaw/issues/555) — the file is a
  first-class transfer form. The substrate produces export content and reads
  seed files, but never writes them; that gap is why a hand-written sink cell
  exists at the member level. Export learns a `to:` path, import learns a
  `from:` path, the sink dissolves.

## Later: memory after the measurement, and names for people

The memory hive is public since 0.9.0. The one finding that orders this stream,
from a 50-question LongMemEval run: **the bottleneck is the synthesis, not the
remembering** — in nineteen of twenty-one wrong answers the retrieval had
already delivered the gold session.

- [#261](https://github.com/mmeyerlein/meclaw/issues/261) — the memory porter
  predates the substrate's `transfer` slot and duplicates four things it does
  natively; it can shrink to a walk over the sixteen tables. Not urgent, but
  it is the one component where a member's history is at stake, so it earns a
  slot of its own rather than a place at the end of a wave.
- Re-running the answer half of that measurement, directed and stratified. It
  waits until the memory chain — collector, recall, curator, memory hive —
  stops moving between builds; a measurement of a surface still in motion buys
  a number that is stale by the next build. *(register: memory-answer-half)*
- Renaming, despite append-only. Nothing in the tree is ever deleted and a path
  IS a cell's identity — that rigidity is deliberate, it is what makes the
  record auditable. But giving a thing a name, and changing it later, is a human
  act, and today the only answer is `move_nodes`: an identity-level operation
  for what is a presentation problem. The likely shape is a description layer —
  mutable display names held in a database, never an address, the tree untouched
  underneath. It gets a design round before it gets an issue.
  *(register: display-names)*
- [#553](https://github.com/mmeyerlein/meclaw/issues/553) — polling stops. Two
  shipped timers ask for state that only changes on a committed mutation: the
  tool menu and the topology view. The mutation receipt becomes the event
  source, first fills happen lazily, and the timers go. One design, two
  subscribers; until it lands the timers are documented debt.

## Alongside: surfaces, docs, and the way it is operated

- [#43](https://github.com/mmeyerlein/meclaw/issues/43) — the README showcase
  is down to its last piece: the moving proof is a capture of the colony view
  now, or nothing; the keyless quickstart and the annotated trace shipped long
  ago.
- [#138](https://github.com/mmeyerlein/meclaw/issues/138) — the environment
  knobs are a declared **experimental** surface; ~140 knob names remain across
  the shipped templates and migrate to params one template at a time, defaults
  bit-identical. Order: `memory-hive`, then `builder-librarian`, then the long
  tail.
- [#552](https://github.com/mmeyerlein/meclaw/issues/552) — the memory hive
  answers tool calls itself. Today `memory_recall` is served by the collector
  out of a special lane pair, under a hand-typed schema; the hive becomes a
  classic tool answerer with a `schemas` cell of its own, and the collector's
  special case goes. `thread_recall` stays with the collector — the slate is
  its own database.
- [#551](https://github.com/mmeyerlein/meclaw/issues/551) — the rest of that
  sweep: five families of one role under two names, and the boundary the
  instance-name rule needs before it reaches the examples.
- Realtime voice — not a further channel but the way the thing is meant to be
  operated: spoken intent straight into structure in the tree. It gets a
  design round of its own before it gets an issue. Dictation (a voice note
  through the ordinary text path) stays fully designed and explicitly
  secondary. *(register: voice-realtime)*

The template surface is open alongside all of this, and it needs no entry to
stay that way: a template is a directory, a README and a `template.json`.
Thirty-eight are listed in [`templates/README.md`](templates/README.md) as worked
examples; what a hive template has to satisfy is § *The hive boundary* there —
a requirement, not a convention.

## Shipped

One line per release; details in [CHANGELOG.md](CHANGELOG.md) and the
[GitHub releases](https://github.com/mmeyerlein/meclaw/releases).

- **v0.29.0** — the tracker is clean: the front door is one place, the deep edge
  is the declared form, and a generation grows with its keys in one act.
- **v0.28.0** — the organism grows its own surfaces: four composition levels from
  one catalogue, a builder that submits through one front, a core with one brain
  that declares its own errand, and one memory hive for several askers.
- **v0.27.0** — the builder stops guessing and starts looking: the intake is a
  bounded, typed tool loop with four eyes and no hand.
- **v0.26.0** — a template arrives in a running colony (`add_templates`), and
  the shutdown finally drains.
- **v0.25.0** — a wish in the chat becomes a manifest somebody else submits.
- **v0.24.0** — the vault delivers without giving anything away.
- **v0.23.0** — a display moves without being rebuilt.
- **v0.22.5** — the toolchain moves when somebody moves it.
- **v0.22.4** — the registry has no unpinned bucket left.
- **v0.22.3** — a mixed completion answers the turn it spoke for.
- **v0.22.2** — a seed nobody can load is refused by name.
- **v0.22.1** — what the first day of a `web` cell in production found.
- **v0.22.0** — a display is a cell, and it owns its port.
- **v0.21.0** — the four composition levels, and a connector that is one cell.
- **v0.20.1** — an emission from the boot window is held, not lost.
- **v0.20.0** — identity comes off the edge, and the colony answers counts
  about its own books.
- **v0.19.0** — the turn annotates itself, and the closed session is read once.
- **v0.18.0** — a default, a slot, and one message instead of nine.
- **v0.17.4** — a refusal stops arriving as a result.
- **v0.17.3** — a template can put another template inside itself.
- **v0.17.2** — the error paths keep the contract's word.
- **v0.17.1** — the night the audit was answered.
- **v0.17.0** — content can leave a cell and enter a running one.
- **v0.16.0** — a fact remembers who was there.
- **v0.15.x** — every shipped hive is behind its boundary.
- **v0.14.0** — a name means one thing.
- **v0.13.0 / v0.12.x** — the canvas keeps its stylesheet's word; a surface
  installs into a running colony.
- **v0.11.x** — a colony serves surfaces over HTTP.
- **v0.10.x** — the wave before the launch: vault, audience sets, the steward.
- **v0.9.x** — sealed hives, open memory.
- **v0.8.0** — the hard shell.
- **v0.5.0–0.7.0** — the agent waves: collector, front door, advisor.
- **v0.4.x** — the bug-and-substrate wave, the pre-MVP finish line.
- **v0.1.x–0.3.x** — hardening, memory quality, statement identity.
