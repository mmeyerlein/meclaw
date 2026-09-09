# examples

Nine colonies and one step, in the order they are worth reading. Every one of them is a
directory tree plus edges, and none of them adds a line of Rust.

Each example is here to prove one claim, and the last column is that claim. Read
[`hard-shell`](hard-shell/) first: it has no `llm` cell, so there is nothing to authenticate
and nothing to pay for.

| example | cells | keyless | boot | why it exists |
|---|---:|---|---|---|
| [`hard-shell`](hard-shell/) | 1 → 3 | yes | < 1 s | an agent you are allowed to attack, out of a seed that configures no security at all |
| [`hello`](hello/) | 2 | no | < 1 s | one `llm` and one edge, which is the whole model in one folder |
| [`swarm`](swarm/) | 7 | no | < 1 s | the tool loop as a shape: fan-out, fan-in through a store, and a loopback edge into the `llm` |
| [`meclaw-os`](meclaw-os/) | 0 → 17 | to boot | < 1 s | an empty seed plus one declaration that grows the whole agent from the template library |
| [`organism`](organism/) | 0 → 92 | to boot | < 1 s | six declarations grow four levels of composition, 565 edges, 70 of them written by hand |
| [`never-forgets`](never-forgets/) | 3 → 16 | to boot | < 1 s | tell it in January, ask it in March, and the model names the time range it wants |
| [`vault-pilot`](vault-pilot/) | 2 → 8 | to boot | < 1 s | a model that holds no key, because its credential arrives sealed from the broker's vault |
| [`display-colony-view`](display-colony-view/) | 2 → 8 | yes | < 1 s | two owners on one screen, and neither can touch the other's view |
| [`telegram-research`](telegram-research/) | 10 | no | < 1 s | a multi-tool agent on a real surface, written out node by node |
| [`memory-import`](memory-import/) | 0 → one member | to boot | < 1 s | the one door a memory comes back through, so a member is born with its past |

`a → b` means *a* cells are checked in and *b* are running once the example's `grow.json` has
been applied. A hive is a scope marker and is not counted as a cell. `memory-import` is a step
rather than a colony: it has no seed of its own and adds one member to whichever org shell you
already have.

What "keyless" means here, because the distinction is a boot-time one:

- yes: boots and answers with no key anywhere.
- to boot: boots, grows and validates with no key, and needs a provider key before it can
  answer, because answering is what an `llm` cell does. `vault-pilot` wants its key in a vault
  instead of in `.env`, plus the passphrase that opens it.
- no: will not boot until the variable its `llm` cell substitutes exists. `hello`, `swarm` and
  `telegram-research` read `${OPENROUTER_API_KEY}`, the substitution reads `{root}/.env` (or
  `--env`) and never the process environment, and an unset variable fails the bootstrap with
  `env_var_missing`.

`--validate` checks a tree without spawning anything:

```bash
./target/release/meclaw --root ./examples/hard-shell/seed --templates ./templates --validate
```

Two of them carry a `WALKTHROUGH.md`, the same example end to end, every command in the order
it has to happen with the real output next to it. If you mean to run either one, start there.
[`hard-shell`](hard-shell/WALKTHROUGH.md) shows a refusal becoming a routed, typed event, in
under two minutes and without a key.
[`never-forgets`](never-forgets/WALKTHROUGH.md) shows an answer coming out of a months-old
episode, and it carries a seed step the README alone will not get you past.

Every colony here writes its conditions in the guarded CEL form (`has(hop.x) && hop.x == ...`),
which the sweep in `crates/meclaw-colony/tests/gh80_shipped_conditions_are_guarded.rs` enforces
over every shipped `config.json`. Copy from these examples instead of from an old README.
