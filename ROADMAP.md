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
[#28](https://github.com/mmeyerlein/meclaw/issues/28) the talky/cogny split,
[#30](https://github.com/mmeyerlein/meclaw/issues/30) talky lifecycle,
[#31](https://github.com/mmeyerlein/meclaw/issues/31) per-talky memory,
[#32](https://github.com/mmeyerlein/meclaw/issues/32) one-file hives.

0.5.0 delivered the first half of this stream as templates: the collector hive
and its fan-out half (#27), the session lifecycle and its handover, the llm
cell's seed path (#99), and the collector's own follow-ups #77, #91, #76 and
#103. 0.6.0 put the front door on it: the firewall hive (#36), one talky per
channel (#29), and window-mode memory requests as a real tool round (#78). What
is left of the epic is the talky/cogny split and what hangs off it.

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

## Later: memory hive continued

**The memory hive rests until after the showcase launch.** Nothing below is
abandoned and nothing below blocks the streams above; the hive is simply not
where the next measurable win is.

The stall had one carve-out: the three hive *bugs* (#52, #53, #73) shipped in
0.4.0, because a defect is not an enhancement. The enhancements keep waiting:
[#72](https://github.com/mmeyerlein/meclaw/issues/72) the extraction
lane claims 1.5x the turn count under sustained ingest,
[#88](https://github.com/mmeyerlein/meclaw/issues/88) a query hygiene guard for
recall, so a contaminated query cannot poison all legs,
[#51](https://github.com/mmeyerlein/meclaw/issues/51) extraction batch gate
defaults tuned for cost over freshness,
[#55](https://github.com/mmeyerlein/meclaw/issues/55) no consumer derives a
recall window, so time-range questions run as point recalls,
[#47](https://github.com/mmeyerlein/meclaw/issues/47) the async cell shutdown
drain.

## Alongside: surfaces and docs

New ways in and out, [#38](https://github.com/mmeyerlein/meclaw/issues/38)
voice ingress first (dictation-style now, realtime speech when the APIs land),
then [#39](https://github.com/mmeyerlein/meclaw/issues/39) the realtime HTML
window. The docs travel in two steps:
[#92](https://github.com/mmeyerlein/meclaw/issues/92) README and website on the
current release plus a short what's-next, then
[#93](https://github.com/mmeyerlein/meclaw/issues/93) the full rewrite once
meclaw-os lands, together with
[#43](https://github.com/mmeyerlein/meclaw/issues/43) the moving showcase demo
and a keyless quickstart.

## Ongoing: community templates

The good first issues from the first public wave — #3 and #4 — shipped with
0.5.0 as `retry@1` and `archive-bridge@1`. The template surface is open: a
template is a directory, a README and a `template.json`, and the nine under
`builder/templates/` are the worked examples — eight single-purpose ones plus
the `talky@1` composite that carries four of them as sub-units. New ones are
welcome.

## Shipped

One line per release; details in [CHANGELOG.md](CHANGELOG.md) and the
[GitHub releases](https://github.com/mmeyerlein/meclaw/releases).

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
