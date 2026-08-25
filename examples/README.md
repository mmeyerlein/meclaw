# examples

Seven colonies, in the order they are worth reading. Every one of them is a directory tree plus
edges -- no example here adds a line of Rust.

| example | cells | keyless | boot | why it exists |
|---|---:|---|---|---|
| [`hard-shell`](hard-shell/) | 1 → 3 | **yes** | < 1 s | An agent you are allowed to attack: point it at the cloud metadata endpoint and watch a refusal become a routed, typed event -- from a seed that configures no security at all. |
| [`hello`](hello/) | 2 | no | < 1 s | The smallest colony that does something. One `llm`, one edge. If you understand this folder, you understand the model. |
| [`swarm`](swarm/) | 7 | no | < 1 s | The tool loop as a shape: fan-out to tools, fan-in through a store, and a loopback edge that re-enters the `llm`. |
| [`meclaw-os`](meclaw-os/) | 0 → 16 | to boot | < 1 s | The same class of agent, **not** written out: an EMPTY seed -- two config files, zero cells -- plus one declaration that instantiates the whole tree from the template library at runtime. |
| [`organism`](organism/) | 0 → 55 | to boot | < 1 s | The same empty seed, one level up: **five** declarations grow the whole four-level stack -- a colony shell, an organisation, a person, one generation of that person's agent and a Telegram surface for it -- each level instantiated into the open container the level above ships for it. 287 edges, 48 of them written by hand. |
| [`never-forgets`](never-forgets/) | 3 → 18 | to boot | < 1 s | Tell it something in January, ask it in March. Bi-temporal episodes, an import lane for a past you already have, and a model that names the time range it wants. |
| [`telegram-research`](telegram-research/) | 10 | no | < 1 s | A real multi-tool agent on a real surface, written out node by node. Needs a Telegram bot token as well as a provider key. |

`a → b` means *a* cells are checked in and *b* are running once the example's `grow.json` has
been applied. A hive is not a cell and is not counted: it is a scope marker, not an actor.

`hard-shell` is the one to run first, and the one the top-level README walks through: it has no
`llm` cell at all, so there is nothing to authenticate and nothing to pay for.

Two of them carry a **`WALKTHROUGH.md`** — the same example end to end, every command in the
order it has to happen with the real output next to it:
[`hard-shell`](hard-shell/WALKTHROUGH.md) (under two minutes, keyless) and
[`never-forgets`](never-forgets/WALKTHROUGH.md) (needs a key and a model). If you mean to
*run* either example rather than read it, start there — the walkthrough is the tested path,
and for `never-forgets` it carries a seed step the README alone will not get you past.

**What "keyless" means here**, because the distinction is a boot-time one:

- **yes** -- boots and *answers* with no key anywhere. Only `hard-shell`.
- **to boot** -- boots, grows and validates with no key; it needs a provider key before it can
  answer, because answering is what an `llm` cell does. `meclaw-os`, `never-forgets` and
  `organism`.
- **no** -- will not boot at all until the variable its `llm` cell substitutes exists. `hello`,
  `swarm` and `telegram-research` read `${OPENROUTER_API_KEY}`, and the substitution reads
  `{root}/.env` (or `--env`), **not** the process environment -- an unset variable fails the
  bootstrap with `env_var_missing` rather than starting something half-wired. Any value gets you
  through boot and `--validate`; a real one is needed to get an answer.

The `boot` column is time to a healthy HTTP API on an already-built binary, not the build.
`cargo build --release` is the slow step, and you pay it once.

`--validate` is the cheap way to check a tree without spawning anything:

```bash
./target/release/meclaw --root ./examples/hard-shell/seed --templates ./templates --validate
```

Every colony here runs the guarded CEL form (`has(hop.x) && hop.x == ...`), which the sweep in
`crates/meclaw-colony/tests/gh80_shipped_conditions_are_guarded.rs` enforces over every shipped
`config.json`. Copy from these, not from an old README.
