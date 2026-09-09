# Prepared for self-improvement

Does a colony improve itself? The primitives for it are here and tested, and nothing closes them
into an unattended loop. That is a decision, not a gap in the schedule.

## The primitives that exist

A mutation is a diff POSTed to `/colony/mutations`, carrying eight operations and no others
(`DIFF_OPERATIONS` in `crates/meclaw-colony/src/mutation/validate.rs`). A diff key no operation
reads is refused rather than ignored, a refusal names one of 28 stable `error_code` strings, and
it is raised before anything is staged or spawned. A rejected mutation leaves nothing behind, and
every submission lands in `colony.db::mutation_log`
(`crates/meclaw-colony/tests/gh276_rejected_mutation_leaves_no_residue.rs`).

An agent goes through that same door. The shipped `builder` drafts manifests and holds no edge
onto `/colony/mutations`, `submit` is the one node in the tree that has one, and a mutation cannot
draw the missing edge, because that address is absent from the endpoints a diff may name
(`crates/meclaw-cells/tests/gh425_the_builder_cannot_reach_the_mutation_door.rs`). `submit` checks
that the manifest's bytes are the ones its digest was drawn over, reads the requester off the
envelope and asks the capability broker. A manifest that wants to author executable behaviour, a
script override or an `add_templates`, is a second question (`code.author`).

`argus` is the control loop, a hive of seven cells. It reads a charter, measures its colony out of
the substrate's ledger, has a model judge against those numbers, sends the decision to a named
cell as a params update, then measures the effect and keeps or reverts it. A cycle without a
pre-authored revert plan is refused with `no_revert_plan`, and every tick leaves an append-only
receipt (`templates/argus/template.json`).

## Why nothing closes the loop

Nothing in that hive authors a diff ([#304](https://github.com/mmeyerlein/meclaw/issues/304)), so
argus can move a parameter on a cell somebody wired an edge to and can never add, remove or rewire
one. Both goal rows in `templates/argus/charter/seed/goals.jsonl` ship with `"enabled": 0`, so a
freshly grown loop ticks, receipts each tick as idle and changes nothing until an operator turns a
row on.

A self-improving loop you cannot audit is an incident with a delay on it. The properties a
defensible one needs sit in the substrate rather than in a prompt: one recorded door, a second
question when a diff wants to author code, measurement drawn from a ledger the loop cannot write,
and revert as an outcome the charter demands up front.

## Where to read on

- [ontology](ontology.md) for the catalogue a builder drafts against, and
  [`rewiring.md`](../rewiring.md) for what a diff may say
