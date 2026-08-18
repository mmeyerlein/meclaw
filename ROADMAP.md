# Roadmap

The [issue tracker](https://github.com/mmeyerlein/meclaw/issues) is the single
source of truth for everything actionable. This file only orders it: what comes
next, what comes after, and why.

Three rules keep it from silting up, because it has:

- **A stream names open issues only.** Work that shipped leaves the stream and
  appears once, as one line, under [§ Shipped](#shipped).
- **No content lives here twice.** The issue carries the detail; this file
  carries the ordering and the reason.
- **A claim about an issue's state is checked, not remembered.** The last pass
  found the file asserting an issue was closed while it was open for another two
  weeks.

Milestones mirror these streams. Release detail is in
[CHANGELOG.md](CHANGELOG.md) and the
[GitHub releases](https://github.com/mmeyerlein/meclaw/releases).

## Now: meclaw-os, the organism

A colony grown from a seed into a personal operating system. This is the stream,
because the gate for a public showcase is a provably better agent rather than
more memory machinery. The epic
[#26](https://github.com/mmeyerlein/meclaw/issues/26) leads and carries the
settled principles and the open forks.

Its first half shipped as templates across 0.5.0–0.7.0 (collector hives, the
session lifecycle, one talky per channel, the firewall, the advisor split, the
memory drain), and 0.9.0 made the memory a public building block. The example
that proves the claim end to end is
[`examples/meclaw-os`](examples/meclaw-os/): an empty seed, one declaration,
seventeen cells.

What is left of the epic:

- [#31](https://github.com/mmeyerlein/meclaw/issues/31) each talky owns an
  internal memory hive
- [#32](https://github.com/mmeyerlein/meclaw/issues/32) one-file hives, a hive
  per document
- [#30](https://github.com/mmeyerlein/meclaw/issues/30) talky lifecycle, a fresh
  instance per closed day — with
  [#102](https://github.com/mmeyerlein/meclaw/issues/102) as its experiment
- [#122](https://github.com/mmeyerlein/meclaw/issues/122) information ownership:
  who holds what, and how an asker finds the holder
- [#124](https://github.com/mmeyerlein/meclaw/issues/124) the first thing the
  advisor measured about itself — a consult round trip is far too slow for a
  question memory could have answered
- [#33](https://github.com/mmeyerlein/meclaw/issues/33) templates as the public
  app store, and [#34](https://github.com/mmeyerlein/meclaw/issues/34) a coding
  hive built with the builder

## Next: the channel round

One talky per channel, the connectors moved out of `access` into channel hives of
their own, `access` left holding a vault. It was blocked on three rulings; all
three are in as of 2026-08-18, and what is left is work rather than decisions.

The round is now four items, and they are ordered because the first one settles
how the other three are built:

- ~~[#197](https://github.com/mmeyerlein/meclaw/issues/197) the four
  cell-named-port hives get migrated, not aliased~~ — **done 2026-08-18.**
  `canvy`, `access`, `steward` and `memory-hive` are `ports: []` plus doors plus
  a `params.contract` in lanes named for what a caller wants. A port alias would
  have kept the deep-endpoint address shape, which is the one thing the boundary
  exists to remove.
- ~~[#228](https://github.com/mmeyerlein/meclaw/issues/228) fourteen shipped
  templates declare no `params.ports` at all~~ — **done 2026-08-18.** With
  those fourteen the library has no unsealed hive template left, and the
  canonical instantiation example points at a hive path with a lane rather than
  at an inner hive.
- [#185](https://github.com/mmeyerlein/meclaw/issues/185) **a cell declares that
  it is an ingress.** Ruled: option (a). Today it is deduced from having no
  incoming edge, which stops being true the moment the check can see the running
  graph — and a proxy in a channel hive is exactly the shape that breaks. This
  is a **breaking** change to the cell contract and wants to land with the rest
  of the round rather than alone.
- [#176](https://github.com/mmeyerlein/meclaw/issues/176) **a failure lane leaks
  `hop.finish_reason` across its own boundary.** A provider's word for why a
  completion stopped, in a hive's outward contract. Since the boundary rule is
  now normative this is a violation rather than a wart, but the fix changes what
  an error sink receives, and error paths are where a wrong turn burns a
  provider call per round until TTL. Deliberate, with a test in front of it.

The rule all four serve is written down in
[`docs/meclaw-overview.md`](docs/meclaw-overview.md) § The hive boundary, and
what it asks of a template author is in
[`templates/README.md`](templates/README.md) § The hive boundary. The procedure for doing one
without knocking a colony over is [`docs/rewiring.md`](docs/rewiring.md) §
Putting an existing hive behind its boundary, including the two parts that bite —
a topology lives in four places, not two, and innermost first.

## Next: substrate flanks

Named flanks left by the pre-MVP waves, plus new findings from running the thing.

- [#46](https://github.com/mmeyerlein/meclaw/issues/46) the harness permission
  surface and error-code semantics
- [#45](https://github.com/mmeyerlein/meclaw/issues/45) the code cell's 16 ms
  interpreter start, which dominates serial hot paths
- [#47](https://github.com/mmeyerlein/meclaw/issues/47) the async cell shutdown
  drain: in-flight work is lost on EOF and child termination
- [#48](https://github.com/mmeyerlein/meclaw/issues/48) measuring what a
  subscription plan actually carries until reset
- [#111](https://github.com/mmeyerlein/meclaw/issues/111) guarding interpreter
  bytecode caches in the coding templates' edit-test loops
- [#130](https://github.com/mmeyerlein/meclaw/issues/130) natural-language model
  selection and closed-loop automation, beyond the v1 catalogue
- [#165](https://github.com/mmeyerlein/meclaw/issues/165) the heartbeat watchdog
  cannot tell a wedged colony from a starved host, and the trip is fatal — a
  compile on the same box killed a healthy colony three times in one day
- [#232](https://github.com/mmeyerlein/meclaw/issues/232) a workshop fixture
  still wires at an interior address, producing 11 `hive_no_route` dead letters
  per run

Two of these are watches rather than fixes, and stay open on purpose:

- [#141](https://github.com/mmeyerlein/meclaw/issues/141) message headers are
  unbounded by design — measured every few weeks rather than capped
- [#138](https://github.com/mmeyerlein/meclaw/issues/138) the environment knobs
  are a declared **experimental** surface; roughly 79 remain across the shipped
  templates and migrate to params one template at a time, defaults bit-identical.
  Order: `memory-hive`, `talky`, `cogny`, then the small ones

## Later: memory, after the measurement

The memory hive shipped publicly in 0.9.0, test suite included. What replaces the
old list here is one measurement: a 50-question LongMemEval run against the 0.9.0
tree returned **96 % R@5 and 58 % end accuracy** — in 19 of 21 wrong answers the
retrieval had delivered the gold session and the synthesis failed to answer from
it. The sharpest case scored 100 % R@5 against 30.8 % accuracy.

- [#148](https://github.com/mmeyerlein/meclaw/issues/148) is therefore where this
  stream points: the bottleneck is not the remembering. 0.10.3 took the first of
  its three measures — tier 2 receives the candidates grouped by session
  alongside the flat ranking, and the prompt says how to aggregate. The two
  remaining measures are untouched, because both change what is retrieved or how
  often the model runs, and the gate for either is a benchmark run rather than a
  test.
- [#55](https://github.com/mmeyerlein/meclaw/issues/55) no consumer ever derives
  a recall window, so time-range questions run as point recalls

## Alongside: surfaces and docs

New ways in and out. [#38](https://github.com/mmeyerlein/meclaw/issues/38) voice
ingress first — dictation-style now, realtime speech when the APIs land — then
[#39](https://github.com/mmeyerlein/meclaw/issues/39) the realtime HTML window.
[#43](https://github.com/mmeyerlein/meclaw/issues/43) is down to its last piece:
the keyless quickstart and the annotated message trace shipped with 0.9.0, the
*moving* demo did not.

The canvas landed across 0.11.0–0.14.0 and its remaining two are both about a
picture disagreeing with the truth:

- [#167](https://github.com/mmeyerlein/meclaw/issues/167) a newly instantiated
  cell is placed by the layout engine and can land on top of a hand-placed one —
  the two sets of positions are never reconciled
- [#172](https://github.com/mmeyerlein/meclaw/issues/172) after a colony restart
  the page keeps showing the old picture until the operator touches something,
  because a transport join is not a render

Two doc gates are open, and both are the same lesson twice:

- [#229](https://github.com/mmeyerlein/meclaw/issues/229) the port-address gate
  does not scan `docs/`, which is how the specification kept an address the
  boundary refuses — in the section defining the rule it broke. Extending it
  needs a marker for deliberate counter-examples, because the section now
  contains some on purpose
- [#230](https://github.com/mmeyerlein/meclaw/issues/230) whether a receipt's
  comments may be corrected after the fact

## Ongoing: community templates

The template surface is open: a template is a directory, a README and a
`template.json`. Sixteen are listed in
[`templates/README.md`](templates/README.md) as worked examples — thirteen
single-purpose ones plus three composites: `talky@3.0.0`, which carries four of
them as sub-units, `cogny@3.0.0`, which carries two, and `memory-hive@2.0.0`, the
agent memory as a hive of ten cells. `canvy@0.3.0` ships as well.

New ones are welcome. What a hive template has to satisfy is
[`templates/README.md`](templates/README.md) § The hive boundary — it is a
requirement, not a convention.

## Shipped

One line per release; details in [CHANGELOG.md](CHANGELOG.md) and the
[GitHub releases](https://github.com/mmeyerlein/meclaw/releases).

- **v0.14.0 — a name means one thing.** A migration that put every hive behind
  its boundary walked into the mutation validator's oldest assumption and left
  twelve defects behind, three of them destructive. One function decides what a
  diff name means now. Plus `move_nodes`, a machine-readable hive contract, a
  seedable `hop` at both ingresses, one rule for what the boot topology is, and a
  canvas arrangement that survives a cell being added.
- **v0.13.0 — the canvas keeps its stylesheet's word.** It was correct and
  unreadable: the CSS had promised arrowheads, selection and hive labels since
  the first version and the markup never emitted them. Half the release is not
  new work, it is the picture finally saying what the tree looks like.
- **v0.12.3 — hive in hive.** A frame was derived from a cell's direct parent
  only, so nested hives were drawn as unrelated rectangles side by side and a
  hive of nothing but sub-hives got no frame at all. The layout is recursive now.
- **v0.12.2 — a hive can be dragged, and it costs one row.** The empty space
  inside a frame is the handle; frame, label and every cell move together, and
  the release writes one store row for the group whatever its size.
- **v0.12.1 — three defects found by opening the page.** The canvas offered the
  client nothing to attach to (a LiveView hook needs `phx-hook` *and* an `id`),
  so no edges and no drag — and the fourth item is that finding itself: a client
  path cannot be proven over the websocket.
- **v0.12.0 — a surface installs into a colony that is already running.** One
  mutation, no restart. Two rules moved for it: the egress door is no longer a
  place, and `/colony/graph` is drawable by a mutation. Database isolation lost
  its last exception in the same release.
- **v0.11.1 — a hive's height is its own flow depth.** The flow layer was
  computed across the whole colony and applied inside one hive, so two cells in
  the same hive could sit 395 empty rows apart. Measured on a live colony:
  174828 px of vertical extent became 3384 px.
- **v0.11.0 — a colony serves surfaces over HTTP.** A cell may declare
  `cell.surface` and is then served under its own cell path — page, transport
  and assets under one URL prefix, so a single nginx location block authorises
  all three.
- **v0.10.7 — a liveness check that perturbs what it measures is not one.**
  The 0.10.6 repair of a flaky test was worse than the flake; the Monday cron
  caught it within hours. The assertion is removed, not replaced.
- **v0.10.6 — the last two spawn sites are decided.** The `mcp` child reads a
  sandbox profile; the `subcolony` child deliberately does not, and both halves
  are written down. Plus a unit test that stopped asserting against a clock.
- **v0.10.5 — latency you can read off the log.** A read-only tool that answers
  "how slow is this lane" from the colony's own message log, consult eta hints
  that follow what it measured instead of what somebody hoped, and a proxy test
  that waits for the poll instead of for a cycle.
- **v0.10.4 — the cells inside a subtree can be parameterised at birth.**
  `override_params` on a subtree template is addressed by the cells' paths
  inside it. R10's protection stays: a key that addresses nothing is refused,
  and the refusal lists what the template does contain.
- **v0.10.3 — tier 2 sees sessions.** The multi-session synthesis gap (100 %
  retrieval, 30.8 % accuracy) gets the first of its three measures: the
  candidates arrive grouped by conversation, oldest first, and the prompt says
  how to aggregate over them. A shape fix; the gate is a benchmark run.
- **v0.10.2 — a wired port must have its drain.** A hive can declare which of
  its ports come in pairs, and a mutation that wires the ingress without the
  egress is refused rather than quietly opening a lane that loses messages. The
  check asks the router, not the condition's spelling.
- **v0.10.1 — an edge can be replaced again.** `remove_edges` now runs before
  `add_edges`, so dropping a lane and adding its widened replacement in ONE
  mutation does what it reads like. The other way round it deleted its own new
  edge and reported success.
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
