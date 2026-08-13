# Roadmap

The [issue tracker](https://github.com/mmeyerlein/meclaw/issues) is the single
source of truth for everything actionable. This file only orders it: what comes
next, what comes after, and why. Content lives in the issues, never here twice.
Milestones mirror these streams.

## Shipped: 0.1.x hardening

Defects found by running the system in production, shipped as patch releases in
roughly this order. The two watchdog items travel together.

Done so far: [#10](https://github.com/mmeyerlein/meclaw/issues/10) fresh-clone tests, [#40](https://github.com/mmeyerlein/meclaw/issues/40) Windows build,
[#41](https://github.com/mmeyerlein/meclaw/issues/41) CI, [#42](https://github.com/mmeyerlein/meclaw/issues/42) releases with notes,
[#8](https://github.com/mmeyerlein/meclaw/issues/8) Slack full-operation deadlines,
[#54](https://github.com/mmeyerlein/meclaw/issues/54) tolerant memory-tag split,
[#56](https://github.com/mmeyerlein/meclaw/issues/56) seed files validated before boot,
[#6](https://github.com/mmeyerlein/meclaw/issues/6) watchdog armed after boot with a nonzero exit on trip,
[#7](https://github.com/mmeyerlein/meclaw/issues/7) I/O liveness marks on /health,
[#50](https://github.com/mmeyerlein/meclaw/issues/50) Slack idle deadline,
[#44](https://github.com/mmeyerlein/meclaw/issues/44) stderr warn line for successful scripts,
[#49](https://github.com/mmeyerlein/meclaw/issues/49) chat_id promotion pinned end to end,
[#9](https://github.com/mmeyerlein/meclaw/issues/9) embedding tokens in the books,
[#11](https://github.com/mmeyerlein/meclaw/issues/11) DbConn shutdown panic,
[#37](https://github.com/mmeyerlein/meclaw/issues/37) import/export matrix with the snapshot-versus-live spec section,
[#60](https://github.com/mmeyerlein/meclaw/issues/60) WAL sidecars documented as part of the restore unit,
[#61](https://github.com/mmeyerlein/meclaw/issues/61) foreign template index re-anchors on boot,
[#58](https://github.com/mmeyerlein/meclaw/issues/58) a wider marker for the child-process fixture,
[#57](https://github.com/mmeyerlein/meclaw/issues/57) a panic-free store wake path,
[#59](https://github.com/mmeyerlein/meclaw/issues/59) DbConn reconnects after a dropped call future,
[#63](https://github.com/mmeyerlein/meclaw/issues/63) a panic-free store respawn path.

The queue emptied with v0.1.16. New substrate defects found in production land
here first.

## Shipped: 0.2.0 memory quality

The identity pass for the memory hive, released as v0.2.0. One question under
all of it: when are two remembered things the same thing?

Done: [#21](https://github.com/mmeyerlein/meclaw/issues/21) subject
canonicalization at mint time,
[#22](https://github.com/mmeyerlein/meclaw/issues/22) predicate canonicalization
across paraphrase and language,
[#23](https://github.com/mmeyerlein/meclaw/issues/23) entity dedup with a fuzzy
index, [#24](https://github.com/mmeyerlein/meclaw/issues/24) a no-delete
migration for existing stores (epic
[#12](https://github.com/mmeyerlein/meclaw/issues/12)),
[#14](https://github.com/mmeyerlein/meclaw/issues/14) keyword morphology through
an index-time stemmer,
[#15](https://github.com/mmeyerlein/meclaw/issues/15) bundle-level episode
dedup, [#16](https://github.com/mmeyerlein/meclaw/issues/16) tier 0 window
honesty.

## Shipped: 0.3.0 statement identity

The follow-up pass on the same question, released as v0.3.0: once both versions
of a fact sit on one axis, which of them is still true?

Done: [#13](https://github.com/mmeyerlein/meclaw/issues/13) statement identity,
which moved the supersession unit down to
`(canonical_subject, canonical_predicate, canonical_claim)`, made every closure
an explicit attributed one (the nightly judge and the extractor in the turn,
both revertible by a single `where`), made cardinality a judged property of the
predicate with seed precedence and a session guard under the learned rule, added
judged claim aliases for rewordings, and put a currency marker on superseded
candidates in the bundle. Riding the same release:
[#18](https://github.com/mmeyerlein/meclaw/issues/18) mailbox preservation on
panic.

## Shipped: 0.3.1 memory quality follow-ups

Everything the track-end measurement of 0.3.0 named as a flank, fixed in one
pass and re-measured by one paid run at the end. The 5K case, the one wrong
answer that had survived the whole statement identity track, is right.

Done: [#66](https://github.com/mmeyerlein/meclaw/issues/66) the currency
question reaches the bucket axes it exists for, through a cardinality-first
triage and a paged currency question,
[#67](https://github.com/mmeyerlein/meclaw/issues/67) a predicate names the
subject matter and the intention moved onto the statement, so one fact and its
update land on one axis,
[#68](https://github.com/mmeyerlein/meclaw/issues/68) the vocabulary read has an
order and a bound, [#69](https://github.com/mmeyerlein/meclaw/issues/69) the
night renders only the sections that carry data,
[#64](https://github.com/mmeyerlein/meclaw/issues/64) every model call of a
night is booked with its tokens,
[#65](https://github.com/mmeyerlein/meclaw/issues/65) an end date is no longer
mirrored into an invalidity.

Found by the wave's own measurement and fixed inside it:
[#71](https://github.com/mmeyerlein/meclaw/issues/71) an extractor replacement
points forwards in time, never backwards.

## Shipped: 0.3.2 substrate reliability

Seven defects that all sit *between* cells, ranked by a practice run of the tool
cells against a test colony rather than by reading the code, fixed in one wave
and released as v0.3.2.

Done: [#75](https://github.com/mmeyerlein/meclaw/issues/75) a gateway error
inside a 200 body is classified at the wire, so an in-body 429 lands on the lane
a status 429 lands on, [#82](https://github.com/mmeyerlein/meclaw/issues/82) a
tool round is budgeted at the dozen routing hops it actually costs and a TTL
death names itself instead of stalling in silence,
[#83](https://github.com/mmeyerlein/meclaw/issues/83) a fetched body has a
`max_bytes` bound and a visible cut,
[#17](https://github.com/mmeyerlein/meclaw/issues/17) op bodies reach a cell over
the HTTP API and a scheduled lane can be triggered once from outside,
indistinguishably from its own tick,
[#84](https://github.com/mmeyerlein/meclaw/issues/84) the watchdog deadline is
reachable from `colony.json`, a trip names its evidence, and a trip can be
survivable without being invisible,
[#74](https://github.com/mmeyerlein/meclaw/issues/74) every scenario case binds a
port of its own and a red line carries the daemon's state,
[#20](https://github.com/mmeyerlein/meclaw/issues/20) instantiation keeps
environment placeholders literal on disk and binds them late, on every read.

The most useful result of the wave was a negative one: the boot failures three
earlier packages had blamed on port collisions were watchdog trips, and the
watchdog was measuring a legitimately long colony-loop iteration rather than a
hang. Two structural questions stay open on their own issues, a `ttl` refresh on
the loopback edge (#82) and an eviction policy for an assembled context (#83),
and so does the architectural cut a watchdog would need to tell a long iteration
from a hung one (#84).

## Shipped: 0.4.0 the bug-and-substrate wave

Every open bug on the tracker plus the pre-MVP substrate items, built on five
parallel tracks in isolated worktrees and merged one by one behind full gates,
released as v0.4.0.

Done: [#52](https://github.com/mmeyerlein/meclaw/issues/52) the batch lane skips
episodes inline extraction already covered, and an empty inline block counts as
a verdict, [#53](https://github.com/mmeyerlein/meclaw/issues/53) the inline
extraction contract ships with the hive and carries the batch lane's world-state
discipline, [#73](https://github.com/mmeyerlein/meclaw/issues/73) the nightly
identity questions see the spellings of recently closed rows, so a closure
across two spellings proposes the alias instead of hiding it,
[#79](https://github.com/mmeyerlein/meclaw/issues/79) a write into a missing
parent names the parent instead of the syscall,
[#80](https://github.com/mmeyerlein/meclaw/issues/80) fan-out edges guard their
keys with `has()` and a missing key logs at debug, so a healthy colony's log is
no longer mostly warnings, [#81](https://github.com/mmeyerlein/meclaw/issues/81)
the timer reads its op from a `tool_call` turn and acks it, making `remind` a
tool lane like `bash`, [#82](https://github.com/mmeyerlein/meclaw/issues/82) the
loopback edge may restore `ttl` as an explicit, condition-bound modifier, so the
example runs on the default budget again,
[#19](https://github.com/mmeyerlein/meclaw/issues/19) in-message blob pointers
resolve recursively with a depth limit and an all-or-nothing delivery,
[#62](https://github.com/mmeyerlein/meclaw/issues/62) instantiated nodes record
their template, version and instantiation time, re-indexed at every boot,
[#35](https://github.com/mmeyerlein/meclaw/issues/35) code and bash cells take a
sandbox profile: a Landlock filesystem view and a network-namespace deny,
fail-closed, with an explicit trusted escape hatch, and
[#90](https://github.com/mmeyerlein/meclaw/issues/90) an existing pre-v5
database migrates before the DDL batch runs, instead of dying on the new index.

The wave's sharpest lesson came from its only red gate: #90 was invisible to
every per-track run because fresh worktrees carry no old database — it took a
pre-v5 `colony.db` lying around the main tree to prove that every existing
colony would have died on upgrade. Follow-ups live on their own issues: sandbox
phase 2 ([#85](https://github.com/mmeyerlein/meclaw/issues/85)), the `system`
tree's unresolved `{text_id}` leaves
([#86](https://github.com/mmeyerlein/meclaw/issues/86)), `attachments[]` wiring
([#87](https://github.com/mmeyerlein/meclaw/issues/87)), and the boot probe's
verdict on edge-less topologies
([#89](https://github.com/mmeyerlein/meclaw/issues/89)).

## Shipped: 0.4.1 the pre-MVP finish line

The pre-MVP substrate stream is complete. Three tracks, one wave:
[#85](https://github.com/mmeyerlein/meclaw/issues/85) sandbox phase 2 — cgroup
v2 resource caps as an RAII scope that cleans up on every path, an in-tree
seccomp-bpf filter (ptrace, foreign-PID signals, raw sockets) with no new
dependency, the harness child under the same profile, and a prospective
default-deny for newly instantiated template-sourced cells;
[#86](https://github.com/mmeyerlein/meclaw/issues/86) `{text_id}` leaves in the
`system` tree resolve at the delivery boundary under one shared `text_id`
contract, and the llm cell's loud rejection is gone;
[#87](https://github.com/mmeyerlein/meclaw/issues/87) a cell that declares
`consumes.body.attachments` receives a read-only store handle, and the llm cell
is the first consumer: image attachments become vision content parts.
Follow-ups on their own issues:
[#94](https://github.com/mmeyerlein/meclaw/issues/94) attachment consumption on
the responses dialect, [#95](https://github.com/mmeyerlein/meclaw/issues/95)
pre-#86 persisted rows, [#96](https://github.com/mmeyerlein/meclaw/issues/96)
mcp/subcolony children still unsandboxed,
[#97](https://github.com/mmeyerlein/meclaw/issues/97) host capability probe,
[#98](https://github.com/mmeyerlein/meclaw/issues/98) an intermittent boot lock
under parallel test load.


## Now: meclaw-os, the organism

A colony grown from a seed into a personal operating system. This is the stream
now, because the gate for a public showcase is a provably better agent, not more
memory machinery. The epic
[#26](https://github.com/mmeyerlein/meclaw/issues/26) leads and carries the settled principles and open forks; its
sub-issues in intended order, collector hives first:
[#27](https://github.com/mmeyerlein/meclaw/issues/27) collector hives as context orchestrators,
[#28](https://github.com/mmeyerlein/meclaw/issues/28) the talky/cogny split,
[#29](https://github.com/mmeyerlein/meclaw/issues/29) one talky per channel,
[#30](https://github.com/mmeyerlein/meclaw/issues/30) talky lifecycle,
[#31](https://github.com/mmeyerlein/meclaw/issues/31) per-talky memory,
[#32](https://github.com/mmeyerlein/meclaw/issues/32) one-file hives.
Riding the same wave: [#36](https://github.com/mmeyerlein/meclaw/issues/36) the firewall hive,
[#33](https://github.com/mmeyerlein/meclaw/issues/33) templates as the public app store,
[#34](https://github.com/mmeyerlein/meclaw/issues/34) a coding hive built with the builder.

## Done: pre-MVP substrate

The stream is complete: #17 and #20 shipped in 0.3.2, #19, #35 (phase 1) and
#62 in 0.4.0, and #85, #86, #87 closed it in 0.4.1. What remains of it are the
named follow-ups listed under the 0.4.1 section above; new substrate work joins
the hardening stream at the top.

Suite hygiene rides in this stream because it is infrastructure rather than
product. [#74](https://github.com/mmeyerlein/meclaw/issues/74) shipped in 0.3.2:
the port rotation is gone, and the diagnosis that came with it showed that the
boot failures blamed on TIME_WAIT were watchdog trips
([#84](https://github.com/mmeyerlein/meclaw/issues/84)).

## Later: memory hive continued

**The memory hive rests until after the showcase launch.** Nothing below is
abandoned and nothing below blocks the streams above; the hive is simply not
where the next measurable win is.

The stall had one carve-out: the three hive *bugs* (#52, #53, #73) shipped in
0.4.0, because a defect is not an enhancement. The enhancements keep waiting:
[#72](https://github.com/mmeyerlein/meclaw/issues/72) the extraction
lane claims 1.5x the turn count under sustained ingest,
[#51](https://github.com/mmeyerlein/meclaw/issues/51) extraction batch gate
defaults tuned for cost over freshness,
[#55](https://github.com/mmeyerlein/meclaw/issues/55) no consumer derives a
recall window, so time-range questions run as point recalls,
[#47](https://github.com/mmeyerlein/meclaw/issues/47) the async cell shutdown
drain.

## Alongside: surfaces

New ways in and out, [#38](https://github.com/mmeyerlein/meclaw/issues/38) voice ingress first (dictation-style now,
realtime speech when the APIs land), then [#39](https://github.com/mmeyerlein/meclaw/issues/39) the realtime HTML
window.

## Ongoing: community templates

Good first issues from the first public wave, order free:
[#3](https://github.com/mmeyerlein/meclaw/issues/3)
[#4](https://github.com/mmeyerlein/meclaw/issues/4)
[#5](https://github.com/mmeyerlein/meclaw/issues/5).
