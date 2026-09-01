# Recursive self-improvement — seriously?

Short version: **the primitives are here and tested; the loop is not — deliberately.**
Both halves of that sentence are load-bearing, so this page states exactly what exists
and exactly what does not.

## What exists

Self-improvement decomposes into four capabilities, and each one is shipped, tested, and
usable today:

1. **Runtime mutation.** A cell can emit a diff; the colony validates and applies it
   atomically while everything keeps running. Topology change is an operation, not a
   deployment.
2. **The builder.** A structural wish becomes a manifest without a hand on a keyboard —
   rendered deterministically where a recipe exists, designed against the typed catalogue
   where one does not ([Ontology](ontology.md)).
3. **Keep-or-revert.** `argus`, the control loop, runs a charter: a deterministic
   measurement out of the colony's own ledger, a judge that simulates before it decides,
   one parameter changed, health checked, and the change reverted if the window
   disagrees.
4. **Receipts.** Every mutation is in the ledger; every argus tick writes a receipt,
   including the tick that had nothing to do and the cycle that failed — because a loop
   that only writes down its successes is a loop nobody can audit.

A colony can therefore already *extend itself on request*: an agent wishes, the builder
drafts, a human (or a policy) says yes to a digest, the tree grows — while it runs.

## What does not exist

There is **no process in this repository that closes the circle** — no component that
observes, decides, mutates and repeats unattended. Every goal `argus` could pursue ships
**disabled**. A fresh colony measures nothing, changes nothing, and improves nothing
until an operator turns on exactly what they mean.

This is a position, not a gap in the schedule: **no blind RSI.** A self-improving loop
you cannot audit is an incident with a delay on it.

## What closing the loop would take

The honest reason the primitives came first: a defensible loop needs properties that must
live in the substrate, not in the loop's prompt —

- every change through one door, attributable, with a receipt (shipped);
- approval classified by *effect*, so an inert new subtree may auto-approve while
  rerouting live traffic escalates to a human (shipped);
- measurement the loop cannot flatter, drawn from the ledger rather than self-report
  (shipped);
- revert as a first-class outcome, not an apology (shipped).

What remains is the part that should remain: deciding *which* goals a colony may pursue
about itself, and turning them on. That switch belongs to an operator, and it ships off.
