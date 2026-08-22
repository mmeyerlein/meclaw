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

Two of its sub-issues closed in August without being built, because a design
round answered them: a talky generation ends when the **participant set** changes
rather than at midnight, and what keeps one room's talk out of another is the
audience an entry carries, not a memory store per channel. The memory half of
that rule shipped in 0.16.0; the topology half stays with #122 below.

What is left of the epic:

- [#32](https://github.com/mmeyerlein/meclaw/issues/32) one-file hives, a hive
  per document
- [#122](https://github.com/mmeyerlein/meclaw/issues/122) information ownership:
  who holds what, and how an asker finds the holder
- [#124](https://github.com/mmeyerlein/meclaw/issues/124) the first thing the
  advisor measured about itself — a consult round trip is far too slow for a
  question memory could have answered
- [#33](https://github.com/mmeyerlein/meclaw/issues/33) templates as the public
  app store, and [#34](https://github.com/mmeyerlein/meclaw/issues/34) a coding
  hive built with the builder

## Next: substrate flanks

Named flanks left by the pre-MVP waves, plus new findings from running the thing.

- [#45](https://github.com/mmeyerlein/meclaw/issues/45) the code cell's 16 ms
  interpreter start, which dominates serial hot paths
- [#47](https://github.com/mmeyerlein/meclaw/issues/47) the async cell shutdown
  drain: in-flight work is lost on EOF and child termination
- [#48](https://github.com/mmeyerlein/meclaw/issues/48) measuring what a
  subscription plan actually carries until reset
- [#267](https://github.com/mmeyerlein/meclaw/issues/267) `steward` reads
  `colony.db` directly, which the spec has forbidden without exception since
  #160 — the data it needs (cost, errors, dead letters, whether a mutation
  committed) has no sanctioned route today, so this is a decision about the rule
  rather than a repair
- [#130](https://github.com/mmeyerlein/meclaw/issues/130) natural-language model
  selection and closed-loop automation, beyond the v1 catalogue

Two flanks closed since this list was written, both by measurement rather than
argument. [#46](https://github.com/mmeyerlein/meclaw/issues/46): the CLI never
sends a `can_use_tool` request under the adapter's current invocation, and
`permission_mode` is no ceiling either — with `--allowedTools` omitted entirely,
`Bash` still ran. The reliable bounds stay `env_clear` plus a passthrough
allow-list, the canonicalised cwd clamp, and `params.sandbox`; whether the
adapter gains `--input-format stream-json` or a first-class tool-ceiling
parameter are open decisions, and neither will be guessed into production code.
[#111](https://github.com/mmeyerlein/meclaw/issues/111) shipped the half that
exists: a cookbook note on interpreter bytecode caches plus `python3 -B` in the
coder-pipeline examples. The substrate half has nowhere to live — a `bash`
cell's params know `max_concurrency`, `external_timeout_ms`, `max_bytes` and
`sandbox`, and no environment key.

Two of these are watches rather than fixes, and stay open on purpose:

- [#141](https://github.com/mmeyerlein/meclaw/issues/141) message headers are
  unbounded by design — measured every few weeks rather than capped
- [#138](https://github.com/mmeyerlein/meclaw/issues/138) the environment knobs
  are a declared **experimental** surface; 103 behavioural knobs remain across the
  shipped templates — 107 of the 131 occurrences sit in `script_inline` — and they
  migrate to params one template at a time, defaults bit-identical. Order:
  `memory-hive` (51, of which `recall` alone holds 25), then the small ones;
  `talky` and `cogny` own none of their own, theirs come from their sub-units.
  The count grew by three in 0.17.3 rather than shrinking: the fusion floors of
  [#297](https://github.com/mmeyerlein/meclaw/issues/297) arrived as env knobs
  like their neighbours, all three in `recall`

## Later: memory, after the measurement

The memory hive shipped publicly in 0.9.0, test suite included. What replaces the
old list here is one measurement: a 50-question LongMemEval run against the 0.9.0
tree returned **96 % R@5 and 58 % end accuracy** — in 19 of 21 wrong answers the
retrieval had delivered the gold session and the synthesis failed to answer from
it. The sharpest case scored 100 % R@5 against 30.8 % accuracy.

- [#148](https://github.com/mmeyerlein/meclaw/issues/148) is therefore where this
  stream points: the bottleneck is not the remembering. Its first measure — the
  shape of what tier 2 is handed — has had two instalments: 0.10.3 grouped the
  candidates by session alongside the flat ranking and said how to aggregate,
  and 0.17.3 rewrote the document itself (a header that says what it is,
  separate `FACTS` and `WHAT WAS SAID` sections, the run's bookkeeping moved out
  into `recall_diagnostic`). Neither instalment has been measured against end
  accuracy. The other two measures are untouched — whether the tier-1 cap
  truncates the *set* a multi-session answer needs, and whether `dialectic`
  earns a second pass on questions that count, compare or span — because both
  change what is retrieved or how often the model runs, and the gate for either
  is a benchmark run rather than a test. The 0.17.3 fusion measurement does not
  close any of this: it was retrieval-only and paired, and it says only that the
  new relevance floors cost nothing.
- [#55](https://github.com/mmeyerlein/meclaw/issues/55) the recall window has a
  producer since #78, but no shipped composite carries the tool that drives it,
  so time-range questions still run as point recalls
- [#261](https://github.com/mmeyerlein/meclaw/issues/261) the memory porter
  predates the substrate's `transfer` slot and now duplicates four things it does
  natively — a hand-maintained schema mirror, an idempotence probe, a provenance
  name-list and whole-part atomicity. What stays template-level is the walk over
  the fifteen tables. Not urgent: the porter works, and this is the one component
  where a member's history is at stake, so it earns a slot of its own rather than
  a place at the end of a wave

## Alongside: surfaces and docs

New ways in and out. [#38](https://github.com/mmeyerlein/meclaw/issues/38) voice
ingress first — dictation-style now, realtime speech when the APIs land — then
[#39](https://github.com/mmeyerlein/meclaw/issues/39) the realtime HTML window.
[#43](https://github.com/mmeyerlein/meclaw/issues/43) is down to its last piece:
the keyless quickstart and the annotated message trace shipped with 0.9.0, the
*moving* demo did not.

## Ongoing: community templates

The template surface is open: a template is a directory, a README and a
`template.json`. Nineteen are listed in
[`templates/README.md`](templates/README.md) as worked examples — sixteen
single-purpose ones plus three composites: `talky@3.0.13`, which references four
of them as sub-units, `cogny@3.0.10`, which references two, and
`memory-hive@2.3.1`, a member's long-term memory as a hive of twelve cells.

New ones are welcome. What a hive template has to satisfy is
[`templates/README.md`](templates/README.md) § The hive boundary — it is a
requirement, not a convention.

What a shipped template promises and what it does drift apart silently, because
nothing recomputes the promise. 0.17.0 closed six such findings and swept the
`not_in_scope` field of all 34 templates against the code, which turned up eleven
more sentences that were simply false. What is left:

- [#272](https://github.com/mmeyerlein/meclaw/issues/272) every user turn of one
  drained batch is attributed to the same speaker, because `context` is per
  message and a batch is a day. The audience gate holds and nothing leaks; what
  breaks is who said what *inside* the room, which is the distinction a
  multi-person channel exists to make. A wrong identity is worse than an absent
  one — nothing downstream can tell a filled column from a correct one
- [#254](https://github.com/mmeyerlein/meclaw/issues/254) the general form of the
  same problem, one level up: a review checks code against spec, so a spec that
  is ahead of the code passes every review. The audit sorts every claim about
  behaviour into built-and-pinned, built-and-unpinned, or described-and-absent

## Shipped

One line per release; details in [CHANGELOG.md](CHANGELOG.md) and the
[GitHub releases](https://github.com/mmeyerlein/meclaw/releases).

- **v0.17.4 — a refusal stops arriving as a result.** A `store` error reply was
  read as an empty result set by the lanes that consume it, so a failed read
  looked like "nothing is there" and a failed write like a success; every lane
  that talks to a `store` checks `error_code` before it reads rows now, which
  closes [#343](https://github.com/mmeyerlein/meclaw/issues/343) on the nine
  small ones too. The same class in other places: a `/colony` read dropped a
  filter it could not parse and answered with the unfiltered set (all four
  refuse with `invalid_query` now), a builder deployment reported a mutation the
  colony had refused, a failed rescan let the run die one cell later instead of
  stopping there, and a `bash` command of 128 KiB or more reported an I/O fault
  rather than its size. The builder hands back its scope lease and its in-flight
  marker together at the end of every lane, never one without the other.
  **Breaking:** an unknown key in a `cell` block refuses the mutation the way it
  has always refused the boot ([#353](https://github.com/mmeyerlein/meclaw/issues/353));
  the migration is to remove the key. Plus the export audit runs the published
  tree's gates before the publication rather than after, and the librarian's
  corpus carries whole sections instead of their first 4000 characters.
- **v0.17.3 — a template can put another template inside itself.** `cell.type:
  "ref"` names a template as a sub-unit, so `talky` and `cogny` reference the
  four and the two units they used to carry as byte copies, and a cell records
  which template **it** came from plus the composites that placed it
  (`registry.template_chain`, `colony.db` schema v6). A template also declares
  what it needs — `requires.ctx` / `requires.env`, checked before the first byte
  is staged — an `override_params` key must name a param the target cell really
  has, and a refused mutation names every violation of the stage that refused it
  instead of only the first. On the memory side the tier-1 recall becomes two
  documents in one message: a bundle that says what memory holds, and
  `recall_diagnostic` beside it for the retrieval's own bookkeeping; the ambient
  leg arrives as a `memory_recall` tool pair in the round rather than as durable
  state in `system.memory`. And the defect that had made every live recall
  invisible: a `code` cell whose `script_inline` crossed 128 KiB never spawned at
  all.
- **v0.17.2 — the error paths keep the contract's word.** A `store` error reply
  stamps `hop.operation` like every other reply, a contract that consists of
  nothing but `consumes.topology.inbound_edges` stops being counted as vacuous
  and losing its capability at spawn, and `code` refuses
  `external_timeout_ms: 0` the way its three siblings always have. A row of
  shipped templates says what it does again — the `steward`'s revert is checked
  like its outbound change, and its probe asks about the mechanism the loop
  actually uses. Nothing in it hands a caller anything that was not already
  promised, which is why the third digit moved.
- **v0.17.1 — the night the audit was answered.** Twenty-two findings of the
  2026-08-20 consistency audit, twelve of them shipped templates whose documents,
  addresses or numbers had drifted from what their code does. A rejected mutation
  leaves no registered cell any more (#276), a cell may declare that its database
  does not travel (`contract.transfer: "none"`, which is what shut the vault's
  `transfer` leak, #314), and the `steward` loop can commit for the first time in
  its life (#304). Plus three gates against the class itself: a spec-claims
  registry, an anchor per accepted ADR, and the builder-scenario suite as an
  export gate.
- **v0.17.0 — content can leave a cell and enter a running one.** The `transfer`
  body slot serves all eight cell types with a `cell.db` from the substrate, with
  no per-type code — export an inventory or a document, import into a **running**
  cell under the memory porter's rules. `memory-hive@2.2.1` is the same answer
  one level up, with `in_export`/`in_import` lanes and a twelfth interior cell.
  And a `system.*` subtree can be revoked rather than only overwritten
  (`"$replace": true`), which a writer with data-keyed sub-paths never could.
- **v0.16.0 — a fact remembers who was there.** `memory-hive@2.1.0` records the
  participant set a turn was said in front of, and the recall answers only with
  rows the current round could have heard — the subset rule `affinity` has used
  since #154, now on both halves of one rule. Derived rows get the intersection
  of their sources, so two private facts cannot be laundered into one shareable
  claim. Fail-closed on both sides: a lane without an audience refuses rather
  than writing or answering. And where the gate costs certainty it says so —
  a claim whose successor is invisible is marked rather than presented as
  current.
- **v0.15.1 — a sealed hive insists on its drain again.**
  `params.required_drains` can name a lane and not only a port, so the one
  guarantee `memory-hive` lost by sealing itself exists again in the vocabulary
  the boundary leaves standing (`memory-hive@2.1.0`). Plus an address scan that
  matches a template's name whole or not at all.
- **v0.15.0 — every shipped hive is behind its boundary.** The four templates
  whose ports carried the name of a cell inside, and the fourteen that declared
  no ports at all: a lane is named for what the caller wants, never for where it
  lands. The failure lane stops carrying `hop.finish_reason` across the boundary,
  and the contract check learned the case it was blind to — a door that produces
  a lane is an exit for it.
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
