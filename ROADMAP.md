# Roadmap

The [issue tracker](https://github.com/mmeyerlein/meclaw/issues) is the single
source of truth for everything actionable. This file only orders it: what comes
next, what comes after, and why. Content lives in the issues, never here twice.
Milestones mirror these streams.

## Now: 0.1.x hardening

Defects found by running the system in production, shipped as patch releases in
roughly this order. The two watchdog items travel together.

1. [#10](https://github.com/mmeyerlein/meclaw/issues/10) fresh clone: cargo test must be green without the private tree
2. [#8](https://github.com/mmeyerlein/meclaw/issues/8) Slack proxy: full-operation deadline for the long poll
3. [#6](https://github.com/mmeyerlein/meclaw/issues/6) watchdog: no arming during boot, nonzero exit on trip
4. [#7](https://github.com/mmeyerlein/meclaw/issues/7) cell I/O liveness signal (the watchdog item's other half)
5. [#9](https://github.com/mmeyerlein/meclaw/issues/9) embedding calls in token accounting
6. [#11](https://github.com/mmeyerlein/meclaw/issues/11) rare shutdown panic in DbConn

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
classes.

## Ongoing: community templates

Good first issues from the first public wave, order free:
[#3](https://github.com/mmeyerlein/meclaw/issues/3)
[#4](https://github.com/mmeyerlein/meclaw/issues/4)
[#5](https://github.com/mmeyerlein/meclaw/issues/5).
