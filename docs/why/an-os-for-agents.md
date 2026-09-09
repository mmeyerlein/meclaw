# An operating system for agents

Why is `meclaw-os` cut into four nested levels instead of one flat agent? Because one rule
decides where a thing belongs: a level owns what its siblings must share. Put a thing one level
too low and it is duplicated or lost when a part is replaced; put it one level too high and two
siblings get one answer to a question that was theirs.

## The rule at work

Memory belongs to the person, not to their agent, so `memory-hive` hangs at the member level.
Replacing an assistant does not take the history with it, and two assistants of one person read
one record (`crates/meclaw-cells/tests/gh302_member_holds_the_memory.rs`).

Screening and channels belong to the person for the same reason. A new generation meets the
attacker record and the rate window the old one left behind, and one chat account reaches two
of that person's agents (`crates/meclaw-cells/tests/gh454_two_assistants_one_channel.rs`).

The control loop and the authoring path sit at the shell, because one colony has one charter and
one place a submission lives. A person's own keys are held one level down, by the member whose
agents authenticate with them (`templates/member/README.md`).

An organisation shares a name and a boundary and nothing else, so that level owns nothing else.
`templates/org/template.json` ships no cell at all: one hive, one open container, transit edges
derived from what the member below accepts and emits
(`crates/meclaw-cells/tests/gh302_org_is_a_namespace.rs`).

The assistant level is the one place that references both halves of a generation, which makes it
the level that can give each half its own model ([one assistant, two brains](two-brains.md)).

## What the levels do not decide

None of these authorities asks a model. `access` answers a capability request by comparison and
hands out a handle, `firewall` measures size, sender, forbidden literal and rate and names the
row that fired, and `session-keeper` ends a session by arithmetic, on the pattern of a phone
call. `argus` is the control loop, and everything it could pursue ships switched off: both rows
in `templates/argus/charter/seed/goals.jsonl` carry `"enabled": 0`, so a freshly grown shell
measures nothing until an operator turns one on ([prepared for self-improvement](rsi.md)).

## What is rudimentary about it

The tree ships one app and one screen layout, and what an app may offer a member is still moving
([ROADMAP](../../ROADMAP.md)). Nothing in a colony is ever deleted, so a generation you regret
stays on disk, disconnected.

The levels, their occupants and the mutation that grows each are on [meclaw-os](../meclaw-os.md).
