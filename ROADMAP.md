# Roadmap

The [issue tracker](https://github.com/mmeyerlein/meclaw/issues) is the single
source of truth for everything actionable. This file only orders it: what comes
next, what comes after, and why. Content lives in the issues, never here twice.
Milestones mirror these streams.

## Now: 0.1.x hardening

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
[#61](https://github.com/mmeyerlein/meclaw/issues/61) foreign template index re-anchors on boot.

1. [#58](https://github.com/mmeyerlein/meclaw/issues/58) CI: widen the 30s failure marker for the heaviest topology tests
2. [#57](https://github.com/mmeyerlein/meclaw/issues/57) remaining expects on the store wake path
3. [#59](https://github.com/mmeyerlein/meclaw/issues/59) DbConn: reconnect after a dropped call future

## Next: 0.2.0 memory quality

The identity pass for the memory hive. The canonicalization epic
[#12](https://github.com/mmeyerlein/meclaw/issues/12) leads and its open design
questions gate the plan; its sub-issues are
[#21](https://github.com/mmeyerlein/meclaw/issues/21)
[#22](https://github.com/mmeyerlein/meclaw/issues/22)
[#23](https://github.com/mmeyerlein/meclaw/issues/23)
[#24](https://github.com/mmeyerlein/meclaw/issues/24).
[#13](https://github.com/mmeyerlein/meclaw/issues/13) statement identity rides
the same design pass. Independent of the epic, in any order:
[#14](https://github.com/mmeyerlein/meclaw/issues/14) keyword morphology,
[#15](https://github.com/mmeyerlein/meclaw/issues/15) episode content dedup,
[#16](https://github.com/mmeyerlein/meclaw/issues/16) tier 0 window honesty.

## After: pre-MVP substrate

Substrate invariants that must land before an MVP claim, no order committed yet:
[#18](https://github.com/mmeyerlein/meclaw/issues/18) mailbox preservation on
panic, [#19](https://github.com/mmeyerlein/meclaw/issues/19) in-message blob
resolution, [#17](https://github.com/mmeyerlein/meclaw/issues/17) timer ops
reachable from the API,
[#20](https://github.com/mmeyerlein/meclaw/issues/20) secret materialization
classes, [#35](https://github.com/mmeyerlein/meclaw/issues/35) sandboxing for
code and bash cells,
[#62](https://github.com/mmeyerlein/meclaw/issues/62) provenance for
instantiated nodes.

## Then: meclaw-os, the organism

A colony grown from a seed into a personal operating system. The epic
[#26](https://github.com/mmeyerlein/meclaw/issues/26) leads and carries the settled principles and open forks; its
sub-issues in intended order:
[#27](https://github.com/mmeyerlein/meclaw/issues/27) collector hives as context orchestrators,
[#28](https://github.com/mmeyerlein/meclaw/issues/28) the talky/cogny split,
[#29](https://github.com/mmeyerlein/meclaw/issues/29) one talky per channel,
[#30](https://github.com/mmeyerlein/meclaw/issues/30) talky lifecycle,
[#31](https://github.com/mmeyerlein/meclaw/issues/31) per-talky memory,
[#32](https://github.com/mmeyerlein/meclaw/issues/32) one-file hives.
Riding the same wave: [#36](https://github.com/mmeyerlein/meclaw/issues/36) the firewall hive,
[#33](https://github.com/mmeyerlein/meclaw/issues/33) templates as the public app store,
[#34](https://github.com/mmeyerlein/meclaw/issues/34) a coding hive built with the builder.

## Alongside: surfaces

New ways in and out, [#38](https://github.com/mmeyerlein/meclaw/issues/38) voice ingress first (dictation-style now,
realtime speech when the APIs land), then [#39](https://github.com/mmeyerlein/meclaw/issues/39) the realtime HTML
window.

## Ongoing: community templates

Good first issues from the first public wave, order free:
[#3](https://github.com/mmeyerlein/meclaw/issues/3)
[#4](https://github.com/mmeyerlein/meclaw/issues/4)
[#5](https://github.com/mmeyerlein/meclaw/issues/5).
