# Roadmap

The [issue tracker](https://github.com/mmeyerlein/meclaw/issues) is the single
source of truth for everything actionable. This file only orders it: what comes
next, what comes after, and why.

Three rules keep it from silting up:

- **A stream names open issues only.** Work that shipped leaves the stream and
  appears once, as one line, under [§ Shipped](#shipped).
- **No content lives here twice.** The issue carries the detail; this file
  carries the ordering and the reason.
- **Closed issues are not eulogised here.** Their record lives where they
  closed — in the issue itself.

Release detail is in [CHANGELOG.md](CHANGELOG.md) and the
[GitHub releases](https://github.com/mmeyerlein/meclaw/releases).

## Now: meclaw-os, the organism

A colony grown from a seed into a personal operating system — the four
composition levels ship as templates, and
[`examples/meclaw-os`](examples/meclaw-os/) proves the claim end to end: an
empty seed, one declaration, seventeen cells. The gate for a public showcase is
a provably better agent, not more memory machinery.

- [#124](https://github.com/mmeyerlein/meclaw/issues/124) — the first thing the
  advisor measured about itself: a consult round trip is far too slow for a
  question memory could have answered.

## Next: substrate flanks

Findings from running the thing.

- [#443](https://github.com/mmeyerlein/meclaw/issues/443) — an `add_templates`
  declaration that survives a refusal one stage later in the same diff leaves
  its staged library entry behind, so the retry is refused by name.

Two lines here are **triggered**, not scheduled: metering what a subscription
plan carries (fires when a recurring lane wants the subscription path, was
[#48](https://github.com/mmeyerlein/meclaw/issues/48)), and the message-header
size watch (fires on drift past ~100 KB on a single hop; last reading 5.4 KB
max, query in [#141](https://github.com/mmeyerlein/meclaw/issues/141)).

## Later: memory, after the measurement

The memory hive is public since 0.9.0. The one finding that orders this stream,
from a 50-question LongMemEval run: **the bottleneck is the synthesis, not the
remembering** — in nineteen of twenty-one wrong answers the retrieval had
already delivered the gold session
([#148](https://github.com/mmeyerlein/meclaw/issues/148), measured on 0.9.0).
The re-run waits until the memory chain (collector, recall, curator, memory
hive) stops moving between builds.

- [#261](https://github.com/mmeyerlein/meclaw/issues/261) — the memory porter
  predates the substrate's `transfer` slot and duplicates four things it does
  natively; it can shrink to a walk over the sixteen tables. Not urgent, but it
  is the one component where a member's history is at stake, so it earns a slot
  of its own rather than a place at the end of a wave.

## Alongside: surfaces and docs

- [#43](https://github.com/mmeyerlein/meclaw/issues/43) — the README showcase
  is down to its last piece: the moving proof is a capture of `canvy` now, or
  nothing; the keyless quickstart and the annotated trace shipped long ago.

Realtime voice is the near-term line in this stream — not a further channel but
the way the thing is meant to be operated, spoken intent straight into
structure in the tree. It gets a design round of its own before it gets an
issue. Dictation (a voice note through the ordinary text path) stays fully
designed and explicitly secondary.

## Ongoing: community templates

The template surface is open: a template is a directory, a README and a
`template.json`. Thirty are listed in
[`templates/README.md`](templates/README.md) as worked examples; what a hive
template has to satisfy is § *The hive boundary* there — a requirement, not a
convention.

- [#138](https://github.com/mmeyerlein/meclaw/issues/138) — the environment
  knobs are a declared **experimental** surface; ~140 knob names remain across
  the shipped templates and migrate to params one template at a time, defaults
  bit-identical. Order: `memory-hive`, then `builder-librarian`, then the long
  tail.

## Shipped

One line per release; details in [CHANGELOG.md](CHANGELOG.md) and the
[GitHub releases](https://github.com/mmeyerlein/meclaw/releases).

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
