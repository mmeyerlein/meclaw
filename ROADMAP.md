# Roadmap

The [issue tracker](https://github.com/mmeyerlein/meclaw/issues) is the single
source of truth for everything actionable. This file only orders it: what comes
next, what comes after, and why. Content lives in the issues, never here twice.
Milestones mirror these streams. Shipped history lives at the bottom, one line
per release; the details are in [CHANGELOG.md](CHANGELOG.md) and the GitHub
releases.

## Now: meclaw-os, the organism

A colony grown from a seed into a personal operating system. This is the stream
now, because the gate for a public showcase is a provably better agent, not more
memory machinery. The epic
[#26](https://github.com/mmeyerlein/meclaw/issues/26) leads and carries the
settled principles and open forks; its sub-issues in intended order, collector
hives first:
[#27](https://github.com/mmeyerlein/meclaw/issues/27) collector hives as context orchestrators,
[#30](https://github.com/mmeyerlein/meclaw/issues/30) talky lifecycle,
[#31](https://github.com/mmeyerlein/meclaw/issues/31) per-talky memory,
[#32](https://github.com/mmeyerlein/meclaw/issues/32) one-file hives.

0.5.0 delivered the first half of this stream as templates: the collector hive
and its fan-out half (#27), the session lifecycle and its handover, the llm
cell's seed path (#99), and the collector's own follow-ups #77, #91, #76 and
#103. 0.6.0 put the front door on it: the firewall hive (#36), one talky per
channel (#29), and window-mode memory requests as a real tool round (#78). 0.7.0
split the thinking off the talking (#28): a tool that answers on its own lane
while the channel is served at once, the `in_advice` return round, the consult
ETA as an observe-only measurement (#123), and the drain adapter that finally
carries a closed day into memory (#101). What is left of the epic is per-talky
memory and one-file hives — plus
[#124](https://github.com/mmeyerlein/meclaw/issues/124), the first thing the
advisor measured about itself: a consult round trip is far too slow for a
question memory could have answered.

0.8.0 and 0.9.0 carried the stream past the point where the agent was the only
thing being built. 0.8.0 hardened the substrate the organism runs on; 0.9.0 made
its memory a public building block, moved an episode from the session close to
the turn it happened in, and gave a hive the means to own its domain — ports a
parent may not wire around (#133) and a store write surface that is internal to
its hive (#132). The example that proves the claim end to end lives in
[`examples/meclaw-os`](examples/meclaw-os/): an empty seed, one declaration,
seventeen cells.

Still open in this stream:
[#33](https://github.com/mmeyerlein/meclaw/issues/33) templates as the public
app store, [#34](https://github.com/mmeyerlein/meclaw/issues/34) a coding hive
built with the builder.

## Next: substrate flanks

The pre-MVP stream closed with 0.4.1; what its waves left behind are named
flanks, and new substrate findings from production land here first. 0.5.0 took
#94, #95, #83, #89, #97 and #98 off this list. What remains:
[#96](https://github.com/mmeyerlein/meclaw/issues/96) the mcp and subcolony
stdio children still run with the daemon's full rights,
[#46](https://github.com/mmeyerlein/meclaw/issues/46) the harness permission
surface and error-code semantics,
[#45](https://github.com/mmeyerlein/meclaw/issues/45) the code cell's
per-invocation interpreter start,
[#48](https://github.com/mmeyerlein/meclaw/issues/48) measuring what a
subscription plan actually carries until reset.

The tool cells got their own pass in 0.6.0: the contract batteries of #104
turned into seven follow-ups, and #105–#110 shipped with this release — the
match-count guard, binary and windowed reads, a fence that is no existence
oracle, a line-closing insert, the store error taxonomy and URL parsing at the
`web_fetch` gate. What is left of that set is
[#111](https://github.com/mmeyerlein/meclaw/issues/111), guarding interpreter
bytecode caches in the coding templates' edit-test loops.

An architecture comparison with prime-agent (2026-08) added a hardening batch to
this stream, and 0.8.0 shipped all of it: the sharpest gates now run in CI
(#115), a hard daemon crash cannot leak children (#116), `web_fetch` refuses the
private network by default (#117), a second daemon cannot boot on the same root
(#121), writes into the llm cell's persistent system tree are gated (#118), a
TTL death inside a fan-in can be answered (#119), and the store-backed tool loop
got a context-compaction lane as a reference topology (#120). Its leftover
advisory findings (#127) closed in 0.9.0.

0.9.0 added a flank of its own, from the release audit rather than from
production: [#138](https://github.com/mmeyerlein/meclaw/issues/138), the
remaining environment knobs that have not moved to the params surface yet —
`collector@1.2.0` is the worked migration —,
[#140](https://github.com/mmeyerlein/meclaw/issues/140), params for cells inside
a subtree template that cannot be set at instantiation, and
[#141](https://github.com/mmeyerlein/meclaw/issues/141), message headers as a
standing watch: unbounded by design, measured every few weeks rather than
capped.

## Later: memory, after the measurement

**The stall is over: the memory hive shipped publicly in 0.9.0**, test suite
included, and what used to wait here has largely landed — the extraction lane's
double claim (#72) and its cost-first gate defaults (#51) in 0.9.0, the recall
query hygiene guard (#88) proven in production, the three hive bugs (#52, #53,
#73) back in 0.4.0.

What replaces the list is one measurement. A 50-question LongMemEval run against
the 0.9.0 tree returned **96 % R@5 and 58 % end accuracy**: in 19 of 21 wrong
answers the retrieval had delivered the gold session and the synthesis failed to
answer from it. The sharpest case scored 100 % R@5 against 30.8 % accuracy.
[#148](https://github.com/mmeyerlein/meclaw/issues/148) is therefore where this
stream now points — the bottleneck is not the remembering.

Still open around it:
[#55](https://github.com/mmeyerlein/meclaw/issues/55) no consumer derives a
recall window, so time-range questions run as point recalls,
[#47](https://github.com/mmeyerlein/meclaw/issues/47) the async cell shutdown
drain,
[#147](https://github.com/mmeyerlein/meclaw/issues/147) wiring the inline ingress
without its reject drain fails silently — a wiring-time check, deliberately held
for the architecture pass that makes inline extraction a system-wide property
rather than a per-hive feature.

## The wave before the launch

Three packages that all answer the same question — what does a colony need
before it can be handed to somebody else — and they are built rather than
planned:
[#151](https://github.com/mmeyerlein/meclaw/issues/151) the `vault` cell type,
whose route surface has no read on it and whose unlock attests its own edge
neighbourhood before it takes the key;
[#154](https://github.com/mmeyerlein/meclaw/issues/154) audience **sets** in
`affinity`, so a fact is usable only in a round that is a subset of the one it
surfaced in;
[#155](https://github.com/mmeyerlein/meclaw/issues/155) the `steward`, the
control loop that measures its own colony, simulates on the ledger, mutates
through the ordinary gated lane, verifies, and keeps or reverts against a
pre-authored plan.

What is left of the wave is deliberately not built yet: the vault is a template
rather than a lane inside `access@1` (rewiring a proven security cell is its own
step), and the steward is tested but not instantiated in the reference colony —
its first real cycle belongs in the test days, not in the night before them.

## Alongside: surfaces and docs

New ways in and out, [#38](https://github.com/mmeyerlein/meclaw/issues/38)
voice ingress first (dictation-style now, realtime speech when the APIs land),
then [#39](https://github.com/mmeyerlein/meclaw/issues/39) the realtime HTML
window. The docs travelled in two steps, and the first one (#92, README and
website on the current release) is done. What is left is
[#93](https://github.com/mmeyerlein/meclaw/issues/93), the full rewrite once
meclaw-os lands, and the remainder of
[#43](https://github.com/mmeyerlein/meclaw/issues/43): the keyless quickstart and
the annotated message trace shipped with 0.9.0, the *moving* demo did not.

## Ongoing: community templates

The good first issues from the first public wave — #3 and #4 — shipped with
0.5.0 as `retry@1.0.0` and `archive-bridge@1.0.0`. The template surface is open: a
template is a directory, a README and a `template.json`, and the fourteen listed
in [`templates/README.md`](templates/README.md) are the worked examples — eleven
single-purpose ones plus three composites: `talky@1.2.0`, which carries four of
them as sub-units, `cogny@1.3.0`, which carries two, and `memory-hive@1.2.0`, the
agent memory as a hive of ten cells. New ones are welcome.

## Shipped

One line per release; details in [CHANGELOG.md](CHANGELOG.md) and the
[GitHub releases](https://github.com/mmeyerlein/meclaw/releases).

- **v0.10.0 — the wave before the launch.** A secret store whose route surface
  has no read on it and whose unlock attests its own edges before taking the
  key; audience **sets** in affinity, so a fact is usable only in a round that
  is a subset of the one it surfaced in; and the steward, the control loop that
  measures its own colony, simulates on the ledger, mutates through the ordinary
  gated lane, and keeps or reverts against a plan authored beforehand.
- **v0.9.1 — what production found.** Four defects out of running 0.9.0 rather
  than reading it: an extraction lane that re-sent a failing batch every five
  seconds, a recall request silently ignored because it carried another
  consumer's chain state, a pin test that ran an example without its seed step,
  and a persona that consulted the core for what its own window already held.
- **v0.9.0 — sealed hives, open memory.** The memory hive ships publicly with its
  test suite, an episode reaches memory at the turn instead of at the session
  close, a hive can seal its ports and a store its write surface, and a `code`
  cell's stdin becomes a structured document.
- **v0.8.0 — the hard shell.** The hardening batch in one day: SSRF policy in
  the `web_fetch` cell, an orphan journal and boot reap, a root lease, a gate on
  the persistent system tree, an answerable TTL death, a compaction lane — and
  the corridor diffs, the unwrap ratchet and `cargo deny` moved from a document
  into CI.
- **v0.7.0 — the advisor.** A tool may answer on a lane of its own while the
  channel is served in the same breath, the advice returns as a fresh round, and
  `memory-drain@1.0.0` carries a closed day into memory losslessly and idempotently.
- **v0.6.0 — the front door.** The firewall hive screens every inbound turn
  against rules that are data, the receptionist hands each channel an agent of
  its own, and memory becomes a tool round the agent can aim at a time range.
- **v0.5.0 — the agent wave.** Two waves in one night: the collector hive goes
  public and four more templates join it, the tool cells get contract batteries,
  and the boot probe stops guessing from row counts.
- **v0.4.1 — the pre-MVP finish line.** Sandbox phase 2, system-tree pointer
  resolution, the attachments wiring with the llm cell as first consumer.
- **v0.4.0 — the bug-and-substrate wave.** Every open bug on the tracker plus
  half the pre-MVP items, on five parallel tracks; the upgrade-breaker #90
  found by its only red gate.
- **v0.3.2 — substrate reliability.** Seven defects between cells, ranked by a
  practice run rather than by reading the code.
- **v0.3.1 — memory quality follow-ups.** Every measured flank of 0.3.0, fixed
  and re-measured; the 5K case is right.
- **v0.3.0 — statement identity.** Attributed closures, judged cardinality,
  claim aliases, a currency marker in the bundle.
- **v0.2.0 — memory quality.** The identity pass: when are two remembered
  things the same thing?
- **v0.1.x — hardening.** Nineteen production defects as patch releases,
  through v0.1.16.
