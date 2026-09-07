# Self-modification, and what is missing from it

A running colony changes through one door, and every change through it is written down. No
process here walks through that door on its own: the loop that would observe, decide, mutate
and repeat unattended is open, and its goals ship switched off.

## The door

A mutation is a diff POSTed to `/colony/mutations`. It carries eight operations and no others:
`add_templates`, `add_nodes`, `remove_nodes`, `swap_nodes`, `move_nodes`, `add_edges`,
`remove_edges`, `seed_rows` (`DIFF_OPERATIONS` in
`crates/meclaw-colony/src/mutation/validate.rs`). A diff key no operation reads is refused
instead of ignored, a refusal names one of 28 stable `error_code` strings, and it is raised
before anything is staged, spawned or wired. A rejected mutation leaves nothing behind, which
is what `crates/meclaw-colony/tests/gh276_rejected_mutation_leaves_no_residue.rs` holds. Every
submission is written to `colony.db::mutation_log` with its scope, payload, status and failure
reason, and `GET /colony/mutations` reads the log back. A committed diff answers 200, a
rejected one 422, and neither restarts the process.

## What an agent can do with it

An agent uses that same door and has no shortcut around it. The shipped `builder` drafts
manifests and holds no edge onto `/colony/mutations`, `submit` is the one node in the tree
that has one, and a mutation cannot draw the missing edge because that address is absent from
the endpoints a diff may name. The lock is
`crates/meclaw-cells/tests/gh425_the_builder_cannot_reach_the_mutation_door.rs`.

`submit` checks that a manifest's bytes are the ones its digest was drawn over, takes the
requester's identity off the envelope, and asks the capability broker whether that requester
may submit over the manifest's scope root. A manifest carrying a script override or an
`add_templates` is asking to author executable behaviour, so `code.author` is asked as a
second question. A colony can therefore already extend itself on request: an agent states a
wish, the builder drafts, somebody says yes to a digest, and the tree grows while it runs
([rewiring](../rewiring.md)).

## The control loop, and how far its arm reaches

`argus` is that loop, as a hive of seven cells. It reads a charter, measures its own colony out
of the substrate's ledger, has a model judge simulate against those numbers, sends the decision
to a named cell as an ordinary params update, checks health, measures the effect over the
charter's window, and then keeps the change or reverts it. A cycle without a pre-authored
revert plan is invalid and `./mutator` refuses it with `no_revert_plan`
(`templates/argus/template.json`). Every tick leaves an append-only receipt, including the tick
that found nothing enabled and the cycle that died at a store reject. Its reach is narrow:
nothing in that hive authors a diff ([GH #304](https://github.com/mmeyerlein/meclaw/issues/304)),
so it can move a parameter on a cell a human wired an edge to and can never add, remove or
rewire one. A topology idea is recorded as a proposal for a person and never executed.

## What is switched off

Both rows in `templates/argus/charter/seed/goals.jsonl` carry `"enabled": 0`. A freshly grown
loop ticks, receipts each tick as `idle`, and changes nothing until an operator turns a row on.
Nothing in this repository turns one on for you.

Recursive self-improvement means a system that measures itself, changes itself, and repeats
that unattended to get better at its own goals. This repository does not do that. The parts a
defensible version would need are here and tested: one recorded door, a second question asked
when a diff wants to author code, measurement drawn from a ledger the loop cannot write, and
revert as an outcome the charter demands up front. What is left is deciding which goals a
colony may pursue about itself, and that decision belongs to an operator.
